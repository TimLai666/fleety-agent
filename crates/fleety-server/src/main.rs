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
mod gc;
mod mdns;

/// Process-wide moment fleety-server started. Used by `fleety status` to
/// compute uptime without threading a start-time through every connection.
pub(crate) fn server_start() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}
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
    // Stamp the start time once so uptime reflects boot, not first status query.
    let _ = server_start();
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

    loop {
        let accept = listener.accept();
        let stop = tokio::signal::ctrl_c();
        tokio::select! {
            accepted = accept => {
                let (stream, peer) = match accepted {
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
                let device_tools = Arc::clone(&device_tools);
                tokio::spawn(async move {
                    match conn::handle_conn(
                        stream,
                        storage,
                        provider,
                        workspace,
                        policy,
                        hub,
                        pending,
                        handles,
                        auth,
                        device_tools,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(e) => {
                            tracing::warn!(%peer, report = ?e.report(), "connection error")
                        }
                    }
                });
            }
            _ = stop => {
                tracing::info!("Ctrl+C received; closing listener and shutting down");
                // Drop the listener so no new connections come in. In-flight
                // connections finish their current turn (the journal protects
                // anything they're mid-step on if they don't).
                drop(listener);
                // Close every live connection's writer channel so peers see
                // the shutdown immediately, not on the next ping timeout.
                let mut hub = hub.lock().await;
                for (device, _) in hub.drain() {
                    tracing::info!(%device, "closing connection on shutdown");
                }
                tracing::info!("fleety-server stopped");
                return;
            }
        }
    }
}
