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
mod builtin_mcp;
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
    // Seed built-in MCP servers so the agent has them available without a
    // manual `mcp_add` (best-effort; same posture).
    if let Err(e) = builtin_mcp::seed(&storage.mcp_builtin_config_path()) {
        tracing::warn!(error = %e, "could not seed built-in mcp servers");
    }
    let provider = build_provider();
    let policy = policy_from_env();
    let workspace = Arc::new(workspace_root());
    tracing::info!(workspace = %workspace.display(), "workspace for tools");

    // Kick off codebase-memory's first index of the workspace in the background
    // so the very first structural query has data — without blocking startup.
    {
        let workspace_for_index = Arc::clone(&workspace);
        tokio::spawn(async move {
            builtin_mcp::auto_index_workspace(&workspace_for_index).await;
        });
    }

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
