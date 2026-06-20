//! fleety-server — the Fleety Agent server.
//!
//! M2: a WebSocket server that accepts client connections, runs a session, does
//! a conversation round-trip (echo provider for now), and persists each
//! conversation as a JSONL event stream. Each connection is isolated so one
//! client can never crash the server.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

mod conn;
mod echo;
mod storage;

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::obs;
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

#[tokio::main]
async fn main() {
    obs::init();
    let addr = std::env::var("FLEETY_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let home = agent_home();
    tracing::info!(version = agent_core::VERSION, %addr, home = %home.display(), "fleety-server starting");

    let storage = Arc::new(Storage::new(home));
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
        // Each connection runs in its own task: an error or panic here is
        // isolated and never brings the server down.
        tokio::spawn(async move {
            match conn::handle_conn(stream, storage).await {
                Ok(()) => {}
                Err(e) => tracing::warn!(%peer, report = ?e.report(), "connection error"),
            }
        });
    }
}
