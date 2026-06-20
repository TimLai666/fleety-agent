//! Per-connection handling: WebSocket handshake, session, and the turn loop.

use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::{Error as WsErr, Message as WsMessage};
use tokio_tungstenite::WebSocketStream;

use agent_core::{run_turn, CoreError, EventLog, LoopConfig, Message, ModelProvider, Result, Role};
use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

use crate::storage::Storage;

type Tx = SplitSink<WebSocketStream<TcpStream>, WsMessage>;
type Rx = SplitStream<WebSocketStream<TcpStream>>;

/// Handle one client connection to completion.
pub async fn handle_conn(
    stream: TcpStream,
    storage: Arc<Storage>,
    provider: Arc<dyn ModelProvider>,
    workspace: Arc<PathBuf>,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| CoreError::Provider(format!("websocket handshake failed: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    // The first frame must be Hello.
    let device_id = match read_client(&mut rx).await? {
        Some(ClientMsg::Hello { device_id, .. }) => device_id,
        Some(_) => {
            send_error(
                &mut tx,
                "expected_hello",
                "first frame must be Hello",
                "send a Hello frame with your device_id before anything else",
            )
            .await?;
            return Ok(());
        }
        None => return Ok(()),
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    let default_conversation = uuid::Uuid::new_v4().to_string();
    tracing::info!(%device_id, %session_id, "client connected");
    send(
        &mut tx,
        &ServerMsg::Welcome {
            session_id,
            conversation_id: default_conversation.clone(),
            protocol: PROTOCOL_VERSION,
        },
    )
    .await?;

    let tools = crate::tools::build_registry(&workspace, &storage.backups_dir());

    while let Some(msg) = read_client(&mut rx).await? {
        match msg {
            ClientMsg::UserMessage {
                conversation_id,
                text,
                origin,
            } => {
                let conversation = conversation_id.unwrap_or_else(|| default_conversation.clone());
                tracing::info!(%device_id, conversation = %conversation, ?origin, "user message");

                // Persist the user message, then run the turn over the history.
                storage.append(&device_id, &conversation, &Message::user(text))?;
                // Inject agent-level core memory (ME/USER/TODO) as the system
                // preamble each turn; it is ephemeral, not persisted to the convo.
                let mut messages = vec![Message::system(storage.core_memory()?)];
                messages.extend(storage.load(&device_id, &conversation)?);
                let mut events = EventLog::new();
                let outcome = run_turn(
                    provider.as_ref(),
                    &tools,
                    &mut messages,
                    &mut events,
                    &LoopConfig::default(),
                )
                .await?;

                // Audit: persist the turn's events (tool calls, results, replies).
                for event in events.events() {
                    storage.append_history(&device_id, event)?;
                }

                let reply = outcome.output;
                let seq = storage.append(
                    &device_id,
                    &conversation,
                    &Message::assistant(reply.clone()),
                )?;
                send(
                    &mut tx,
                    &ServerMsg::Assistant {
                        conversation_id: conversation.clone(),
                        text: reply,
                        seq,
                    },
                )
                .await?;
                send(
                    &mut tx,
                    &ServerMsg::Done {
                        conversation_id: conversation,
                    },
                )
                .await?;
            }
            ClientMsg::Resume {
                conversation_id,
                after_seq,
            } => {
                tracing::info!(%device_id, conversation = %conversation_id, after_seq, "resume");
                let missed = storage.load_after(&device_id, &conversation_id, after_seq)?;
                for stored in missed {
                    let role = match stored.message.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::System => "system",
                        Role::Tool => "tool",
                    };
                    send(
                        &mut tx,
                        &ServerMsg::Replay {
                            conversation_id: conversation_id.clone(),
                            seq: stored.seq,
                            role: role.to_string(),
                            content: stored.message.content.clone().unwrap_or_default(),
                        },
                    )
                    .await?;
                }
                send(&mut tx, &ServerMsg::Done { conversation_id }).await?;
            }
            ClientMsg::Hello { .. } => {
                // Ignore a duplicate Hello.
            }
        }
    }

    tracing::info!(%device_id, "client disconnected");
    Ok(())
}

async fn send(tx: &mut Tx, msg: &ServerMsg) -> Result<()> {
    let json = serde_json::to_string(msg)
        .map_err(|e| CoreError::Message(format!("serialize server frame: {e}")))?;
    tx.send(WsMessage::Text(json))
        .await
        .map_err(|e| CoreError::Provider(format!("websocket send failed: {e}")))?;
    Ok(())
}

async fn send_error(tx: &mut Tx, kind: &str, message: &str, remediation: &str) -> Result<()> {
    let error = WireError {
        kind: kind.to_string(),
        message: message.to_string(),
        remediation: Some(remediation.to_string()),
    };
    send(tx, &ServerMsg::Error { error }).await
}

/// A client going away (close frame, reset, or a disconnect-shaped IO error) is
/// a normal end of connection, not an error.
fn is_disconnect(e: &WsErr) -> bool {
    use std::io::ErrorKind;
    matches!(e, WsErr::ConnectionClosed | WsErr::AlreadyClosed)
        || matches!(
            e,
            WsErr::Protocol(ProtocolError::ResetWithoutClosingHandshake)
        )
        || matches!(e, WsErr::Io(io) if matches!(
            io.kind(),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof
        ))
}

async fn read_client(rx: &mut Rx) -> Result<Option<ClientMsg>> {
    while let Some(frame) = rx.next().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(e) if is_disconnect(&e) => return Ok(None),
            Err(e) => return Err(CoreError::Provider(format!("websocket read failed: {e}"))),
        };
        if frame.is_text() {
            let text = frame
                .to_text()
                .map_err(|e| CoreError::Provider(format!("non-utf8 text frame: {e}")))?;
            let msg = serde_json::from_str(text).map_err(|e| {
                CoreError::Provider(format!(
                    "malformed client frame: {e}; expected a ClientMsg JSON object"
                ))
            })?;
            return Ok(Some(msg));
        } else if frame.is_close() {
            return Ok(None);
        }
        // Ignore ping/pong/binary frames.
    }
    Ok(None)
}
