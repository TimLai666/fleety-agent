//! fleety-server — the Fleety Agent server.
//!
//! M2: a WebSocket server that accepts client connections, runs a session, does
//! a conversation round-trip (echo provider for now), and persists each
//! conversation as a JSONL event stream. Each connection is isolated so one
//! client can never crash the server.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod auth;
mod backup;
mod bridge;
mod builtin_mcp;
mod builtin_skills;
mod codex_sources;
mod conn;
mod conversation_embed;
mod conversation_lifecycle;
mod conversation_recall;
mod editor_tools;
mod effort;
mod embed;
mod gc;
mod http;
mod identity;
mod instructions;
mod mdns;
mod plugin_sources;
mod presence;
mod privacy;
mod sidecar;
mod sse;
mod triage;
mod tz;
mod websocket;

/// Process-wide moment fleety-server started. Used by `fleety status` to
/// compute uptime without threading a start-time through every connection.
pub(crate) fn server_start() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}
mod echo;
mod mcp;
mod pool;
mod providers;
mod scheduler;
mod schedules;
mod service;
mod sites;
mod skill_sources;
mod skill_sync;
mod skills;
mod ssh;
mod storage;
mod subagent;
mod tools;
mod web;
mod wiki;
mod wiki_embed;
#[cfg(windows)]
mod winsvc;
mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{obs, ModelProvider};
use tokio::net::TcpListener;

use crate::storage::Storage;

/// Resolve the Agent home (durable store), separate from any workspace.
fn agent_home() -> PathBuf {
    if let Ok(path) = std::env::var("FLEETY_AGENT_HOME") {
        return PathBuf::from(path);
    }
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join(".fleety").join("agent")
}

/// Workspace the read-only tools operate on (`FLEETY_WORKSPACE`, else cwd).
fn workspace_root() -> PathBuf {
    if let Ok(path) = std::env::var("FLEETY_WORKSPACE") {
        return PathBuf::from(path);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Choose the model provider: an OpenAI-compatible endpoint if configured via
/// `FLEETY_MODEL_BASE_URL` + `FLEETY_MODEL`, otherwise the offline echo stub.
fn build_provider() -> Arc<dyn ModelProvider> {
    providers::build_main()
}

/// Approval policy from `FLEETY_POLICY` (`require_approval` → gate non-read
/// tools; default full access).
fn policy_from_env() -> agent_core::Policy {
    match std::env::var("FLEETY_POLICY").as_deref() {
        Ok("require_approval") => agent_core::Policy::RequireApproval,
        _ => agent_core::Policy::FullAccess,
    }
}

fn main() {
    obs::init();
    // Seed env from ~/.fleety/config.toml before anything reads env: an explicit
    // env var always wins (we only fill what's unset), so existing deployments
    // are unaffected. Best-effort (a missing/corrupt file is ignored).
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    let cmd = std::env::args().nth(1);

    // `config ...` inspects/edits this host's settings (model, addr, token, …),
    // then exits — no runtime needed. Same command surface as `fleety config`.
    if cmd.as_deref() == Some("config") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        if let Err(e) = fleety_tools::config::run(&args) {
            tracing::error!(report = ?e.report(), "config failed");
        }
        return;
    }

    // On Windows, `run-service` is the SCM entry point and must speak the service
    // control protocol on this thread, before any tokio runtime exists.
    #[cfg(windows)]
    if cmd.as_deref() == Some("run-service") {
        if let Err(e) = winsvc::dispatch() {
            tracing::error!(
                %e,
                "windows service dispatcher failed; `run-service` only works when started by \
                 the Service Control Manager (use `fleety-server start` after `install`)"
            );
        }
        return;
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(%e, "cannot start tokio runtime");
            return;
        }
    };
    rt.block_on(async_main(cmd));
}

/// Log the outcome of a one-shot lifecycle verb.
fn log_action(name: &str, res: agent_core::Result<()>) {
    if let Err(e) = res {
        tracing::error!(report = ?e.report(), "{name} failed");
    }
}

/// Resolve when the server should stop cleanly: Ctrl+C anywhere, plus SIGTERM on
/// Unix (service stop) or the external `shutdown` watch on Windows (SCM Stop).
async fn wait_stop(shutdown: Option<tokio::sync::watch::Receiver<bool>>) {
    #[cfg(unix)]
    {
        let _ = &shutdown;
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(t) => { t.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        match shutdown {
            Some(mut rx) => {
                if *rx.borrow() {
                    return;
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    changed = rx.changed() => { let _ = changed; }
                }
            }
            None => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
}

async fn async_main(cmd: Option<String>) {
    // Service lifecycle subcommands (install/uninstall/start/stop/restart/
    // enable/disable/status/up/down) act on the manager and exit.
    if let Some(action) = cmd.as_deref().and_then(service::action_from_arg) {
        log_action(cmd.as_deref().unwrap_or("?"), service::run(action));
        return;
    }

    // `backup now` / `backup restore` run once against the user-configured repo
    // and exit (runtime needed for git/network). Restore is meant to be run with
    // the server stopped.
    if cmd.as_deref() == Some("backup") {
        run_backup_command(std::env::args().nth(2)).await;
        return;
    }

    // Service mode (non-Windows run-service) claims the single-instance pidfile;
    // foreground dev runs do not. On Windows the service path goes through winsvc.
    let service_mode = cmd.as_deref() == Some("run-service");
    let _pid_guard = if service_mode {
        match fleety_tools::service::acquire("fleety-server") {
            Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
            Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
                tracing::error!(pid, "another fleety-server is already running; exiting");
                return;
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(), "pidfile check failed; continuing without it");
                None
            }
        }
    } else {
        None
    };

    run_server(None).await;
}

/// Handle `fleety-server backup <now|restore>`: run once against the
/// user-configured repo, then exit. Prints a clear message when no repo is set.
async fn run_backup_command(sub: Option<String>) {
    let home = agent_home();
    let mirror = Storage::new(home.clone()).backup_mirror_dir();
    let config_path = fleety_tools::config::config_path();
    let providers_path = fleety_tools::providers_config::providers_path();
    let env = match backup::config_from_env() {
        Some(e) => e,
        None => {
            println!(
                "未設定備份 repo:請先設定 FLEETY_BACKUP_REPO(與 FLEETY_BACKUP_TOKEN),見 docs/env.md。"
            );
            return;
        }
    };
    match sub.as_deref() {
        Some("now") => {
            match backup::run_backup(
                &home,
                &config_path,
                &providers_path,
                &mirror,
                &env.repo,
                &env.token,
            )
            .await
            {
                Ok(backup::BackupOutcome::Pushed) => println!("備份完成並已推送到 {}。", env.repo),
                Ok(backup::BackupOutcome::NothingChanged) => {
                    println!("狀態未變更,無需備份。")
                }
                Ok(backup::BackupOutcome::RefusedNotPrivate) => println!(
                    "已在本地建立備份快照,但 repo {} 非私人(或無法確認),拒絕推送。請確認 repo 為私人。",
                    env.repo
                ),
                Err(e) => tracing::error!(report = ?e.report(), "backup now failed"),
            }
        }
        Some("restore") => {
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
            match backup::restore(
                &home,
                &config_path,
                &providers_path,
                &env.repo,
                &env.token,
                &ts,
            )
            .await
            {
                Ok(()) => println!(
                    "已從備份還原。請重新啟動 server;原有資料已保留在 .pre-restore-{ts}(未刪除)。"
                ),
                Err(e) => tracing::error!(report = ?e.report(), "backup restore failed"),
            }
        }
        _ => println!("用法:fleety-server backup <now|restore>"),
    }
}

/// Ensure this server's external dependencies (best-effort, non-blocking).
/// Default subset python+ddgs+node+insyra; `FLEETY_DEPS` overrides the list,
/// `FLEETY_AUTO_INSTALL_DEPS=0` disables installing.
async fn ensure_dependencies() {
    use fleety_tools::deps;
    let names = deps::selected_dep_names(
        std::env::var("FLEETY_DEPS").ok().as_deref(),
        deps::server_default_deps(),
    );
    let chosen: Vec<deps::Dependency> = names
        .iter()
        .filter_map(|n| match n.as_str() {
            "node" => Some(deps::node_dependency()),
            "python" => Some(deps::python_dependency()),
            "insyra" => Some(deps::insyra_dependency()),
            "ddgs" => Some(builtin_mcp::ddgs_dependency()),
            other => {
                tracing::warn!(dep = %other, "unknown dependency in FLEETY_DEPS; ignoring");
                None
            }
        })
        .collect();
    let outcomes = deps::ensure_all(&chosen).await;
    deps::log_outcomes(&outcomes);
}

/// Run the server until a stop signal. `shutdown` is the optional external stop
/// (Windows SCM); Ctrl+C and Unix SIGTERM are always honored (see [`wait_stop`]).
async fn run_server(shutdown: Option<tokio::sync::watch::Receiver<bool>>) {
    // Stamp the start time once so uptime reflects boot, not first status query.
    let _ = server_start();
    let addr = std::env::var("FLEETY_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let home = agent_home();
    tracing::info!(version = agent_core::VERSION, %addr, home = %home.display(), "fleety-server starting");

    let storage = Arc::new(Storage::new(home));
    // One-time, idempotent, lossless migration of legacy per-device conversations
    // to the user-primary layout (privacy model). Best-effort: a failure leaves
    // the legacy layout in place and never stops the server.
    match storage.migrate_conversations() {
        Ok(n) if n > 0 => {
            tracing::info!(
                migrated = n,
                "migrated conversations to user-primary layout"
            )
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(report = ?e.report(), "conversation migration skipped"),
    }
    // Seed built-in skills shipped in the binary (best-effort; a failure here
    // must not stop the server from serving).
    if let Err(e) = builtin_skills::seed(&storage.skills_builtin_dir()) {
        tracing::warn!(error = %e, "could not seed built-in skills");
    }
    // Seed built-in MCP servers (ddgs for web search) so the agent has them
    // available without a manual `mcp_add`. Same posture as builtin_skills.
    if let Err(e) = builtin_mcp::seed(&storage.mcp_builtin_config_path()) {
        tracing::warn!(error = %e, "could not seed built-in MCP servers");
    }
    // Background: ensure this server's external dependencies (python+ddgs+node+
    // insyra by default; override with FLEETY_DEPS, disable with
    // FLEETY_AUTO_INSTALL_DEPS=0). Best-effort and non-blocking — a failure is
    // logged but never stops the server from starting.
    tokio::spawn(ensure_dependencies());
    // 24h background loop that keeps built-in MCPs at their latest upstream
    // version. Same opt-out as install (FLEETY_DDGS_AUTO_INSTALL=0).
    builtin_mcp::spawn_auto_upgrade_loop();
    // Keep a managed chrome-for-testing build (if we downloaded one) current.
    // A system / package-manager Chrome self-updates, so this is a no-op there.
    tokio::spawn(async {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        ticker.tick().await; // immediate tick — skip
        loop {
            ticker.tick().await;
            fleety_tools::upgrade_managed_chrome().await;
        }
    });
    // Warm the wiki semantic-search index in the background (downloads the
    // EmbeddingGemma model on first run; opt out with FLEETY_WIKI_EMBED=0).
    tokio::spawn(wiki_embed::warm(storage.wiki_dir(), storage.models_dir()));
    let provider = build_provider();
    let policy = policy_from_env();
    let workspace = Arc::new(workspace_root());
    tracing::info!(workspace = %workspace.display(), "workspace for tools");

    // Cross-device routing state, shared across all connections.
    let hub = bridge::new_hub();
    let pending = bridge::new_pending();
    let handles = bridge::new_handles();
    let device_tools = bridge::new_device_tools();

    // Connection auth: enforced only with FLEETY_REQUIRE_AUTH=1; FLEETY_TOKEN is
    // a bootstrap admin token for pairing the first device.
    let require_auth = std::env::var("FLEETY_REQUIRE_AUTH").as_deref() == Ok("1");
    let auth = Arc::new(auth::AuthStore::load(
        storage.auth_path(),
        std::env::var("FLEETY_TOKEN").ok(),
        require_auth,
    ));
    tracing::info!(require_auth, "connection auth");

    // Schedule fire loop (unattended): checks for due schedules periodically.
    let tick_secs = std::env::var("FLEETY_SCHED_TICK")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    scheduler::spawn(
        Arc::clone(&storage),
        Arc::clone(&provider),
        Arc::clone(&workspace),
        Arc::clone(&device_tools),
        tick_secs,
    );

    // Eagerly finish any interactive turn interrupted by a crash/redeploy, so it
    // doesn't wait for the user to reconnect. Runs in the background.
    // Periodic retention / GC of audit + backup surfaces (skipped if the user
    // sets FLEETY_GC_DISABLED).
    gc::spawn(Arc::clone(&storage));

    // Keep the `synced` skill tier in step with the external skills repo
    // (FLEETY_SKILLS_SYNC=0 disables). Background, never-crash.
    skill_sync::spawn(storage.skills_synced_dir());

    // Auto-backup the non-regenerable state to a user-configured private repo
    // (inert unless FLEETY_BACKUP_REPO is set). Background, never-crash.
    backup::spawn(
        agent_home(),
        fleety_tools::config::config_path(),
        fleety_tools::providers_config::providers_path(),
        storage.backup_mirror_dir(),
    );

    tokio::spawn(conn::recover_all_interactive(
        Arc::clone(&storage),
        Arc::clone(&provider),
        Arc::clone(&workspace),
        policy,
        Arc::clone(&hub),
        Arc::clone(&pending),
        Arc::clone(&handles),
        Arc::clone(&auth),
        Arc::clone(&device_tools),
    ));

    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(%addr, "cannot bind: {e}; is the port already in use?");
            return;
        }
    };
    tracing::info!(%addr, "listening");
    // Announce ourselves via mDNS so daemons / the CLI on the same LAN can
    // find us without a hand-typed URL. No-op when disabled.
    mdns::spawn_advertise(&addr);

    // Serve WebSocket + (added by this change) the SSE+POST fallback through one
    // axum app on this port. `conn::handle_conn` (raw-TCP WebSocket) stays for the
    // integration tests; in production axum owns the listener and routes to the
    // same transport-agnostic `conn::run_connection`.
    let state = http::AppState {
        storage: Arc::clone(&storage),
        provider: Arc::clone(&provider),
        workspace: Arc::clone(&workspace),
        policy,
        hub: Arc::clone(&hub),
        pending: Arc::clone(&pending),
        handles: Arc::clone(&handles),
        auth: Arc::clone(&auth),
        device_tools: Arc::clone(&device_tools),
        sse_sessions: http::new_sse_sessions(),
    };
    let app = http::router(state);
    tokio::select! {
        r = axum::serve(listener, app.into_make_service()) => {
            if let Err(e) = r {
                tracing::error!(%e, "http server error");
            }
        }
        _ = wait_stop(shutdown.clone()) => {
            tracing::info!("stop signal received; shutting down");
        }
    }
    // Close every live connection's writer channel so peers see the shutdown
    // immediately (an in-flight connection task ends when its writer closes).
    let mut hub = hub.lock().await;
    for (device, _) in hub.drain() {
        tracing::info!(%device, "closing connection on shutdown");
    }
    tracing::info!("fleety-server stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Message;
    use std::io::{Read, Write};
    use std::net::TcpListener as StdTcpListener;
    use std::sync::Mutex;
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
        temp_home: PathBuf,
    }

    impl EnvGuard {
        fn new(name: &str) -> Self {
            let keys = [
                "HOME",
                "USERPROFILE",
                "FLEETY_AGENT_HOME",
                "FLEETY_WORKSPACE",
                "FLEETY_POLICY",
                "FLEETY_MODEL_BASE_URL",
                "FLEETY_MODEL",
                "FLEETY_MODEL_KEY",
                "FLEETY_MODEL_STREAM",
            ];
            let saved = keys
                .into_iter()
                .map(|key| (key, std::env::var(key).ok()))
                .collect::<Vec<_>>();
            let temp_home = std::env::temp_dir()
                .join(format!("fleety-server-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&temp_home);
            std::fs::create_dir_all(&temp_home).expect("temp home");

            std::env::set_var("HOME", &temp_home);
            std::env::set_var("USERPROFILE", &temp_home);
            for key in [
                "FLEETY_AGENT_HOME",
                "FLEETY_WORKSPACE",
                "FLEETY_POLICY",
                "FLEETY_MODEL_BASE_URL",
                "FLEETY_MODEL",
                "FLEETY_MODEL_KEY",
                "FLEETY_MODEL_STREAM",
            ] {
                std::env::remove_var(key);
            }

            Self { saved, temp_home }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            let _ = std::fs::remove_dir_all(&self.temp_home);
        }
    }

    fn serve_once(body: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = StdTcpListener::bind("127.0.0.1:0").expect("bind fake provider");
        let addr = listener.local_addr().expect("fake provider addr");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fake provider request");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).expect("read provider request");
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let _ = tx.send(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write provider response");
        });
        (format!("http://{addr}/v1"), rx)
    }

    #[test]
    fn agent_home_prefers_env_then_home_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let guard = EnvGuard::new("agent-home");

        assert_eq!(agent_home(), guard.temp_home.join(".fleety").join("agent"));

        let explicit = guard.temp_home.join("custom-agent-home");
        std::env::set_var("FLEETY_AGENT_HOME", &explicit);
        assert_eq!(agent_home(), explicit);
    }

    #[test]
    fn workspace_root_prefers_env_then_current_dir() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let guard = EnvGuard::new("workspace-root");

        assert_eq!(
            workspace_root(),
            std::env::current_dir().expect("current dir")
        );

        let explicit = guard.temp_home.join("workspace");
        std::env::set_var("FLEETY_WORKSPACE", &explicit);
        assert_eq!(workspace_root(), explicit);
    }

    #[test]
    fn policy_from_env_defaults_to_full_access_unless_exact_match() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("policy");

        assert_eq!(policy_from_env(), agent_core::Policy::FullAccess);
        std::env::set_var("FLEETY_POLICY", "RequireApproval");
        assert_eq!(policy_from_env(), agent_core::Policy::FullAccess);
        std::env::set_var("FLEETY_POLICY", "require_approval");
        assert_eq!(policy_from_env(), agent_core::Policy::RequireApproval);
    }

    #[tokio::test]
    async fn build_provider_uses_echo_when_model_env_is_incomplete() {
        let provider = {
            let _lock = ENV_LOCK.lock().expect("env lock");
            let _guard = EnvGuard::new("provider");
            std::env::set_var("FLEETY_MODEL_BASE_URL", "http://localhost:1234/v1");
            build_provider()
        };
        let response = provider
            .complete(&[Message::user("hello")], &[])
            .await
            .expect("echo provider");
        assert_eq!(response.message.content.as_deref(), Some("echo: hello"));
    }

    #[tokio::test]
    async fn build_provider_uses_openai_compatible_env_when_complete() {
        let body = r#"{"choices":[{"message":{"content":"provider-ok","tool_calls":[]}}]}"#;
        let (base_url, rx) = serve_once(body.to_string());
        let provider = {
            let _lock = ENV_LOCK.lock().expect("env lock");
            let _guard = EnvGuard::new("provider-openai");
            std::env::set_var("FLEETY_MODEL_BASE_URL", base_url);
            std::env::set_var("FLEETY_MODEL", "server-model");
            std::env::set_var("FLEETY_MODEL_KEY", "server-key");
            build_provider()
        };

        let response = provider
            .complete(&[Message::user("hello")], &[])
            .await
            .expect("openai-compatible provider");
        assert_eq!(response.message.content.as_deref(), Some("provider-ok"));

        let request = rx.recv_timeout(Duration::from_secs(5)).expect("request");
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.contains("Bearer server-key"));
        assert!(request.contains("\"model\":\"server-model\""));
    }
}
