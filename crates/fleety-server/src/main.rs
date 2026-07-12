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
mod hooks_compat;
mod http;
mod identity;
mod instructions;
mod mdns;
mod plugin_sources;
mod presence;
mod privacy;
mod restart_watch;
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

/// Whether connections must authenticate, from `FLEETY_REQUIRE_AUTH`. Auth is
/// **on by default** now: any value other than an explicit `0` requires a token.
/// `seed_env_from_config` runs first, so a `config.toml` value is already in env.
fn require_auth_from_env() -> bool {
    std::env::var("FLEETY_REQUIRE_AUTH").as_deref() != Ok("0")
}

/// Approval policy from `FLEETY_POLICY` (`require_approval` → gate non-read
/// tools; default full access).
fn policy_from_env() -> agent_core::Policy {
    match std::env::var("FLEETY_POLICY").as_deref() {
        Ok("require_approval") => agent_core::Policy::RequireApproval,
        _ => agent_core::Policy::FullAccess,
    }
}

fn main() -> std::process::ExitCode {
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
            let report = e.report();
            eprintln!("error: {}", report.message);
            if let Some(hint) = report.remediation {
                eprintln!("hint: {hint}");
            }
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
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
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start tokio runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    rt.block_on(async_main(cmd))
}

/// Report a one-shot lifecycle verb: quiet on success (the verb itself prints),
/// a clean stderr line + non-zero exit on failure so users and scripts can tell.
fn log_action(name: &str, res: agent_core::Result<()>) -> std::process::ExitCode {
    match res {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            let report = e.report();
            eprintln!("error: {name} failed: {}", report.message);
            if let Some(hint) = report.remediation {
                eprintln!("hint: {hint}");
            }
            std::process::ExitCode::FAILURE
        }
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

/// The server's persistent identity fingerprint: a UUID minted on first start
/// and stored at `<agent home>/server-id`, stable across restarts and address
/// changes. Clients pin it at pairing and use it to re-find this exact server
/// when its address moves. Empty until initialized.
static SERVER_FINGERPRINT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn server_fingerprint() -> &'static str {
    SERVER_FINGERPRINT.get().map(String::as_str).unwrap_or("")
}

fn init_server_fingerprint(home: &std::path::Path) {
    let id_path = home.join("server-id");
    let _ = SERVER_FINGERPRINT.set(load_or_mint_server_id(&id_path));
}

/// Load the persisted server id, or mint and persist a new one. An unreadable
/// or unwritable id file degrades to a per-run id with a warning — the server
/// never fails to start over identity bookkeeping (but sticky healing on
/// enrolled devices will not recognize a fingerprint that changes per run).
fn load_or_mint_server_id(path: &std::path::Path) -> String {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, &id) {
        tracing::warn!(path = %path.display(), error = %e, "could not persist the server identity id; using a per-run id (sticky healing will not survive restarts until this is writable)");
    }
    id
}

async fn async_main(cmd: Option<String>) -> std::process::ExitCode {
    // Print the version and exit — before anything else, so `-v` never falls
    // through to accidentally starting the server (which then fails to bind).
    if matches!(
        cmd.as_deref(),
        Some("version") | Some("--version") | Some("-v") | Some("-V")
    ) {
        println!("fleety-server {}", agent_core::VERSION);
        return std::process::ExitCode::SUCCESS;
    }
    // Service lifecycle subcommands (install/uninstall/start/stop/restart/
    // enable/disable/status/up/down) act on the manager and exit.
    if let Some(action) = cmd.as_deref().and_then(service::action_from_arg) {
        // `restart` takes a `--force` flag: forced restarts immediately; otherwise
        // a running server is asked to restart once idle (deferred). Other verbs
        // ignore extra args.
        if action == service::Action::Restart {
            let force = std::env::args().skip(2).any(|a| a == "--force");
            return log_action("restart", service::restart(force));
        }
        let name = cmd.as_deref().unwrap_or("?");
        let result = service::run(action);
        // Symmetry with fleetyd's install branch: on a successful install/up,
        // provision the data-analysis sidecar (best-effort) so the user hears now
        // — not at a later insyra_exec failure — if it couldn't be fetched. A
        // provisioning failure never fails install/up.
        if result.is_ok() && matches!(action, service::Action::Install | service::Action::Up) {
            if let Err(e) = fleety_tools::deps::insyra::ensure_insyra(false).await {
                eprintln!(
                    "note: could not provision the fleety-insyra sidecar ({}); on-host data \
                     analysis (insyra_exec) will be unavailable until it is provisioned — the \
                     server retries this automatically on each start (or after a later update)",
                    e.report().message
                );
            }
        }
        return log_action(name, result);
    }

    // `update`: self-update from the release manifest (the {bin} template
    // resolves this binary's own manifest), refresh the sidecar either way, and
    // — when a new binary landed — hand over via the idle-deferred restart so
    // an in-flight turn is never interrupted. The server-only-host counterpart
    // of `fleetyd update` / `fleety update`.
    if cmd.as_deref() == Some("update") {
        let mut code = std::process::ExitCode::SUCCESS;
        match fleety_tools::update::self_update().await {
            Ok(true) => {
                println!("requesting an idle-deferred restart to run the new version…");
                if let Err(e) = service::restart(false) {
                    eprintln!(
                        "updated, but could not restart automatically ({}) — restart \
                         fleety-server to apply",
                        e.report().message
                    );
                }
            }
            Ok(false) => {}
            Err(e) => {
                let r = e.report();
                eprintln!("error: {}", r.message);
                if let Some(hint) = r.remediation {
                    eprintln!("hint: {hint}");
                }
                code = std::process::ExitCode::FAILURE;
            }
        }
        // Refresh the data-analysis sidecar alongside the server (best-effort).
        if let Err(e) = fleety_tools::deps::insyra::ensure_insyra(true).await {
            eprintln!(
                "note: could not refresh the fleety-insyra sidecar ({}); insyra_exec may stay \
                 on the old version or be unavailable",
                e.report().message
            );
        }
        return code;
    }

    // `backup now` / `backup restore` run once against the user-configured repo
    // and exit (runtime needed for git/network). Restore is meant to be run with
    // the server stopped.
    if cmd.as_deref() == Some("backup") {
        return run_backup_command(std::env::args().nth(2)).await;
    }

    // Service mode (non-Windows run-service) claims the single-instance pidfile;
    // foreground dev runs do not. On Windows the service path goes through winsvc.
    let service_mode = cmd.as_deref() == Some("run-service");
    let _pid_guard = if service_mode {
        match fleety_tools::service::acquire("fleety-server") {
            Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
            Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
                tracing::error!(pid, "another fleety-server is already running; exiting");
                return std::process::ExitCode::FAILURE;
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
    std::process::ExitCode::SUCCESS
}

/// Print a failed backup/restore as a clean stderr line (not a raw log record).
fn backup_fail(what: &str, e: agent_core::CoreError) -> std::process::ExitCode {
    let report = e.report();
    eprintln!("error: {what} failed: {}", report.message);
    if let Some(hint) = report.remediation {
        eprintln!("hint: {hint}");
    }
    std::process::ExitCode::FAILURE
}

/// Handle `fleety-server backup <now|restore>`: run once against the
/// user-configured repo, then exit. Prints a clear message when no repo is set.
async fn run_backup_command(sub: Option<String>) -> std::process::ExitCode {
    let home = agent_home();
    let mirror = Storage::new(home.clone()).backup_mirror_dir();
    let config_path = fleety_tools::config::config_path();
    let providers_path = fleety_tools::providers_config::providers_path();
    let env = match backup::config_from_env() {
        Some(e) => e,
        None => {
            eprintln!(
                "no backup repo configured: set FLEETY_BACKUP_REPO (and FLEETY_BACKUP_TOKEN) \
                 first — see docs/env.md."
            );
            return std::process::ExitCode::from(2);
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
                Ok(backup::BackupOutcome::Pushed) => {
                    println!("backup complete and pushed to {}.", env.repo);
                    std::process::ExitCode::SUCCESS
                }
                Ok(backup::BackupOutcome::NothingChanged) => {
                    println!("nothing changed since the last backup; nothing to push.");
                    std::process::ExitCode::SUCCESS
                }
                Ok(backup::BackupOutcome::RefusedNotPrivate) => {
                    eprintln!(
                        "a local backup snapshot was created, but {} is not private (or its \
                         visibility could not be confirmed) — refusing to push. Make the repo \
                         private and retry.",
                        env.repo
                    );
                    std::process::ExitCode::FAILURE
                }
                Err(e) => backup_fail("backup now", e),
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
                Ok(()) => {
                    println!(
                        "restored from backup. Restart the server; the previous data was kept \
                         at .pre-restore-{ts} (not deleted)."
                    );
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => backup_fail("backup restore", e),
            }
        }
        _ => {
            eprintln!("usage: fleety-server backup <now|restore>");
            std::process::ExitCode::from(2)
        }
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
            "crv" => Some(deps::crv_dependency()),
            "ffmpeg" => Some(deps::ffmpeg_dependency()),
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
    // Boot gate: migrate a legacy providers.toml to the two-tier shape and refuse
    // to boot on a present-but-broken / referentially incomplete provider config,
    // rather than silently degrading to the echo stub (design M5). A missing file
    // is fine — the FLEETY_MODEL_* env bootstrap seed takes over.
    if let Err(e) = providers::migrate_and_check() {
        let report = e.report();
        eprintln!("error: providers.toml is unusable: {}", report.message);
        if let Some(hint) = report.remediation {
            eprintln!("hint: {hint}");
        }
        std::process::exit(1);
    }
    // Stamp the start time once so uptime reflects boot, not first status query.
    let _ = server_start();
    // Codex credentials are per-provider now: drop any pre-per-provider global
    // token file on boot (no migration — each provider signs in fresh).
    fleety_tools::oauth::clear_legacy_global(&fleety_tools::oauth::default_token_path());
    // Default to all interfaces so a bare-metal server is reachable across
    // devices out of the box (auth is required by default). Set FLEETY_ADDR=
    // 127.0.0.1:8787 for loopback-only.
    let addr = std::env::var("FLEETY_ADDR").unwrap_or_else(|_| "0.0.0.0:8787".to_string());
    let home = agent_home();
    tracing::info!(version = agent_core::VERSION, %addr, home = %home.display(), "fleety-server starting");

    // Deferred-restart channel: first clear any restart-request marker left from
    // before this start (the restart it asked for is *this* start, so acting on
    // it would loop), then watch for new non-forced `restart` requests and carry
    // them out once idle / past the deadline. This watcher is the only path that
    // turns a marker into a manager restart. See `restart_watch`.
    match service::spec() {
        Ok(spec) => {
            restart_watch::clear_stale_request(&spec.name);
            restart_watch::spawn_watcher(spec);
        }
        Err(e) => {
            tracing::warn!(report = ?e.report(), "restart watcher not started (no service spec)")
        }
    }

    let storage = Arc::new(Storage::new(home));
    // Mint-or-load the persistent server identity (advertised over mDNS and in
    // Welcome) so enrolled devices can re-find this exact server after an IP
    // change.
    init_server_fingerprint(&storage.home());
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

    // Connection auth is on by default (set FLEETY_REQUIRE_AUTH=0 to disable);
    // FLEETY_TOKEN is a bootstrap admin token for pairing the first device.
    let require_auth = require_auth_from_env();
    let auth = Arc::new(auth::AuthStore::load(
        storage.auth_path(),
        std::env::var("FLEETY_TOKEN").ok(),
        require_auth,
    ));
    tracing::info!(require_auth, "connection auth");
    // First-run guidance: an auth-required server with no way in yet (no
    // bootstrap token, no paired device) would be an unpairable brick — mint a
    // short-lived pairing code and show the exact next step.
    if require_auth && auth.is_uninitialized() {
        match auth.create_pairing() {
            Ok(code) => tracing::warn!(
                pairing_code = %code,
                "first run: authentication is required but no device is paired yet. \
                 On a device, run `fleety init <this-server-ws-url>` then `fleety pair {code}` \
                 within 10 minutes to enroll it.",
            ),
            Err(e) => {
                tracing::warn!(report = ?e.report(), "could not mint a first-run pairing code")
            }
        }
    }

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
    if addr.starts_with("127.0.0.1") || addr.starts_with("localhost") {
        tracing::info!(
            "bound to loopback — other devices cannot reach this server; set \
             FLEETY_ADDR=0.0.0.0:8787 to expose it on the LAN"
        );
    }
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
        ws_liveness: http::WsLiveness::from_env(),
    };
    let app = http::router(state);
    tokio::select! {
        // ConnectInfo carries each connection's peer socket address so the
        // handlers can grant same-host loopback trust (see conn::authenticate).
        r = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        ) => {
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
    #[test]
    fn server_id_is_minted_once_and_persists() {
        let dir = std::env::temp_dir().join(format!("fleety-srvid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let path = dir.join("server-id");
        let first = super::load_or_mint_server_id(&path);
        assert!(!first.is_empty());
        let second = super::load_or_mint_server_id(&path);
        assert_eq!(first, second, "the persisted id is stable across loads");
        // An unwritable location degrades to a per-run id without panicking.
        let blocked = dir.join("blocker");
        std::fs::write(&blocked, "file").expect("write");
        let degraded = super::load_or_mint_server_id(&blocked.join("server-id"));
        assert!(!degraded.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
                "FLEETY_REQUIRE_AUTH",
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
                "FLEETY_REQUIRE_AUTH",
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

    #[test]
    fn require_auth_defaults_on_unless_explicitly_zero() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("require-auth");

        // Unset → auth required by default (the security-on default).
        assert!(require_auth_from_env(), "unset → auth required");
        // Explicit 0 is the only way to disable it.
        std::env::set_var("FLEETY_REQUIRE_AUTH", "0");
        assert!(!require_auth_from_env(), "explicit 0 → disabled");
        std::env::set_var("FLEETY_REQUIRE_AUTH", "1");
        assert!(require_auth_from_env(), "explicit 1 → required");
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
