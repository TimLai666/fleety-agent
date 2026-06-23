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
mod bridge;
mod browser;
mod builtin_skills;
mod conn;
mod echo;
mod mcp;
mod scheduler;
mod schedules;
mod sites;
mod skills;
mod ssh;
mod storage;
mod tools;
mod web;
mod wiki;

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{obs, ModelProvider, OpenAiCompat};
use tokio::net::TcpListener;

use crate::echo::EchoProvider;
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
    match (
        std::env::var("FLEETY_MODEL_BASE_URL"),
        std::env::var("FLEETY_MODEL"),
    ) {
        (Ok(base_url), Ok(model)) => {
            let key = std::env::var("FLEETY_MODEL_KEY").ok();
            let stream = std::env::var("FLEETY_MODEL_STREAM").as_deref() == Ok("1");
            tracing::info!(%base_url, %model, stream, "using OpenAI-compatible provider");
            Arc::new(OpenAiCompat::new(base_url, model, key).with_streaming(stream))
        }
        _ => {
            tracing::info!("no FLEETY_MODEL_BASE_URL/FLEETY_MODEL set; using echo provider");
            Arc::new(EchoProvider)
        }
    }
}

/// Approval policy from `FLEETY_POLICY` (`require_approval` → gate non-read
/// tools; default full access).
fn policy_from_env() -> agent_core::Policy {
    match std::env::var("FLEETY_POLICY").as_deref() {
        Ok("require_approval") => agent_core::Policy::RequireApproval,
        _ => agent_core::Policy::FullAccess,
    }
}

#[tokio::main]
async fn main() {
    obs::init();
    let addr = std::env::var("FLEETY_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let home = agent_home();
    tracing::info!(version = agent_core::VERSION, %addr, home = %home.display(), "fleety-server starting");

    let storage = Arc::new(Storage::new(home));
    // Seed built-in skills shipped in the binary (best-effort; a failure here
    // must not stop the server from serving).
    if let Err(e) = builtin_skills::seed(&storage.skills_builtin_dir()) {
        tracing::warn!(error = %e, "could not seed built-in skills");
    }
    let provider = build_provider();
    let policy = policy_from_env();
    let workspace = Arc::new(workspace_root());
    tracing::info!(workspace = %workspace.display(), "workspace for tools");

    // Cross-device routing state, shared across all connections.
    let hub = bridge::new_hub();
    let pending = bridge::new_pending();
    let handles = bridge::new_handles();

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
        tick_secs,
    );

    // Eagerly finish any interactive turn interrupted by a crash/redeploy, so it
    // doesn't wait for the user to reconnect. Runs in the background.
    tokio::spawn(conn::recover_all_interactive(
        Arc::clone(&storage),
        Arc::clone(&provider),
        Arc::clone(&workspace),
        policy,
        Arc::clone(&hub),
        Arc::clone(&pending),
        Arc::clone(&handles),
        Arc::clone(&auth),
    ));

    let listener = match TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(%addr, "cannot bind: {e}; is the port already in use?");
            return;
        }
    };
    tracing::info!(%addr, "listening");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                continue;
            }
        };
        let storage = Arc::clone(&storage);
        let provider = Arc::clone(&provider);
        let workspace = Arc::clone(&workspace);
        let hub = Arc::clone(&hub);
        let pending = Arc::clone(&pending);
        let handles = Arc::clone(&handles);
        let auth = Arc::clone(&auth);
        // Each connection runs in its own task: an error or panic here is
        // isolated and never brings the server down.
        tokio::spawn(async move {
            match conn::handle_conn(
                stream, storage, provider, workspace, policy, hub, pending, handles, auth,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => tracing::warn!(%peer, report = ?e.report(), "connection error"),
            }
        });
    }
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
