//! fleetyd — the Fleety device background service.
//!
//! Connects to the Agent on startup (registering this device) and holds the
//! connection open. `fleetyd install`/`uninstall` set up OS autostart. Heartbeat
//! is handled by WebSocket control frames; reconnect/backoff and on-device tool
//! execution are later milestones.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod ondevice;
mod provision;
mod service;
mod update;

use agent_core::{obs, CoreError, Result};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

#[tokio::main]
async fn main() {
    obs::init();
    // Subcommands: `install` / `uninstall` configure OS autostart, then exit.
    match std::env::args().nth(1).as_deref() {
        Some("install") => {
            if let Err(e) = service::install() {
                tracing::error!(report = ?e.report(), "install failed");
            }
            // Provision the data-analysis sidecar (best-effort).
            if let Err(e) = provision::ensure_insyra(false).await {
                tracing::warn!(report = ?e.report(), "could not provision fleety-insyra sidecar");
            }
            return;
        }
        Some("uninstall") => {
            if let Err(e) = service::uninstall() {
                tracing::error!(report = ?e.report(), "uninstall failed");
            }
            return;
        }
        Some("update") => {
            if let Err(e) = update::update().await {
                tracing::error!(report = ?e.report(), "update failed");
            }
            // Refresh the data-analysis sidecar alongside fleetyd (best-effort).
            if let Err(e) = provision::ensure_insyra(true).await {
                tracing::warn!(report = ?e.report(), "could not refresh fleety-insyra sidecar");
            }
            return;
        }
        _ => {}
    }
    tracing::info!(version = agent_core::VERSION, "fleetyd starting");
    if let Err(e) = run().await {
        tracing::error!(report = ?e.report(), "fleetyd exited with error");
    }
}

fn agent_url() -> String {
    std::env::var("FLEETY_AGENT_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787".to_string())
}

fn device_id() -> String {
    std::env::var("FLEETY_DEVICE_ID")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "fleetyd-device".to_string())
}

async fn run() -> Result<()> {
    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    let hello = serde_json::to_string(&ClientMsg::Hello {
        device_id: device_id(),
        protocol: PROTOCOL_VERSION,
        token: std::env::var("FLEETY_TOKEN").ok(),
        pairing_code: std::env::var("FLEETY_PAIRING_CODE").ok(),
    })
    .map_err(|e| CoreError::Message(format!("serialize hello: {e}")))?;
    tx.send(WsMessage::Text(hello))
        .await
        .map_err(|e| CoreError::Provider(format!("send hello failed: {e}")))?;

    let registry = ondevice::build_local_registry(&ondevice::device_root());
    tracing::info!(%url, "connected; holding connection");
    while let Some(frame) = rx.next().await {
        let frame =
            frame.map_err(|e| CoreError::Provider(format!("websocket read failed: {e}")))?;
        if frame.is_close() {
            break;
        }
        if !frame.is_text() {
            continue;
        }
        let Ok(text) = frame.to_text() else { continue };
        let Ok(msg) = serde_json::from_str::<ServerMsg>(text) else {
            continue;
        };
        match msg {
            ServerMsg::Welcome { session_id, .. } => {
                tracing::info!(%session_id, "registered with agent");
            }
            ServerMsg::RunTool {
                call_id,
                tool,
                args_json,
            } => {
                let args: serde_json::Value =
                    serde_json::from_str(&args_json).unwrap_or_else(|_| serde_json::json!({}));
                tracing::info!(%tool, "running on-device tool");
                let reply = match registry.call(&tool, args).await {
                    Ok(value) => ClientMsg::ToolResult {
                        call_id,
                        result_json: value.to_string(),
                    },
                    Err(e) => {
                        let r = e.report();
                        ClientMsg::ToolError {
                            call_id,
                            error: WireError {
                                kind: r.kind,
                                message: r.message,
                                remediation: r.remediation,
                            },
                        }
                    }
                };
                let out = serde_json::to_string(&reply)
                    .map_err(|e| CoreError::Message(format!("serialize reply: {e}")))?;
                tx.send(WsMessage::Text(out))
                    .await
                    .map_err(|e| CoreError::Provider(format!("send reply failed: {e}")))?;
            }
            _ => {}
        }
    }
    tracing::info!("fleetyd disconnected");
    Ok(())
}
