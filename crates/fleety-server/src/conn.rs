//! Per-connection handling: WebSocket handshake, session, and the turn loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::{Error as WsErr, Message as WsMessage};
use tokio_tungstenite::WebSocketStream;

use tokio::sync::mpsc;

use agent_core::{
    run_turn, ApprovalDecision, ApprovalGate, CoreError, EventLog, LoopConfig, Message,
    ModelProvider, Policy, Result, RiskLevel, Role,
};
use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

use crate::bridge::{self, Handles, Hub, Pending};
use crate::storage::Storage;

/// Outbound frame sender (drained by the connection's writer task).
type Out = mpsc::UnboundedSender<WsMessage>;

type Tx = SplitSink<WebSocketStream<TcpStream>, WsMessage>;
type Rx = SplitStream<WebSocketStream<TcpStream>>;

/// Handle one client connection to completion.
#[allow(clippy::too_many_arguments)]
pub async fn handle_conn(
    stream: TcpStream,
    storage: Arc<Storage>,
    provider: Arc<dyn ModelProvider>,
    workspace: Arc<PathBuf>,
    policy: Policy,
    hub: Hub,
    pending: Pending,
    handles: Handles,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| CoreError::Provider(format!("websocket handshake failed: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    // The first frame must be Hello.
    let device_id = match read_client(&mut rx).await? {
        Some(ClientMsg::Hello {
            device_id,
            protocol,
        }) => {
            if protocol != PROTOCOL_VERSION {
                tracing::warn!(
                    client_protocol = protocol,
                    server_protocol = PROTOCOL_VERSION,
                    "protocol version mismatch; proceeding (only v0 exists)"
                );
            }
            device_id
        }
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

    // Register / refresh this device in the registry.
    storage.ensure_device(&device_id, "client_session")?;

    // A single writer task owns the sink; everything else (this handler, the
    // approval gate, and other connections routing RunTool here) sends frames
    // through `out`, registered in the hub under this device_id.
    let (out, mut out_rx) = mpsc::unbounded_channel::<WsMessage>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    hub.lock().await.insert(device_id.clone(), out.clone());

    let result = serve(
        &mut rx,
        &out,
        &storage,
        provider.as_ref(),
        &workspace,
        policy,
        &hub,
        &pending,
        &handles,
        &device_id,
    )
    .await;

    hub.lock().await.remove(&device_id);
    writer.abort();
    tracing::info!(%device_id, "client disconnected");
    result
}

/// The session loop, factored out so `handle_conn` can always clean up the hub
/// entry and writer task afterward.
#[allow(clippy::too_many_arguments)]
async fn serve(
    rx: &mut Rx,
    out: &Out,
    storage: &Storage,
    provider: &dyn ModelProvider,
    workspace: &Path,
    policy: Policy,
    hub: &Hub,
    pending: &Pending,
    handles: &Handles,
    device_id: &str,
) -> Result<()> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let default_conversation = uuid::Uuid::new_v4().to_string();
    tracing::info!(%device_id, %session_id, "client connected");
    emit(
        out,
        &ServerMsg::Welcome {
            session_id,
            conversation_id: default_conversation.clone(),
            protocol: PROTOCOL_VERSION,
        },
    )?;

    let mut tools = crate::tools::build_registry(
        workspace,
        &storage.backups_dir(),
        &storage.memory_dir(),
        &storage.history_path(device_id),
        &storage.devices_dir(),
        &storage.schedules_dir(),
    );
    crate::skills::register(
        &mut tools,
        &storage.skills_builtin_dir(),
        &storage.skills_installed_dir(),
    );
    crate::web::register(&mut tools);
    crate::mcp::register(&mut tools, &storage.mcp_config_path());
    crate::wiki::register(&mut tools, &storage.wiki_dir());
    crate::ssh::register(&mut tools);
    crate::browser::register(&mut tools);
    bridge::register(
        &mut tools,
        Arc::clone(hub),
        Arc::clone(pending),
        Arc::clone(handles),
    );

    while let Some(msg) = read_client(rx).await? {
        match msg {
            ClientMsg::UserMessage {
                conversation_id,
                text,
                origin,
            } => {
                let conversation = conversation_id.unwrap_or_else(|| default_conversation.clone());
                tracing::info!(%device_id, conversation = %conversation, ?origin, "user message");

                // Persist the user message, then run the turn over the history.
                storage.append(device_id, &conversation, &Message::user(text))?;
                // Inject agent-level core memory (ME/USER/TODO) as the system
                // preamble each turn; it is ephemeral, not persisted to the convo.
                let mut messages = vec![Message::system(storage.core_memory()?)];
                messages.extend(storage.load(device_id, &conversation)?);
                let mut events = EventLog::new();
                let outcome = {
                    let mut gate = ConnGate {
                        out: out.clone(),
                        rx,
                    };
                    run_turn(
                        provider,
                        &tools,
                        &mut messages,
                        &mut events,
                        &LoopConfig::default(),
                        policy,
                        &mut gate,
                    )
                    .await?
                };

                // Audit: persist the turn's events (tool calls, results, replies).
                for event in events.events() {
                    storage.append_history(device_id, event)?;
                }

                let reply = outcome.output;
                let seq =
                    storage.append(device_id, &conversation, &Message::assistant(reply.clone()))?;
                emit(
                    out,
                    &ServerMsg::Assistant {
                        conversation_id: conversation.clone(),
                        text: reply,
                        seq,
                    },
                )?;
                emit(
                    out,
                    &ServerMsg::Done {
                        conversation_id: conversation,
                    },
                )?;
            }
            ClientMsg::Resume {
                conversation_id,
                after_seq,
            } => {
                tracing::info!(%device_id, conversation = %conversation_id, after_seq, "resume");
                let missed = storage.load_after(device_id, &conversation_id, after_seq)?;
                for stored in missed {
                    let role = match stored.message.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::System => "system",
                        Role::Tool => "tool",
                    };
                    emit(
                        out,
                        &ServerMsg::Replay {
                            conversation_id: conversation_id.clone(),
                            seq: stored.seq,
                            role: role.to_string(),
                            content: stored.message.content.clone().unwrap_or_default(),
                        },
                    )?;
                }
                emit(out, &ServerMsg::Done { conversation_id })?;
            }
            ClientMsg::ToolResult {
                call_id,
                result_json,
            } => {
                // A daemon's reply to a RunTool we dispatched: route to the waiter.
                let value = serde_json::from_str::<serde_json::Value>(&result_json)
                    .unwrap_or(serde_json::Value::Null);
                bridge::dispatch_result(pending, &call_id, Ok(value)).await;
            }
            ClientMsg::ToolError { call_id, error } => {
                bridge::dispatch_result(pending, &call_id, Err(error.message)).await;
            }
            ClientMsg::Approve { .. } | ClientMsg::Deny { .. } => {
                // Only meaningful as a reply during an approval; ignore otherwise.
            }
            ClientMsg::Hello { .. } => {
                // Ignore a duplicate Hello.
            }
        }
    }

    Ok(())
}

/// Send a frame to this connection's writer task.
fn emit(out: &Out, msg: &ServerMsg) -> Result<()> {
    let json = serde_json::to_string(msg)
        .map_err(|e| CoreError::Message(format!("serialize server frame: {e}")))?;
    out.send(WsMessage::Text(json))
        .map_err(|_| CoreError::Provider("connection writer closed".to_string()))
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

/// Approval gate that asks the connected client over the WebSocket and waits
/// for an Approve/Deny reply. Sequential within the connection, so it can read
/// the reply directly from `rx`.
struct ConnGate<'a> {
    out: Out,
    rx: &'a mut Rx,
}

#[async_trait::async_trait]
impl ApprovalGate for ConnGate<'_> {
    async fn request(
        &mut self,
        tool: &str,
        args: &serde_json::Value,
        risk: RiskLevel,
    ) -> Result<ApprovalDecision> {
        let approval_id = uuid::Uuid::new_v4().to_string();
        let summary: String = args.to_string().chars().take(300).collect();
        emit(
            &self.out,
            &ServerMsg::ApprovalRequested {
                approval_id: approval_id.clone(),
                tool: tool.to_string(),
                summary,
                risk: format!("{risk:?}").to_lowercase(),
            },
        )?;
        loop {
            match read_client(self.rx).await? {
                Some(ClientMsg::Approve { approval_id: id }) if id == approval_id => {
                    return Ok(ApprovalDecision::Approve)
                }
                Some(ClientMsg::Deny { approval_id: id }) if id == approval_id => {
                    return Ok(ApprovalDecision::Deny)
                }
                Some(_) => continue,
                None => return Ok(ApprovalDecision::Deny),
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MockProvider, ModelResponse, Role as CoreRole, ToolCall};
    use tokio_tungstenite::MaybeTlsStream;

    type ClientWs = WebSocketStream<MaybeTlsStream<TcpStream>>;

    async fn recv_server(rx: &mut futures::stream::SplitStream<ClientWs>) -> Option<ServerMsg> {
        while let Some(Ok(frame)) = rx.next().await {
            if frame.is_text() {
                if let Ok(text) = frame.to_text() {
                    if let Ok(msg) = serde_json::from_str::<ServerMsg>(text) {
                        return Some(msg);
                    }
                }
            } else if frame.is_close() {
                return None;
            }
        }
        None
    }

    async fn send_client(
        tx: &mut futures::stream::SplitSink<ClientWs, WsMessage>,
        msg: &ClientMsg,
    ) {
        let json = serde_json::to_string(msg).expect("serialize");
        tx.send(WsMessage::Text(json)).await.expect("send");
    }

    #[tokio::test]
    async fn require_approval_denies_over_websocket() {
        // Provider asks to write a file, then (after denial) finishes.
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse {
                message: Message {
                    role: CoreRole::Assistant,
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: "c1".to_string(),
                        name: "write_file".to_string(),
                        arguments: serde_json::json!({ "path": "x.txt", "content": "hi" }),
                    }],
                    tool_call_id: None,
                },
            },
            ModelResponse {
                message: Message::assistant("done"),
            },
        ]));

        let home = std::env::temp_dir().join(format!("fleety-wsapp-{}", uuid::Uuid::new_v4()));
        let ws_root = home.join("ws");
        std::fs::create_dir_all(&ws_root).expect("mk ws");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(ws_root.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(
                    stream,
                    storage,
                    provider,
                    workspace,
                    Policy::RequireApproval,
                    bridge::new_hub(),
                    bridge::new_pending(),
                    bridge::new_handles(),
                )
                .await;
            }
        });

        let url = format!("ws://{addr}");
        let (client, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let (mut ctx, mut crx) = client.split();

        send_client(
            &mut ctx,
            &ClientMsg::Hello {
                device_id: "d".into(),
                protocol: PROTOCOL_VERSION,
            },
        )
        .await;
        assert!(matches!(
            recv_server(&mut crx).await,
            Some(ServerMsg::Welcome { .. })
        ));

        send_client(
            &mut ctx,
            &ClientMsg::UserMessage {
                conversation_id: None,
                text: "please write".into(),
                origin: Default::default(),
            },
        )
        .await;

        let mut saw_approval = false;
        let mut saw_done = false;
        for _ in 0..10 {
            match recv_server(&mut crx).await {
                Some(ServerMsg::ApprovalRequested {
                    approval_id, tool, ..
                }) => {
                    assert_eq!(tool, "write_file");
                    saw_approval = true;
                    send_client(&mut ctx, &ClientMsg::Deny { approval_id }).await;
                }
                Some(ServerMsg::Done { .. }) | None => {
                    saw_done = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(saw_approval, "server should have requested approval");
        assert!(saw_done, "turn should complete");
        assert!(
            !ws_root.join("x.txt").exists(),
            "denied write must not happen"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn device_exec_routes_to_daemon_and_returns() {
        // The user's agent calls device_exec -> server routes RunTool to the "pi"
        // daemon connection -> daemon replies -> the result returns to the agent
        // loop and is audited. Exercises the full three-party bridge.
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse {
                message: Message {
                    role: CoreRole::Assistant,
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: "c1".to_string(),
                        name: "device_exec".to_string(),
                        arguments: serde_json::json!({ "device": "pi", "tool": "read_file", "args": { "path": "x" } }),
                    }],
                    tool_call_id: None,
                },
            },
            ModelResponse {
                message: Message::assistant("done"),
            },
        ]));

        let home = std::env::temp_dir().join(format!("fleety-bridge-{}", uuid::Uuid::new_v4()));
        let ws_root = home.join("ws");
        std::fs::create_dir_all(&ws_root).expect("mk ws");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(ws_root.clone());
        let hub = bridge::new_hub();
        let pending = bridge::new_pending();
        let handles = bridge::new_handles();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        {
            let storage = Arc::clone(&storage);
            let provider = Arc::clone(&provider);
            let workspace = Arc::clone(&workspace);
            let hub = Arc::clone(&hub);
            let pending = Arc::clone(&pending);
            let handles = Arc::clone(&handles);
            tokio::spawn(async move {
                for _ in 0..2 {
                    if let Ok((stream, _)) = listener.accept().await {
                        let (s, p, w, h, pe, hd) = (
                            Arc::clone(&storage),
                            Arc::clone(&provider),
                            Arc::clone(&workspace),
                            Arc::clone(&hub),
                            Arc::clone(&pending),
                            Arc::clone(&handles),
                        );
                        tokio::spawn(async move {
                            let _ =
                                handle_conn(stream, s, p, w, Policy::FullAccess, h, pe, hd).await;
                        });
                    }
                }
            });
        }

        let url = format!("ws://{addr}");

        // Fake daemon "pi": replies to any RunTool with a marker result.
        let (dws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("daemon connect");
        let (mut dtx, mut drx) = dws.split();
        send_client(
            &mut dtx,
            &ClientMsg::Hello {
                device_id: "pi".into(),
                protocol: PROTOCOL_VERSION,
            },
        )
        .await;
        assert!(matches!(
            recv_server(&mut drx).await,
            Some(ServerMsg::Welcome { .. })
        ));
        let daemon = tokio::spawn(async move {
            while let Some(msg) = recv_server(&mut drx).await {
                if let ServerMsg::RunTool { call_id, tool, .. } = msg {
                    let reply = ClientMsg::ToolResult {
                        call_id,
                        result_json: serde_json::json!({ "device_said": tool }).to_string(),
                    };
                    send_client(&mut dtx, &reply).await;
                }
            }
        });

        // User: connect and trigger the turn that calls device_exec on "pi".
        let (uws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("user connect");
        let (mut utx, mut urx) = uws.split();
        send_client(
            &mut utx,
            &ClientMsg::Hello {
                device_id: "user".into(),
                protocol: PROTOCOL_VERSION,
            },
        )
        .await;
        assert!(matches!(
            recv_server(&mut urx).await,
            Some(ServerMsg::Welcome { .. })
        ));
        send_client(
            &mut utx,
            &ClientMsg::UserMessage {
                conversation_id: None,
                text: "run on pi".into(),
                origin: Default::default(),
            },
        )
        .await;

        let mut done = false;
        for _ in 0..10 {
            match recv_server(&mut urx).await {
                Some(ServerMsg::Done { .. }) | None => {
                    done = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(done, "user turn should complete");

        // The daemon's result was returned to the agent and audited.
        let hist = std::fs::read_to_string(storage.history_path("user")).unwrap_or_default();
        assert!(
            hist.contains("device_said"),
            "daemon result should be recorded; history was: {hist}"
        );

        daemon.abort();
        let _ = std::fs::remove_dir_all(&home);
    }
}
