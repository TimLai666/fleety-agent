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
    reconstruct_messages, run_turn, run_turn_streaming, ApprovalDecision, ApprovalGate, AutoDeny,
    CoreError, LoopConfig, Message, ModelProvider, Policy, Result, RiskLevel, Role, ToolRegistry,
};
use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

use crate::auth::{self, AuthStore};
use crate::bridge::{self, DeviceTools, Handles, Hub, Pending};
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
    auth: Arc<AuthStore>,
    device_tools: DeviceTools,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| CoreError::Provider(format!("websocket handshake failed: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    // The first frame must be Hello; enforce auth if the server requires it.
    let (device_id, minted_token) = match read_client(&mut rx).await? {
        Some(ClientMsg::Hello {
            device_id,
            protocol,
            token,
            pairing_code,
            local_tools_json,
        }) => {
            if protocol != PROTOCOL_VERSION {
                tracing::warn!(
                    client_protocol = protocol,
                    server_protocol = PROTOCOL_VERSION,
                    "protocol version mismatch; proceeding (only v0 exists)"
                );
            }
            match authenticate(&auth, &device_id, token, pairing_code) {
                Ok(minted) => {
                    // Stash any tool specs the device advertised so device_show
                    // and downstream lookups can see them. Best-effort: a parse
                    // failure means the device speaks a future shape we don't
                    // recognise — log and proceed without specs.
                    if let Some(json) = local_tools_json {
                        match serde_json::from_str::<Vec<agent_core::ToolSpec>>(&json) {
                            Ok(specs) => {
                                device_tools.lock().await.insert(device_id.clone(), specs);
                            }
                            Err(e) => {
                                tracing::warn!(%device_id, error = %e, "could not parse advertised tools");
                            }
                        }
                    }
                    (device_id, minted)
                }
                Err(message) => {
                    tracing::warn!(%device_id, "rejected unauthenticated connection");
                    send_error(
                        &mut tx,
                        "unauthenticated",
                        &message,
                        "pass a valid token, or a pairing_code from `pair_create` on a paired device",
                    )
                    .await?;
                    return Ok(());
                }
            }
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
        &auth,
        &device_tools,
        minted_token,
        &device_id,
    )
    .await;

    hub.lock().await.remove(&device_id);
    device_tools.lock().await.remove(&device_id);
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
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    workspace: &Path,
    policy: Policy,
    hub: &Hub,
    pending: &Pending,
    handles: &Handles,
    auth: &Arc<AuthStore>,
    device_tools: &DeviceTools,
    minted_token: Option<String>,
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
            token: minted_token,
        },
    )?;

    let tools = build_full_registry(
        storage,
        workspace,
        device_id,
        hub,
        pending,
        handles,
        auth,
        device_tools,
    );

    while let Some(msg) = read_client(rx).await? {
        match msg {
            ClientMsg::UserMessage {
                conversation_id,
                text,
                origin,
                attachments,
            } => {
                let conversation = conversation_id.unwrap_or_else(|| default_conversation.clone());
                tracing::info!(
                    %device_id,
                    conversation = %conversation,
                    attachments = attachments.len(),
                    ?origin,
                    "user message"
                );

                // First finish any turn left interrupted by a crash/redeploy, so
                // it isn't lost and doesn't interleave with this message. Best
                // effort: on failure the journal stays for a later retry.
                if let Err(e) = recover_incomplete_turn(
                    rx,
                    out,
                    storage,
                    provider,
                    &tools,
                    policy,
                    device_id,
                    &conversation,
                )
                .await
                {
                    tracing::warn!(%device_id, conversation = %conversation, report = ?e.report(), "could not recover interrupted turn");
                }

                // Persist the user message and open a durable turn journal, then
                // run the turn over the history. Wire attachments map 1:1 onto
                // agent-core's Attachment so the model sees them directly.
                let attachments: Vec<agent_core::Attachment> = attachments
                    .into_iter()
                    .map(|a| agent_core::Attachment {
                        mime: a.mime,
                        bytes_b64: a.bytes_b64,
                        url: a.url,
                        name: a.name,
                    })
                    .collect();
                let user_msg = if attachments.is_empty() {
                    Message::user(text)
                } else {
                    Message::user_with_attachments(text, attachments)
                };
                storage.append(device_id, &conversation, &user_msg)?;
                storage.journal_begin(device_id, &conversation, &user_msg)?;
                // Inject agent-level core memory (ME/USER/TODO) as the system
                // preamble each turn; it is ephemeral, not persisted to the convo.
                let mut messages = vec![Message::system(storage.core_memory()?)];
                messages.extend(storage.load(device_id, &conversation)?);
                // Journal every loop event the instant it happens, so a crash
                // mid-turn is recoverable.
                let mut events = storage.journaling_log(device_id, &conversation);
                // Stream content chunks to the client for token-by-token display;
                // the full reply still arrives as `Assistant` below.
                let delta_out = out.clone();
                let delta_conv = conversation.clone();
                let mut on_delta: Box<dyn FnMut(&str) + Send> = Box::new(move |chunk: &str| {
                    let frame = ServerMsg::AssistantDelta {
                        conversation_id: delta_conv.clone(),
                        chunk: chunk.to_string(),
                    };
                    if let Ok(json) = serde_json::to_string(&frame) {
                        let _ = delta_out.send(WsMessage::Text(json));
                    }
                });
                let outcome = {
                    let mut gate = ConnGate {
                        out: out.clone(),
                        rx,
                    };
                    run_turn_streaming(
                        provider,
                        &tools,
                        &mut messages,
                        &mut events,
                        &LoopConfig::default(),
                        policy,
                        &mut gate,
                        on_delta.as_mut(),
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
                // Turn done: clear the journal (the reply now lives in the stream).
                storage.journal_end(device_id, &conversation)?;
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
                // Finish any interrupted turn before replaying, so the catch-up
                // includes its result.
                if let Err(e) = recover_incomplete_turn(
                    rx,
                    out,
                    storage,
                    provider,
                    &tools,
                    policy,
                    device_id,
                    &conversation_id,
                )
                .await
                {
                    tracing::warn!(%device_id, conversation = %conversation_id, report = ?e.report(), "could not recover interrupted turn");
                }
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
            ClientMsg::AuditList {
                device_id: target,
                since,
                limit,
            } => {
                let reply = match storage.list_audit(&target, since, limit) {
                    Ok(entries) => ServerMsg::AuditListResult {
                        device_id: target,
                        entries_json: serde_json::to_string(&entries).unwrap_or("[]".into()),
                    },
                    Err(e) => ServerMsg::Error {
                        error: WireError {
                            kind: e.report().kind,
                            message: e.report().message,
                            remediation: e.report().remediation,
                        },
                    },
                };
                emit(out, &reply)?;
            }
            ClientMsg::AuditShow {
                device_id: target,
                index,
            } => {
                let reply = match storage.read_audit(&target, index) {
                    Ok(event) => ServerMsg::AuditShowResult {
                        device_id: target,
                        index,
                        event_json: serde_json::to_string(&event).unwrap_or("null".into()),
                    },
                    Err(e) => ServerMsg::Error {
                        error: WireError {
                            kind: e.report().kind,
                            message: e.report().message,
                            remediation: e.report().remediation,
                        },
                    },
                };
                emit(out, &reply)?;
            }
            ClientMsg::RollbackList { device_id: target } => {
                let reply = match fleety_tools::list_backups(&storage.backups_dir()) {
                    Ok(backups) => ServerMsg::RollbackListResult {
                        device_id: target,
                        backups_json: serde_json::to_string(&backups).unwrap_or("[]".into()),
                    },
                    Err(e) => ServerMsg::Error {
                        error: WireError {
                            kind: e.report().kind,
                            message: e.report().message,
                            remediation: e.report().remediation,
                        },
                    },
                };
                emit(out, &reply)?;
            }
            ClientMsg::RollbackApply {
                device_id: target,
                backup_id,
            } => {
                let reply =
                    match fleety_tools::apply_backup(workspace, &storage.backups_dir(), &backup_id)
                    {
                        Ok(result) => {
                            let restored = result
                                .get("restored")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            // The rollback itself is auditable: record it.
                            let event = agent_core::Event::ToolResult {
                                id: format!("rollback-{}", backup_id),
                                result: result.clone(),
                            };
                            let _ = storage.append_history(&target, &event);
                            ServerMsg::RollbackResult {
                                device_id: target,
                                backup_id,
                                ok: true,
                                message: format!("restored {restored}"),
                            }
                        }
                        Err(e) => ServerMsg::RollbackResult {
                            device_id: target,
                            backup_id,
                            ok: false,
                            message: e.report().message,
                        },
                    };
                emit(out, &reply)?;
            }
            ClientMsg::ServerStatus => {
                let uptime_secs = crate::server_start().elapsed().as_secs();
                let device_ids: Vec<String> = hub.lock().await.keys().cloned().collect();
                let connected = device_ids.len() as u32;
                // Sidecar health: report each known sidecar as "ok" if its
                // binary is resolvable, "missing" otherwise. Best-effort — a
                // false negative just makes the status look worse than it is.
                let insyra_path = crate::sidecar::resolve_insyra();
                let sidecars = serde_json::json!({
                    "insyra": match &insyra_path {
                        Some(p) => serde_json::json!({
                            "status": "ok",
                            "path": p.to_string_lossy(),
                        }),
                        None => serde_json::json!({ "status": "missing" }),
                    }
                });
                let extra = serde_json::json!({ "sidecars": sidecars });
                let reply = ServerMsg::ServerStatusResult {
                    version: agent_core::VERSION.to_string(),
                    uptime_secs,
                    connected_devices: connected,
                    device_ids_json: serde_json::to_string(&device_ids).unwrap_or("[]".into()),
                    extra_json: Some(extra.to_string()),
                };
                emit(out, &reply)?;
            }
        }
    }

    Ok(())
}

/// Authenticate a Hello. `Ok(Some(token))` = pairing minted a token to return;
/// `Ok(None)` = already authenticated or auth disabled; `Err(msg)` = rejected.
fn authenticate(
    auth: &AuthStore,
    device_id: &str,
    token: Option<String>,
    pairing_code: Option<String>,
) -> std::result::Result<Option<String>, String> {
    if !auth.required() {
        return Ok(None);
    }
    if let Some(code) = pairing_code {
        return auth
            .redeem(&code, device_id)
            .map(Some)
            .map_err(|e| e.report().message);
    }
    if let Some(tok) = token {
        if auth.verify(&tok).is_some() {
            return Ok(None);
        }
        return Err("invalid token".to_string());
    }
    Err("this server requires authentication".to_string())
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

/// Build the full interactive tool registry for `device_id`: workspace tools
/// (+ insyra), memory/history/device/sites/schedules, skills, web, mcp, wiki,
/// ssh, browser, auth, and the cross-device bridge. Shared by live connections
/// and startup recovery so a recovered turn has the same capabilities.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_full_registry(
    storage: &Storage,
    workspace: &Path,
    device_id: &str,
    hub: &Hub,
    pending: &Pending,
    handles: &Handles,
    auth: &Arc<AuthStore>,
    device_tools: &DeviceTools,
) -> ToolRegistry {
    let mut tools = crate::tools::build_registry(
        workspace,
        &storage.backups_dir(),
        &storage.memory_dir(),
        &storage.history_path(device_id),
        &storage.devices_dir(),
        &storage.schedules_dir(),
        Arc::clone(device_tools),
    );
    crate::skills::register(
        &mut tools,
        &storage.skills_builtin_dir(),
        &storage.skills_installed_dir(),
    );
    crate::web::register(&mut tools, &storage.cookies_dir(), workspace);
    crate::mcp::register(
        &mut tools,
        &storage.mcp_builtin_config_path(),
        &storage.mcp_installed_config_path(),
    );
    crate::wiki::register(&mut tools, &storage.wiki_dir());
    crate::ssh::register(&mut tools);
    crate::browser::register(&mut tools);
    crate::sites::register(&mut tools, &storage.sites_dir(), &storage.devices_dir());
    auth::register(&mut tools, Arc::clone(auth));
    bridge::register(
        &mut tools,
        Arc::clone(hub),
        Arc::clone(pending),
        Arc::clone(handles),
        Arc::clone(device_tools),
    );
    tools
}

/// At startup, finish interactive turns left interrupted by a crash/redeploy so
/// they don't wait for the user to reconnect. Scheduler turns are skipped (the
/// scheduler tick recovers those). There is no client to stream to or ask for
/// approval: the reply is persisted to the conversation stream and the journal
/// cleared, so the next reconnect's `Resume` delivers it; gated tools in the
/// continuation are denied (`AutoDeny`) — under full access the turn just
/// completes. Each turn is isolated; a failure leaves its journal for a retry.
///
/// (Best-effort: a client reconnecting to the *same* conversation during this
/// pass could in principle also drive it; the window is tiny and the interrupted
/// tool is never re-run. A per-conversation lock would close it fully.)
#[allow(clippy::too_many_arguments)]
pub async fn recover_all_interactive(
    storage: Arc<Storage>,
    provider: Arc<dyn ModelProvider>,
    workspace: Arc<PathBuf>,
    policy: Policy,
    hub: Hub,
    pending: Pending,
    handles: Handles,
    auth: Arc<AuthStore>,
    device_tools: DeviceTools,
) {
    let incomplete = match storage.list_incomplete_turns() {
        Ok(turns) => turns,
        Err(e) => {
            tracing::warn!(report = ?e.report(), "cannot scan for interrupted turns");
            return;
        }
    };
    for (device_id, conversation) in incomplete {
        if device_id == crate::scheduler::SCHED_DEVICE {
            continue; // recovered by the scheduler tick, with its mandate
        }
        if let Err(e) = recover_one_interactive(
            &storage,
            provider.as_ref(),
            &workspace,
            policy,
            &hub,
            &pending,
            &handles,
            &auth,
            &device_tools,
            &device_id,
            &conversation,
        )
        .await
        {
            tracing::warn!(%device_id, %conversation, report = ?e.report(), "could not recover interactive turn at startup");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn recover_one_interactive(
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    workspace: &Path,
    policy: Policy,
    hub: &Hub,
    pending: &Pending,
    handles: &Handles,
    auth: &Arc<AuthStore>,
    device_tools: &DeviceTools,
    device_id: &str,
    conversation: &str,
) -> Result<()> {
    let events = storage.journal_events(device_id, conversation)?;
    if events.is_empty() {
        storage.journal_end(device_id, conversation)?;
        return Ok(());
    }
    tracing::info!(%device_id, %conversation, events = events.len(), "recovering interrupted interactive turn at startup");
    let tools = build_full_registry(
        storage,
        workspace,
        device_id,
        hub,
        pending,
        handles,
        auth,
        device_tools,
    );
    let config = LoopConfig::default();
    let mut messages = vec![Message::system(storage.core_memory()?)];
    messages.extend(storage.load(device_id, conversation)?);
    messages.extend(reconstruct_messages(&events, config.max_tool_result_chars));
    let mut log = storage.journaling_log(device_id, conversation);
    let mut gate = AutoDeny;
    let outcome = run_turn(
        provider,
        &tools,
        &mut messages,
        &mut log,
        &config,
        policy,
        &mut gate,
    )
    .await?;
    for event in log.events() {
        storage.append_history(device_id, event)?;
    }
    storage.append(device_id, conversation, &Message::assistant(outcome.output))?;
    storage.journal_end(device_id, conversation)?;
    Ok(())
}

/// Finish a turn left interrupted by a crash/redeploy: reconstruct its messages
/// from the journal (the in-flight tool is flagged interrupted, never re-run),
/// continue the loop to a final answer, persist it, and clear the journal. A
/// no-op when there is no journal. The continuation is itself journaled, so a
/// second crash is still recoverable.
#[allow(clippy::too_many_arguments)]
async fn recover_incomplete_turn(
    rx: &mut Rx,
    out: &Out,
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    policy: Policy,
    device_id: &str,
    conversation: &str,
) -> Result<()> {
    let events = storage.journal_events(device_id, conversation)?;
    if events.is_empty() {
        // No journal, or an empty/stale one — nothing to finish.
        storage.journal_end(device_id, conversation)?;
        return Ok(());
    }
    tracing::info!(%device_id, %conversation, events = events.len(), "recovering interrupted turn");

    let config = LoopConfig::default();
    let mut messages = vec![Message::system(storage.core_memory()?)];
    messages.extend(storage.load(device_id, conversation)?);
    messages.extend(reconstruct_messages(&events, config.max_tool_result_chars));

    let delta_out = out.clone();
    let delta_conv = conversation.to_string();
    let mut on_delta: Box<dyn FnMut(&str) + Send> = Box::new(move |chunk: &str| {
        let frame = ServerMsg::AssistantDelta {
            conversation_id: delta_conv.clone(),
            chunk: chunk.to_string(),
        };
        if let Ok(json) = serde_json::to_string(&frame) {
            let _ = delta_out.send(WsMessage::Text(json));
        }
    });

    let mut log = storage.journaling_log(device_id, conversation);
    let outcome = {
        let mut gate = ConnGate {
            out: out.clone(),
            rx,
        };
        run_turn_streaming(
            provider,
            tools,
            &mut messages,
            &mut log,
            &config,
            policy,
            &mut gate,
            on_delta.as_mut(),
        )
        .await?
    };

    for event in log.events() {
        storage.append_history(device_id, event)?;
    }
    let reply = outcome.output;
    let seq = storage.append(device_id, conversation, &Message::assistant(reply.clone()))?;
    storage.journal_end(device_id, conversation)?;
    emit(
        out,
        &ServerMsg::Assistant {
            conversation_id: conversation.to_string(),
            text: reply,
            seq,
        },
    )?;
    emit(
        out,
        &ServerMsg::Done {
            conversation_id: conversation.to_string(),
        },
    )?;
    Ok(())
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
    use agent_core::{Event, MockProvider, ModelResponse, Role as CoreRole, ToolCall};
    use tokio_tungstenite::MaybeTlsStream;

    type ClientWs = WebSocketStream<MaybeTlsStream<TcpStream>>;

    #[tokio::test]
    async fn startup_recovers_interrupted_interactive_turn() {
        use crate::auth::AuthStore;
        use crate::echo::EchoProvider;
        use serde_json::json;

        let home = std::env::temp_dir().join(format!("fleety-erec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(home.clone());

        // Seed a non-scheduler conversation interrupted mid-turn (no tool result).
        let user = Message::user("status");
        storage.append("dev", "c1", &user).expect("append");
        storage.journal_begin("dev", "c1", &user).expect("begin");
        let mut assistant = Message::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "t1".into(),
            name: "run_command".into(),
            arguments: json!({ "command": "echo hi" }),
        }];
        storage
            .journal_event("dev", "c1", &Event::Assistant(assistant))
            .expect("ev1");
        storage
            .journal_event(
                "dev",
                "c1",
                &Event::ToolCall(ToolCall {
                    id: "t1".into(),
                    name: "run_command".into(),
                    arguments: json!({ "command": "echo hi" }),
                }),
            )
            .expect("ev2");

        // Startup recovery, no client connected.
        let auth = Arc::new(AuthStore::load(storage.auth_path(), None, false));
        recover_all_interactive(
            Arc::clone(&storage),
            Arc::new(EchoProvider),
            workspace,
            Policy::FullAccess,
            bridge::new_hub(),
            bridge::new_pending(),
            bridge::new_handles(),
            auth,
            bridge::new_device_tools(),
        )
        .await;

        // The interrupted turn is finished (journal cleared, reply persisted)
        // without re-running the interrupted run_command.
        assert!(storage.list_incomplete_turns().expect("list").is_empty());
        let msgs = storage.load("dev", "c1").expect("load");
        assert!(msgs.iter().any(|m| m.role == CoreRole::Assistant));

        let _ = std::fs::remove_dir_all(&home);
    }

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

    fn open_auth() -> Arc<AuthStore> {
        let path = std::env::temp_dir()
            .join(format!("fleety-auth-{}", uuid::Uuid::new_v4()))
            .join("auth.json");
        Arc::new(AuthStore::load(path, None, false))
    }

    fn hello(device_id: &str) -> ClientMsg {
        ClientMsg::Hello {
            device_id: device_id.to_string(),
            protocol: PROTOCOL_VERSION,
            token: None,
            pairing_code: None,
            local_tools_json: None,
        }
    }

    /// Full enrollment round-trip: server requires auth, fleetyd-like device
    /// arrives with a pairing code, receives a token, reconnects with the
    /// token, then is rejected if it tries again with garbage.
    #[tokio::test]
    async fn enrollment_pair_then_token_then_reject_bad() {
        use crate::auth::AuthStore;
        use crate::echo::EchoProvider;

        let home = std::env::temp_dir().join(format!("fleety-enroll-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(home.clone());
        let auth = Arc::new(AuthStore::load(storage.auth_path(), None, true));

        // The agent would have called `pair_create` to mint this; we do it
        // directly to keep the test focused on the connect-side flow.
        let pairing_code = auth.create_pairing().expect("mint code");

        // Spawn a server that loops accepting connections (so we can do three
        // separate connects against the same auth store).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_storage = Arc::clone(&storage);
        let server_workspace = Arc::clone(&workspace);
        let server_auth = Arc::clone(&auth);
        let server_handle = tokio::spawn(async move {
            for _ in 0..3 {
                if let Ok((stream, _)) = listener.accept().await {
                    let storage = Arc::clone(&server_storage);
                    let workspace = Arc::clone(&server_workspace);
                    let auth = Arc::clone(&server_auth);
                    let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider);
                    tokio::spawn(async move {
                        let _ = handle_conn(
                            stream,
                            storage,
                            provider,
                            workspace,
                            Policy::FullAccess,
                            bridge::new_hub(),
                            bridge::new_pending(),
                            bridge::new_handles(),
                            auth,
                            bridge::new_device_tools(),
                        )
                        .await;
                    });
                }
            }
        });

        let url = format!("ws://{addr}");

        // Connect #1: present the pairing code, expect a token back.
        let (ws, _) = tokio_tungstenite::connect_async(&url).await.expect("conn1");
        let (mut tx, mut rx) = ws.split();
        send_client(
            &mut tx,
            &ClientMsg::Hello {
                device_id: "fleetyd-new".into(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: Some(pairing_code.clone()),
                local_tools_json: None,
            },
        )
        .await;
        let token = match recv_server(&mut rx).await {
            Some(ServerMsg::Welcome { token: Some(t), .. }) => t,
            other => panic!("expected Welcome with token, got {other:?}"),
        };
        let _ = tx.close().await;

        // Connect #2: token should authenticate without needing the pairing code.
        let (ws, _) = tokio_tungstenite::connect_async(&url).await.expect("conn2");
        let (mut tx, mut rx) = ws.split();
        send_client(
            &mut tx,
            &ClientMsg::Hello {
                device_id: "fleetyd-new".into(),
                protocol: PROTOCOL_VERSION,
                token: Some(token.clone()),
                pairing_code: None,
                local_tools_json: None,
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::Welcome { .. }) => {}
            other => panic!("expected Welcome on token reconnect, got {other:?}"),
        }
        let _ = tx.close().await;

        // Connect #3: garbage token must be rejected with an actionable error.
        let (ws, _) = tokio_tungstenite::connect_async(&url).await.expect("conn3");
        let (mut tx, mut rx) = ws.split();
        send_client(
            &mut tx,
            &ClientMsg::Hello {
                device_id: "fleetyd-new".into(),
                protocol: PROTOCOL_VERSION,
                token: Some("garbage-token-xxx".into()),
                pairing_code: None,
                local_tools_json: None,
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::Error { error }) => {
                assert_eq!(error.kind, "unauthenticated", "wrong error kind");
            }
            other => panic!("expected unauthenticated error, got {other:?}"),
        }
        let _ = tx.close().await;

        server_handle.abort();
        let _ = std::fs::remove_dir_all(&home);
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
                    attachments: Vec::new(),
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
                    open_auth(),
                    bridge::new_device_tools(),
                )
                .await;
            }
        });

        let url = format!("ws://{addr}");
        let (client, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let (mut ctx, mut crx) = client.split();

        send_client(&mut ctx, &hello("d")).await;
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
                attachments: Vec::new(),
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
                    attachments: Vec::new(),
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
        let auth = open_auth();

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
            let auth = Arc::clone(&auth);
            tokio::spawn(async move {
                for _ in 0..2 {
                    if let Ok((stream, _)) = listener.accept().await {
                        let (s, p, w, h, pe, hd, a) = (
                            Arc::clone(&storage),
                            Arc::clone(&provider),
                            Arc::clone(&workspace),
                            Arc::clone(&hub),
                            Arc::clone(&pending),
                            Arc::clone(&handles),
                            Arc::clone(&auth),
                        );
                        tokio::spawn(async move {
                            let dt = bridge::new_device_tools();
                            let _ =
                                handle_conn(stream, s, p, w, Policy::FullAccess, h, pe, hd, a, dt)
                                    .await;
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
        send_client(&mut dtx, &hello("pi")).await;
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
        send_client(&mut utx, &hello("user")).await;
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
                attachments: Vec::new(),
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

    #[tokio::test]
    async fn auth_required_rejects_then_pairs() {
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![]));
        let home = std::env::temp_dir().join(format!("fleety-authws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(home.clone());
        let hub = bridge::new_hub();
        let pending = bridge::new_pending();
        let handles = bridge::new_handles();
        let auth = Arc::new(AuthStore::load(
            home.join("auth.json"),
            Some("admintok".to_string()),
            true,
        ));
        let code = auth.create_pairing().expect("code");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        {
            let (storage, provider, workspace, hub, pending, handles, auth) = (
                Arc::clone(&storage),
                Arc::clone(&provider),
                Arc::clone(&workspace),
                Arc::clone(&hub),
                Arc::clone(&pending),
                Arc::clone(&handles),
                Arc::clone(&auth),
            );
            tokio::spawn(async move {
                for _ in 0..3 {
                    if let Ok((stream, _)) = listener.accept().await {
                        let (s, p, w, h, pe, hd, a) = (
                            Arc::clone(&storage),
                            Arc::clone(&provider),
                            Arc::clone(&workspace),
                            Arc::clone(&hub),
                            Arc::clone(&pending),
                            Arc::clone(&handles),
                            Arc::clone(&auth),
                        );
                        tokio::spawn(async move {
                            let dt = bridge::new_device_tools();
                            let _ =
                                handle_conn(stream, s, p, w, Policy::FullAccess, h, pe, hd, a, dt)
                                    .await;
                        });
                    }
                }
            });
        }
        let url = format!("ws://{addr}");

        // 1. No credentials -> rejected.
        let (c1, _) = tokio_tungstenite::connect_async(&url).await.expect("c1");
        let (mut t1, mut r1) = c1.split();
        send_client(&mut t1, &hello("d")).await;
        assert!(matches!(
            recv_server(&mut r1).await,
            Some(ServerMsg::Error { .. })
        ));

        // 2. Bootstrap token -> accepted.
        let (c2, _) = tokio_tungstenite::connect_async(&url).await.expect("c2");
        let (mut t2, mut r2) = c2.split();
        send_client(
            &mut t2,
            &ClientMsg::Hello {
                device_id: "d".into(),
                protocol: PROTOCOL_VERSION,
                token: Some("admintok".into()),
                pairing_code: None,
                local_tools_json: None,
            },
        )
        .await;
        assert!(matches!(
            recv_server(&mut r2).await,
            Some(ServerMsg::Welcome { .. })
        ));

        // 3. Pairing code -> Welcome mints a token that then validates.
        let (c3, _) = tokio_tungstenite::connect_async(&url).await.expect("c3");
        let (mut t3, mut r3) = c3.split();
        send_client(
            &mut t3,
            &ClientMsg::Hello {
                device_id: "newdev".into(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: Some(code),
                local_tools_json: None,
            },
        )
        .await;
        let minted = match recv_server(&mut r3).await {
            Some(ServerMsg::Welcome { token: Some(t), .. }) => t,
            other => panic!("expected Welcome with token, got {other:?}"),
        };
        assert!(auth.verify(&minted).is_some());

        let _ = std::fs::remove_dir_all(&home);
    }
}
