//! fleety-server — the Fleety Agent server.
//!
//! M2: a WebSocket server that accepts client connections, runs a session, does
//! a conversation round-trip (echo provider for now), and persists each
//! conversation as a JSONL event stream. Each connection is isolated so one
//! client can never crash the server.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod conn;
mod echo;
mod mcp;
mod scheduler;
mod schedules;
mod skills;
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
    let provider = build_provider();
    let policy = policy_from_env();
    let workspace = Arc::new(workspace_root());
    tracing::info!(workspace = %workspace.display(), "workspace for tools");

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
        // Each connection runs in its own task: an error or panic here is
        // isolated and never brings the server down.
        tokio::spawn(async move {
            match conn::handle_conn(stream, storage, provider, workspace, policy).await {
                Ok(()) => {}
                Err(e) => tracing::warn!(%peer, report = ?e.report(), "connection error"),
            }
        });
    }
}
