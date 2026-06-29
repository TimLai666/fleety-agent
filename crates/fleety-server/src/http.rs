//! axum front for the server's single listen port.
//!
//! All client transports enter here. The WebSocket route (`GET /` with an
//! Upgrade) is the primary path; the SSE+POST fallback routes are added by the
//! sse-transport-fallback change. Every route ultimately drives the same
//! transport-agnostic [`run_connection`] core in [`crate::conn`] — only the
//! inbound ([`ClientInbound`]) and outbound ([`FrameWriter`]) adapters differ.

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{CoreError, ModelProvider, Policy, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use fleety_protocol::ClientMsg;

use crate::auth::AuthStore;
use crate::bridge::{DeviceTools, Handles, Hub, Pending};
use crate::conn::{run_connection, ClientInbound, FrameWriter};
use crate::storage::Storage;

/// Shared dependencies handed to every connection. Cheap to clone (Arcs + Copy).
#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<Storage>,
    pub provider: Arc<dyn ModelProvider>,
    pub workspace: Arc<PathBuf>,
    pub policy: Policy,
    pub hub: Hub,
    pub pending: Pending,
    pub handles: Handles,
    pub auth: Arc<AuthStore>,
    pub device_tools: DeviceTools,
}

/// Build the router. WebSocket upgrades at `/` (clients connect to `ws://host`,
/// path `/`); SSE+POST fallback routes are added alongside this one.
pub fn router(state: AppState) -> Router {
    Router::new().route("/", get(ws_handler)).with_state(state)
}

/// Upgrade a `GET /` request to WebSocket and drive the connection.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| serve_ws(socket, state))
}

async fn serve_ws(socket: WebSocket, state: AppState) {
    let (sink, stream) = socket.split();
    let inbound: Box<dyn ClientInbound> = Box::new(AxumWsInbound { stream });
    let writer: Box<dyn FrameWriter> = Box::new(AxumWsFrameWriter { sink });
    if let Err(e) = run_connection(
        inbound,
        writer,
        state.storage,
        state.provider,
        state.workspace,
        state.policy,
        state.hub,
        state.pending,
        state.handles,
        state.auth,
        state.device_tools,
    )
    .await
    {
        tracing::warn!(report = ?e.report(), "websocket connection error");
    }
}

/// Inbound adapter over an axum WebSocket stream.
struct AxumWsInbound {
    stream: SplitStream<WebSocket>,
}

#[async_trait::async_trait]
impl ClientInbound for AxumWsInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        while let Some(frame) = self.stream.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let msg = serde_json::from_str(text.as_str()).map_err(|e| {
                        CoreError::Provider(format!(
                            "malformed client frame: {e}; expected a ClientMsg JSON object"
                        ))
                    })?;
                    return Ok(Some(msg));
                }
                Ok(Message::Close(_)) => return Ok(None),
                // A read error (reset/abort/EOF) is a normal end of connection.
                Err(_) => return Ok(None),
                // Ignore ping/pong/binary frames.
                Ok(_) => continue,
            }
        }
        Ok(None)
    }
}

/// Outbound adapter over an axum WebSocket sink.
struct AxumWsFrameWriter {
    sink: SplitSink<WebSocket, Message>,
}

#[async_trait::async_trait]
impl FrameWriter for AxumWsFrameWriter {
    async fn send_text(&mut self, text: String) -> bool {
        self.sink.send(Message::Text(text.into())).await.is_ok()
    }
}
