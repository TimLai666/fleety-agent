//! fleetyd — the Fleety device background service.
//!
//! Connects to the Agent on startup (registering this device) and holds the
//! connection open. Heartbeat, reconnect/backoff, autostart, and on-device tool
//! execution are later milestones.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use agent_core::{obs, CoreError, Result};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use fleety_protocol::{ClientMsg, ServerMsg, PROTOCOL_VERSION};

#[tokio::main]
async fn main() {
    obs::init();
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
    })
    .map_err(|e| CoreError::Message(format!("serialize hello: {e}")))?;
    tx.send(WsMessage::Text(hello))
        .await
        .map_err(|e| CoreError::Provider(format!("send hello failed: {e}")))?;

    tracing::info!(%url, "connected; holding connection");
    while let Some(frame) = rx.next().await {
        let frame =
            frame.map_err(|e| CoreError::Provider(format!("websocket read failed: {e}")))?;
        if frame.is_text() {
            if let Ok(text) = frame.to_text() {
                if let Ok(ServerMsg::Welcome { session_id, .. }) = serde_json::from_str(text) {
                    tracing::info!(%session_id, "registered with agent");
                }
            }
        } else if frame.is_close() {
            break;
        }
    }
    tracing::info!("fleetyd disconnected");
    Ok(())
}
