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
    reconstruct_messages, run_turn, run_turn_streaming, run_turn_streaming_cached,
    ApprovalDecision, ApprovalGate, AutoDeny, CoreError, GoalState, LoopConfig, Message,
    ModelProvider, Policy, Result, RiskLevel, Role, Terminal, ToolRegistry,
};
use fleety_protocol::{AttentionHint, ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

use crate::auth::{self, AuthStore};
use crate::bridge::{self, DeviceTools, Handles, Hub, Pending};
use crate::storage::Storage;

/// Outbound frame sender (drained by the connection's writer task).
pub(crate) type Out = mpsc::UnboundedSender<WsMessage>;

type Tx = SplitSink<WebSocketStream<TcpStream>, WsMessage>;
type Rx = SplitStream<WebSocketStream<TcpStream>>;

/// The server's own hostname (for deciding whether an originating CLI is on the
/// same machine as the server). Empty if unknown — then no client is "same host".
fn server_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

/// Apply a rollover requested during a turn: set the old conversation aside with
/// the new as its successor (old stays recall-able), claim the new for the acting
/// user, and tell the client via `ConversationRolled`. Drains the shared slot, so
/// calling it twice is safe (the second is a no-op).
async fn apply_rollover(
    storage: &Arc<Storage>,
    out: &Out,
    conversation: &str,
    acting: &crate::identity::ActingUser,
    state: &crate::conversation_lifecycle::RolloverState,
) {
    let req = { state.lock().await.take() };
    let Some(req) = req else { return };
    if let Err(e) = storage.mark_conversation_ended(conversation, &req.new_id) {
        tracing::warn!(report = ?e.report(), "rollover: could not record successor");
        return;
    }
    let _ = storage.register_conversation_owner(&req.new_id, acting);
    tracing::info!(old = %conversation, new = %req.new_id, distill = req.distill, note = ?req.note, "conversation rolled over");
    let _ = emit(
        out,
        &ServerMsg::ConversationRolled {
            old: conversation.to_string(),
            new: req.new_id,
        },
    );
}

/// Per-conversation single-flight set for background housekeeping (process-wide,
/// so the same conversation never reflects twice concurrently).
fn housekeeping_inflight() -> &'static tokio::sync::Mutex<std::collections::HashSet<String>> {
    static S: std::sync::OnceLock<tokio::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    S.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Build the per-connection tool stack rooted at `root`: the full registry plus
/// the subagent orchestration, dynamic-workflow, and goal tools (registered only
/// at this top level). Returns the registry, the subagent host, and the shared
/// goal state. Rebuilt when a conversation binds to a different workspace root.
#[allow(clippy::too_many_arguments)]
fn build_connection_stack(
    storage: &Arc<Storage>,
    root: &Path,
    device_id: &str,
    hub: &Hub,
    pending: &Pending,
    handles: &Handles,
    auth: &Arc<AuthStore>,
    device_tools: &DeviceTools,
    policy: Policy,
    out: &Out,
    acting: &crate::identity::ActingUser,
    rollover_state: &crate::conversation_lifecycle::RolloverState,
    editor_specs: &[agent_core::ToolSpec],
) -> (
    agent_core::ToolRegistry,
    Arc<crate::subagent::FleetyHost>,
    Arc<tokio::sync::Mutex<GoalState>>,
) {
    let mut tools = build_full_registry(
        storage,
        root,
        device_id,
        hub,
        pending,
        handles,
        auth,
        device_tools,
    );
    // Conversation recall, scoped to the acting user (per-user history).
    crate::conversation_recall::register(
        &mut tools,
        Arc::clone(storage),
        acting.user_id().map(String::from),
    );
    // Tool-result retrieval + a privacy-filtered audit listing, scoped to the
    // acting user (overrides the unscoped base `history_list`).
    crate::tools::register_user_scoped(
        &mut tools,
        Arc::clone(storage),
        device_id,
        acting.clone(),
        agent_core::LoopConfig::default().max_tool_result_chars,
    );
    let subagent_host = crate::subagent::FleetyHost::new(
        crate::providers::ProviderTiers::from_env(),
        policy,
        Arc::clone(storage),
        root.to_path_buf(),
        device_id.to_string(),
        Arc::clone(hub),
        Arc::clone(pending),
        Arc::clone(handles),
        Arc::clone(auth),
        Arc::clone(device_tools),
        out.clone(),
        acting.clone(),
    );
    let subagent_mgr = agent_core::SubagentManager::new(
        Arc::clone(&subagent_host) as Arc<dyn agent_core::SubagentHost>,
        policy,
        crate::subagent::max_concurrent_from_env(),
    );
    subagent_host.set_manager(Arc::downgrade(&subagent_mgr));
    agent_core::register_orchestration(&mut tools, Arc::clone(&subagent_mgr));
    agent_workflow::register_workflow(&mut tools, subagent_mgr);
    let goal_state = Arc::new(tokio::sync::Mutex::new(GoalState::new()));
    agent_core::register_goal_tools(&mut tools, Arc::clone(&goal_state));
    crate::conversation_lifecycle::register(&mut tools, Arc::clone(rollover_state));
    // Editor-backed tools (ACP delegation): when the connecting editor advertised
    // fs/terminal tools, the agent gets `editor_*` tools that route to this very
    // connection (its `out` sender) so file edits go through the user's editor.
    crate::editor_tools::register_editor(&mut tools, out, Arc::clone(pending), editor_specs);
    (tools, subagent_host, goal_state)
}

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
            hostname,
        }) => {
            if protocol != PROTOCOL_VERSION {
                tracing::warn!(
                    client_protocol = protocol,
                    server_protocol = PROTOCOL_VERSION,
                    "protocol version mismatch; proceeding (only v0 exists)"
                );
            }
            match authenticate(&auth, &device_id, token.as_deref(), pairing_code.as_deref()) {
                Ok(minted) => {
                    // Resolve the authoritative device id (token-bound when
                    // authenticated) and migrate any legacy hostname/bound-id data
                    // to it once.
                    let device_id = resolve_device_identity(
                        &auth,
                        &storage,
                        &device_id,
                        token.as_deref(),
                        hostname.as_deref(),
                    );
                    // Record the hostname as a display label on the device record
                    // (ensure the record exists first; it is re-ensured below too).
                    if let Some(h) = &hostname {
                        let _ = storage.ensure_device(&device_id, "client_session");
                        let _ = storage.set_device_label(&device_id, h);
                    }
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

    // Build the tool stack rooted at the server's default workspace. A
    // conversation may rebind it to the originating CLI's cwd on its first
    // message (see the UserMessage arm), making Fleety a coding agent in that
    // directory. The orchestration/workflow/goal tools live only at this top
    // level (subagent child registries omit them, capping nesting at one level).
    let server_host = server_hostname();
    let mut current_root: std::path::PathBuf = workspace.to_path_buf();
    // Before the first message we don't yet know the acting user; default recall
    // scope to the device owner (rebuilt with the resolved user on first message).
    let connect_acting = storage.acting_for_device(device_id);
    let rollover_state = crate::conversation_lifecycle::new_state();
    // Editor-backed tools to offer this connection: the `editor_*` subset it
    // advertised in Hello (an ACP editor gates these by the editor's capabilities).
    let editor_specs: Vec<agent_core::ToolSpec> = device_tools
        .lock()
        .await
        .get(device_id)
        .map(|specs| {
            specs
                .iter()
                .filter(|s| s.name.starts_with("editor_"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let (mut tools, mut subagent_host, mut goal_state) = build_connection_stack(
        storage,
        &current_root,
        device_id,
        hub,
        pending,
        handles,
        auth,
        device_tools,
        policy,
        out,
        &connect_acting,
        &rollover_state,
        &editor_specs,
    );
    let mut workspace_bound = false;
    let goal_max_continues = goal_max_continues_from_env();
    let skill_reflect_min_steps = skill_reflect_min_steps_from_env();

    while let Some(msg) = read_client(rx).await? {
        match msg {
            ClientMsg::UserMessage {
                conversation_id,
                text,
                origin,
                attachments,
                voice,
                acting_user,
            } => {
                let conversation = conversation_id.unwrap_or_else(|| default_conversation.clone());
                // Transparent redirect: if this conversation has rolled over,
                // follow the successor chain to the active one (clients that
                // ignored ConversationRolled still land in the right place).
                let conversation = storage.active_conversation(&conversation);
                // Resolve who this turn is for: the device owner by default, an
                // asserted (and device-authorized) user otherwise, else Guest.
                let acting = {
                    let (owner, users, _shared) =
                        storage.device_ownership(device_id).unwrap_or_default();
                    crate::identity::resolve_acting_user(
                        owner.as_deref(),
                        &users,
                        acting_user.as_deref(),
                    )
                };
                tracing::info!(
                    %device_id,
                    conversation = %conversation,
                    attachments = attachments.len(),
                    ?origin,
                    "user message"
                );

                // Privacy boundary: claim a new/unowned conversation for the
                // acting user, then gate access. A conversation owned by someone
                // else (and not granted) is refused with a uniform, non-revealing
                // message that does not distinguish "no such conversation" from
                // "exists but forbidden".
                let _ = storage.register_conversation_owner(&conversation, &acting);
                if !storage
                    .conversation_access(&acting, &conversation, &storage.grants())
                    .is_allow()
                {
                    tracing::warn!(%device_id, conversation = %conversation, "cross-user conversation access denied");
                    let _ = emit(
                        out,
                        &ServerMsg::Assistant {
                            conversation_id: conversation.clone(),
                            text: "That conversation isn't available to you.".to_string(),
                            seq: 0,
                            speech: None,
                            attention: None,
                        },
                    );
                    let _ = emit(
                        out,
                        &ServerMsg::Done {
                            conversation_id: conversation.clone(),
                        },
                    );
                    continue;
                }

                // Bind this conversation's workspace once (from its first
                // message): when the originating CLI is on the server host, root
                // the tools at its cwd so Fleety acts as a coding agent in that
                // directory. Persisted and reused on later turns / resume.
                if !workspace_bound {
                    let binding = storage
                        .conversation_workspace(&conversation)
                        .unwrap_or_else(|| {
                            let b = crate::workspace::resolve_binding(
                                origin.cwd.as_deref(),
                                origin.hostname.as_deref(),
                                device_id,
                                &server_host,
                                workspace,
                            );
                            let _ = storage.set_conversation_workspace(&conversation, &b);
                            b
                        });
                    if binding.device.is_none() && binding.root != current_root {
                        tracing::info!(root = %binding.root.display(), "rooting conversation workspace at the CLI's cwd");
                        current_root = binding.root.clone();
                    }
                    // Rebuild the stack once with the resolved acting user (so
                    // recall is scoped to them) and the chosen root.
                    let (t, h, g) = build_connection_stack(
                        storage,
                        &current_root,
                        device_id,
                        hub,
                        pending,
                        handles,
                        auth,
                        device_tools,
                        policy,
                        out,
                        &acting,
                        &rollover_state,
                        &editor_specs,
                    );
                    tools = t;
                    subagent_host = h;
                    goal_state = g;
                    workspace_bound = true;
                }

                // Hold the per-connection turn lock across BOTH recovery and this
                // turn so a background subagent's wake turn can't interleave
                // storage appends; record the active conversation so a `fork`
                // subagent inherits it.
                let _turn_guard = subagent_host.lock_turn().await;
                subagent_host.set_active_conversation(&conversation).await;

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
                let mut gate = ConnGate {
                    out: out.clone(),
                    rx,
                };
                let steps = drive_to_goal(
                    out,
                    storage,
                    provider,
                    &tools,
                    policy,
                    device_id,
                    &conversation,
                    user_msg,
                    &mut gate,
                    &goal_state,
                    goal_max_continues,
                    voice,
                    &acting,
                )
                .await?;
                // Apply an explicit rollover the agent requested during the turn
                // (fast, storage-only): set the old aside, switch active, tell the
                // client. Transparent redirect handles clients that ignore it.
                apply_rollover(storage, out, &conversation, &acting, &rollover_state).await;

                // Learning loop + lifecycle distillation run as BACKGROUND
                // housekeeping — off the connection loop so the user's next
                // message is handled immediately — on the economy tier, single-
                // flight per conversation, never user-facing. A rollover the
                // reflection itself requests is applied at its end.
                if skill_reflect_min_steps > 0 && steps >= skill_reflect_min_steps {
                    let storage_bg = Arc::clone(storage);
                    let out_bg = out.clone();
                    let hub_bg = hub.clone();
                    let pending_bg = pending.clone();
                    let handles_bg = handles.clone();
                    let auth_bg = Arc::clone(auth);
                    let device_tools_bg = device_tools.clone();
                    let device_bg = device_id.to_string();
                    let conv_bg = conversation.clone();
                    let acting_bg = acting.clone();
                    let root_bg = current_root.clone();
                    let rollover_bg = Arc::clone(&rollover_state);
                    let min_steps = skill_reflect_min_steps;
                    tokio::spawn(async move {
                        // Single-flight per conversation: skip if one is running.
                        {
                            let mut inflight = housekeeping_inflight().lock().await;
                            if inflight.contains(&conv_bg) {
                                return;
                            }
                            inflight.insert(conv_bg.clone());
                        }
                        let cheap = crate::providers::ProviderTiers::from_env().resolve("cheap");
                        let mut tools = build_full_registry(
                            &storage_bg,
                            &root_bg,
                            &device_bg,
                            &hub_bg,
                            &pending_bg,
                            &handles_bg,
                            &auth_bg,
                            &device_tools_bg,
                        );
                        crate::conversation_lifecycle::register(
                            &mut tools,
                            Arc::clone(&rollover_bg),
                        );
                        let mut bg_gate = agent_core::AutoApprove;
                        if let Err(e) = maybe_reflect(
                            &out_bg,
                            &storage_bg,
                            cheap.as_ref(),
                            &tools,
                            policy,
                            &device_bg,
                            &conv_bg,
                            &mut bg_gate,
                            steps,
                            min_steps,
                        )
                        .await
                        {
                            tracing::warn!(report = ?e.report(), "background housekeeping failed (isolated)");
                        }
                        apply_rollover(&storage_bg, &out_bg, &conv_bg, &acting_bg, &rollover_bg)
                            .await;
                        housekeeping_inflight().lock().await.remove(&conv_bg);
                    });
                }
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
    token: Option<&str>,
    pairing_code: Option<&str>,
) -> std::result::Result<Option<String>, String> {
    if !auth.required() {
        return Ok(None);
    }
    if let Some(code) = pairing_code {
        return auth
            .redeem(code, device_id)
            .map(Some)
            .map_err(|e| e.report().message);
    }
    if let Some(tok) = token {
        if auth.verify(tok).is_some() {
            return Ok(None);
        }
        return Err("invalid token".to_string());
    }
    Err("this server requires authentication".to_string())
}

/// Resolve the connection's authoritative device id and migrate legacy data once.
///
/// - **Authenticated** (a token bound to a real device): the identity is the
///   token's bound id. If the client now reports a different (machine) id and that
///   id's directory is free, the device's data is migrated to it and the token
///   rebound — the one-time hostname/legacy → machine-id move. If the destination
///   already exists, we keep the bound id (never clobber/hijack another device).
/// - **Unauthenticated / pairing / admin token**: the reported (machine) id is
///   used, and any legacy directory keyed by the reported hostname is migrated to
///   it once. A spoofed id can only ever move the caller's own data, never another
///   device's (the no-clobber rule).
fn resolve_device_identity(
    auth: &AuthStore,
    storage: &Storage,
    asserted: &str,
    token: Option<&str>,
    hostname: Option<&str>,
) -> String {
    if let Some(tok) = token {
        if let Some(bound) = auth.verify(tok) {
            if bound != "admin" {
                if bound == asserted {
                    return asserted.to_string();
                }
                match storage.migrate_device(&bound, asserted) {
                    Ok(true) => {
                        let _ = auth.rebind_device(&bound, asserted);
                        return asserted.to_string();
                    }
                    // Destination exists or the move failed → keep the bound id.
                    _ => return bound,
                }
            }
        }
    }
    if let Some(h) = hostname {
        if h != asserted {
            let _ = storage.migrate_device(h, asserted);
        }
    }
    asserted.to_string()
}

/// The terminal result of one turn: the assistant reply text and its persisted
/// sequence number. Returned so a multi-turn driver can emit the user-facing
/// frames itself only on the terminal turn.
pub(crate) struct TurnReply {
    pub reply: String,
    pub seq: u64,
    /// Spoken-version text when voice mode is on and the model produced one;
    /// emitted only on the terminal turn.
    pub speech: Option<String>,
    /// Provider steps taken this turn (a complexity proxy for the learning loop).
    pub steps: usize,
    /// Device-deixis attention hint when voice is on and the model produced one;
    /// emitted only on the terminal turn.
    pub attention: Option<agent_core::AttentionHint>,
}

/// Map a core attention hint to its wire form (the two structs are identical but
/// live in different crates so agent-core stays host-free).
fn wire_attention(a: Option<agent_core::AttentionHint>) -> Option<AttentionHint> {
    a.map(|a| AttentionHint {
        device: a.device,
        look_at: a.look_at,
        url: a.url,
    })
}

/// Default cap on automatic goal continuations per user message; override with
/// `FLEETY_GOAL_MAX_CONTINUES` (floor 1).
pub const DEFAULT_GOAL_MAX_CONTINUES: u32 = 8;

/// Read the configured auto-continuation cap (floor 1).
pub fn goal_max_continues_from_env() -> u32 {
    std::env::var("FLEETY_GOAL_MAX_CONTINUES")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_GOAL_MAX_CONTINUES)
        .max(1)
}

/// Default tool-step threshold above which a completed user message triggers one
/// learning-loop reflection turn; override with `FLEETY_SKILL_REFLECT_MIN_STEPS`
/// (0 disables reflection).
pub const DEFAULT_SKILL_REFLECT_MIN_STEPS: usize = 5;

/// Read the configured reflection step threshold (0 disables; a parse failure
/// falls back to the default).
pub fn skill_reflect_min_steps_from_env() -> usize {
    std::env::var("FLEETY_SKILL_REFLECT_MIN_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_SKILL_REFLECT_MIN_STEPS)
}

/// Drive a single user message to its goal: run a turn, and while a goal is
/// active and neither terminal signal (`complete_goal`/`ask_user`) fired, inject
/// a continuation nudge and run another turn — bounded by `max_continues`. The
/// goal state is reset for this message up front. Intermediate turns are silent
/// (progress still streams as deltas); only the terminal turn emits the
/// user-facing reply + `Done` (and, when voice is on, the spoken summary — the
/// gate falls out of emitting only here). When no goal is ever set the first
/// turn is terminal — behaviour identical to a single-shot turn.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_to_goal(
    out: &Out,
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    policy: Policy,
    device_id: &str,
    conversation: &str,
    user_msg: Message,
    gate: &mut dyn agent_core::ApprovalGate,
    goal_state: &Arc<tokio::sync::Mutex<GoalState>>,
    max_continues: u32,
    voice: bool,
    acting: &crate::identity::ActingUser,
) -> Result<usize> {
    // Fresh goal state for this user message.
    *goal_state.lock().await = GoalState::new();
    let mut next_msg = user_msg;
    let mut continues: u32 = 0;
    let mut total_steps: usize = 0;
    loop {
        // Run silently; whether this turn is terminal is only known afterward.
        // Each turn runs with the message's voice flag so the terminal turn can
        // carry a spoken reply; intermediate turns' speech is discarded.
        let turn = drive_turn(
            out,
            storage,
            provider,
            tools,
            policy,
            device_id,
            conversation,
            next_msg,
            gate,
            false,
            voice,
            acting,
        )
        .await?;
        total_steps += turn.steps;
        let (active, terminal) = {
            let mut g = goal_state.lock().await;
            (g.is_active(), g.take_terminal())
        };
        let premature = active && matches!(terminal, Terminal::None);
        let hit_cap = continues >= max_continues;
        if !premature || hit_cap {
            // Terminal turn: emit the real reply (and Done). On a cap stop with an
            // unmet goal, tell the user it may be incomplete.
            let text = if premature && hit_cap {
                format!(
                    "{}\n\n[Reached the auto-continue cap of {max_continues}; the goal may be \
                     incomplete.]",
                    turn.reply
                )
            } else {
                turn.reply
            };
            emit(
                out,
                &ServerMsg::Assistant {
                    conversation_id: conversation.to_string(),
                    text,
                    seq: turn.seq,
                    speech: turn.speech,
                    attention: wire_attention(turn.attention),
                },
            )?;
            emit(
                out,
                &ServerMsg::Done {
                    conversation_id: conversation.to_string(),
                },
            )?;
            return Ok(total_steps);
        }
        // Premature stop under the cap: nudge and run another turn.
        continues += 1;
        next_msg = Message::user(goal_state.lock().await.nudge_text());
    }
}

/// Run one turn over a conversation given a seed `user_msg` and an approval
/// `gate`, streaming deltas and persisting the result. Shared by live user
/// turns (via [`drive_to_goal`]) and a subagent's proactive wake turn
/// (`AutoApprove`), so both take the identical journal / audit path. When
/// `emit_terminal` is true the user-facing `Assistant` + `Done` frames are sent
/// here; when false the turn is silent (the caller emits on the terminal turn)
/// while progress still streams as `AssistantDelta`. Returns the reply + seq.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_turn(
    out: &Out,
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    policy: Policy,
    device_id: &str,
    conversation: &str,
    user_msg: Message,
    gate: &mut dyn agent_core::ApprovalGate,
    emit_terminal: bool,
    voice: bool,
    acting: &crate::identity::ActingUser,
) -> Result<TurnReply> {
    storage.append(device_id, conversation, &user_msg)?;
    storage.journal_begin(device_id, conversation, &user_msg)?;
    // Inject agent-level core memory (ME/USER/TODO) as the system preamble each
    // turn; it is ephemeral, not persisted to the conversation.
    let mut messages = vec![Message::system(storage.system_prompt_for(acting)?)];
    // Tell the agent the current time in the acting user's timezone (profile →
    // FLEETY_TZ → UTC) so it reasons about "today"/"tonight" correctly; storage
    // stays UTC.
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let tz = crate::tz::resolve_tz(
        storage.user_timezone(acting).as_deref(),
        std::env::var("FLEETY_TZ").ok().as_deref(),
    );
    messages.push(Message::system(format!(
        "Current time: {}",
        crate::tz::format_for_user(now_secs, tz)
    )));
    messages.extend(storage.load(device_id, conversation)?);
    let mut events = storage.journaling_log(device_id, conversation);
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
    // Incremental compaction: reuse this conversation's persisted rolling summary
    // so a long conversation only summarizes new messages (not the whole middle
    // every turn/reload). A missing/stale cache safely falls back to a full
    // summary. Saved back after the turn; a save failure is non-fatal.
    let mut compaction = storage.load_compaction(device_id, conversation);
    let outcome = run_turn_streaming_cached(
        provider,
        tools,
        &mut messages,
        &mut events,
        &LoopConfig {
            voice,
            ..LoopConfig::default()
        },
        policy,
        gate,
        on_delta.as_mut(),
        &mut compaction,
    )
    .await?;
    if let Some(cache) = &compaction {
        if let Err(e) = storage.save_compaction(device_id, conversation, cache) {
            tracing::warn!(%conversation, error = %e, "could not persist compaction cache");
        }
    }
    for event in events.events() {
        storage.append_history_tagged(device_id, conversation, event)?;
    }
    let steps = outcome.steps;
    let reply = outcome.output;
    let speech = outcome.speech;
    let attention = outcome.attention;
    let seq = storage.append(device_id, conversation, &Message::assistant(reply.clone()))?;
    storage.journal_end(device_id, conversation)?;
    // Off-turn: append this user's new messages to their semantic-recall index,
    // fire-and-forget so the turn never waits on embedding. Best-effort — a
    // failure (model unavailable, etc.) is logged and the next turn retries.
    if crate::embed::enabled() {
        if let Some(user) = acting.user_id().map(str::to_string) {
            let storage = Arc::clone(storage);
            let conversation = conversation.to_string();
            tokio::task::spawn_blocking(move || {
                let path = storage.conversation_index_path(&user);
                let cache = storage.models_dir();
                let msgs: Vec<crate::conversation_embed::IndexMsg> = storage
                    .load_user_conversation(&user, &conversation)
                    .into_iter()
                    .filter_map(|e| {
                        let content = e.message.content?;
                        let role = match e.message.role {
                            Role::System => "system",
                            Role::User => "user",
                            Role::Assistant => "assistant",
                            Role::Tool => "tool",
                        }
                        .to_string();
                        Some((e.seq, e.ts_secs, role, content))
                    })
                    .collect();
                if let Err(e) =
                    crate::conversation_embed::index_messages(&path, &cache, &conversation, &msgs)
                {
                    tracing::warn!(error = %e, "conversation index update failed");
                }
            });
        }
    }
    if emit_terminal {
        emit(
            out,
            &ServerMsg::Assistant {
                conversation_id: conversation.to_string(),
                text: reply.clone(),
                seq,
                speech: speech.clone(),
                attention: wire_attention(attention.clone()),
            },
        )?;
        emit(
            out,
            &ServerMsg::Done {
                conversation_id: conversation.to_string(),
            },
        )?;
    }
    Ok(TurnReply {
        reply,
        seq,
        speech,
        steps,
        attention,
    })
}

/// Run one reflection turn after a sufficiently complex user message: prompt the
/// agent to persist a reusable procedure as an authored skill and durable facts
/// to memory/wiki. Runs at most once, only when `min_steps > 0 && steps >=
/// min_steps`; it is a plain single turn (no goal, voice off, `emit_terminal`),
/// so it never recurses into another reflection. A no-op otherwise.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_reflect(
    out: &Out,
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    policy: Policy,
    device_id: &str,
    conversation: &str,
    gate: &mut dyn agent_core::ApprovalGate,
    steps: usize,
    min_steps: usize,
) -> Result<()> {
    if min_steps == 0 || steps < min_steps {
        return Ok(());
    }
    let seed = "[housekeeping] That was a multi-step task. Quietly do any worthwhile upkeep, then \
        reply in one short line with what you did (or that there was nothing) — do NOT redo the \
        task, and never address the user as if a system prompted you. \
        (1) Reusable procedure? Save/update an authored skill (skill_write_file; helper tools in \
        scripts/ referenced from SKILL.md). \
        (2) Durable takeaways? Distil by type to the RIGHT place — lasting knowledge/insight → the \
        wiki (wisdom, not transcripts); pending work → TODO; facts about the user → USER; \
        device-operational facts → that device's NOTES; ephemeral recap → nowhere (recall already \
        keeps it). \
        (3) If a whole task or goal just finished and this thread is done, consider \
        rollover_conversation to start fresh (after distilling). \
        Skip anything one-off or already obvious from code/git.";
    drive_turn(
        out,
        storage,
        provider,
        tools,
        policy,
        device_id,
        conversation,
        Message::user(seed),
        gate,
        // Emit the one-line result (it may write skills/memory — surface that),
        // and never voice.
        true,
        false,
        &storage.acting_for_device(device_id),
    )
    .await?;
    Ok(())
}

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
        &storage.skills_authored_dir(),
        &storage.skills_installed_dir(),
    );
    crate::web::register(&mut tools, &storage.cookies_dir(), workspace);
    crate::mcp::register(
        &mut tools,
        &storage.mcp_builtin_config_path(),
        &storage.mcp_installed_config_path(),
    );
    crate::wiki::register(&mut tools, &storage.wiki_dir(), &storage.models_dir());
    crate::ssh::register(&mut tools);
    fleety_tools::register_browser(&mut tools);
    fleety_tools::register_computer(&mut tools);
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
    let mut messages = vec![Message::system(
        storage.system_prompt_for(&storage.acting_for_device(device_id))?,
    )];
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
        storage.append_history_tagged(device_id, conversation, event)?;
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
    let mut messages = vec![Message::system(
        storage.system_prompt_for(&storage.acting_for_device(device_id))?,
    )];
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
        storage.append_history_tagged(device_id, conversation, event)?;
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
            // Recovery turns are non-voice: no spoken channel, no attention hint.
            speech: None,
            attention: None,
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

    // ---- drive-to-goal loop tests --------------------------------------------

    /// A scripted assistant turn that calls one tool.
    fn call_resp(id: &str, name: &str, args: serde_json::Value) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: CoreRole::Assistant,
                content: None,
                tool_calls: vec![ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments: args,
                }],
                tool_call_id: None,
                attachments: Vec::new(),
            },
        }
    }

    /// A scripted assistant turn that just replies with text (ends the turn).
    fn text_resp(text: &str) -> ModelResponse {
        ModelResponse {
            message: Message::assistant(text),
        }
    }

    /// Storage + a registry holding only the goal tools + their shared state + a
    /// frame sink/source. Self-contained: no websocket, no env vars.
    #[allow(clippy::type_complexity)]
    fn goal_env() -> (
        Arc<Storage>,
        ToolRegistry,
        Arc<tokio::sync::Mutex<GoalState>>,
        PathBuf,
        Out,
        mpsc::UnboundedReceiver<WsMessage>,
    ) {
        let home = std::env::temp_dir().join(format!("fleety-goal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let goal_state = Arc::new(tokio::sync::Mutex::new(GoalState::new()));
        let mut tools = ToolRegistry::new();
        agent_core::register_goal_tools(&mut tools, Arc::clone(&goal_state));
        let (out, rx) = mpsc::unbounded_channel::<WsMessage>();
        (storage, tools, goal_state, home, out, rx)
    }

    /// Drain queued frames and decode the `ServerMsg`es.
    fn drain(rx: &mut mpsc::UnboundedReceiver<WsMessage>) -> Vec<ServerMsg> {
        let mut msgs = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let WsMessage::Text(t) = frame {
                if let Ok(m) = serde_json::from_str::<ServerMsg>(&t) {
                    msgs.push(m);
                }
            }
        }
        msgs
    }

    /// The terminal frames: count of `Done` and the texts of `Assistant` frames.
    fn terminal_frames(msgs: &[ServerMsg]) -> (usize, Vec<String>) {
        let dones = msgs
            .iter()
            .filter(|m| matches!(m, ServerMsg::Done { .. }))
            .count();
        let texts = msgs
            .iter()
            .filter_map(|m| match m {
                ServerMsg::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        (dones, texts)
    }

    #[tokio::test]
    async fn no_goal_message_is_single_shot() {
        // No set_goal → the first turn is terminal, exactly one reply emitted.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![text_resp("hi")]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("hello"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let (dones, texts) = terminal_frames(&drain(&mut rx));
        assert_eq!(dones, 1, "single-shot emits exactly one Done");
        assert_eq!(texts, vec!["hi".to_string()]);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn active_goal_continues_then_completes_emitting_once() {
        // Three turns: set_goal, an idle continuation, then complete_goal. Only
        // the terminal turn emits Assistant/Done.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![
            call_resp("a", "set_goal", serde_json::json!({ "goal": "finish" })),
            text_resp("working"),
            text_resp("still working"),
            call_resp(
                "b",
                "complete_goal",
                serde_json::json!({ "summary": "done" }),
            ),
            text_resp("All finished."),
        ]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("do the work"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let (dones, texts) = terminal_frames(&drain(&mut rx));
        assert_eq!(dones, 1, "only the terminal turn emits Done");
        assert_eq!(texts, vec!["All finished.".to_string()]);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn ask_user_stops_the_loop() {
        // set_goal then, after one continuation, ask_user — a terminal signal.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![
            call_resp("a", "set_goal", serde_json::json!({ "goal": "x" })),
            text_resp("t1"),
            call_resp("b", "ask_user", serde_json::json!({ "question": "which?" })),
            text_resp("Need your input."),
        ]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("go"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let (dones, texts) = terminal_frames(&drain(&mut rx));
        assert_eq!(dones, 1);
        assert_eq!(texts, vec!["Need your input.".to_string()]);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn cap_stops_non_converging_loop() {
        // Goal set but never completed. cap=2 → 1 initial + 2 continuations = 3
        // turns, then stop with an incomplete note. A 4th turn would exhaust the
        // scripted provider and error, so reaching Ok proves the cap stopped it.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![
            call_resp("a", "set_goal", serde_json::json!({ "goal": "never done" })),
            text_resp("t1"),
            text_resp("t2"),
            text_resp("t3"),
        ]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("go forever"),
            &mut gate,
            &goal_state,
            2,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let (dones, texts) = terminal_frames(&drain(&mut rx));
        assert_eq!(dones, 1, "cap stop emits exactly one Done");
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("t3"), "carries the last turn's reply");
        assert!(
            texts[0].contains("auto-continue cap"),
            "tells the user the cap was hit; was: {}",
            texts[0]
        );
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn goal_cap_env_floor_is_one() {
        // Unset → default; the floor is enforced by `.max(1)`.
        assert_eq!(goal_max_continues_from_env(), DEFAULT_GOAL_MAX_CONTINUES);
    }

    /// All `Assistant` frames with their speech, in order.
    fn assistant_frames(msgs: &[ServerMsg]) -> Vec<(String, Option<String>)> {
        msgs.iter()
            .filter_map(|m| match m {
                ServerMsg::Assistant { text, speech, .. } => Some((text.clone(), speech.clone())),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn voice_on_emits_speech_only_on_terminal_turn() {
        // A voice-on goal loop runs intermediate continuation turns, then
        // completes. Only the terminal turn emits an Assistant frame, and it
        // carries the spoken channel split from the model's sentinel.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let terminal = format!(
            "All finished.\n{}\nAll done, take a look.",
            agent_core::SPEECH_SENTINEL
        );
        let provider = MockProvider::new(vec![
            call_resp("a", "set_goal", serde_json::json!({ "goal": "finish" })),
            text_resp("working"), // ends turn 1 (intermediate, not emitted)
            text_resp("still working"), // ends turn 2 (intermediate, not emitted)
            call_resp(
                "b",
                "complete_goal",
                serde_json::json!({ "summary": "done" }),
            ),
            text_resp(&terminal), // terminal turn carries the spoken channel
        ]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("do the work"),
            &mut gate,
            &goal_state,
            5,
            true,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let frames = assistant_frames(&drain(&mut rx));
        assert_eq!(frames.len(), 1, "only the terminal turn emits an Assistant");
        assert_eq!(frames[0].0, "All finished.");
        assert_eq!(frames[0].1.as_deref(), Some("All done, take a look."));
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn voice_off_terminal_turn_has_no_speech() {
        // Same single-shot turn with voice off: the Assistant frame carries no
        // spoken channel.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![text_resp("hi")]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("hello"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let frames = assistant_frames(&drain(&mut rx));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "hi");
        assert_eq!(frames[0].1, None);
        let _ = std::fs::remove_dir_all(home);
    }

    // ---- learning-loop reflection tests ---------------------------------------

    #[test]
    fn reflect_threshold_env_default() {
        // Unset → default; 0 disables (read by the env helper).
        assert_eq!(
            skill_reflect_min_steps_from_env(),
            DEFAULT_SKILL_REFLECT_MIN_STEPS
        );
    }

    #[tokio::test]
    async fn drive_to_goal_returns_step_count() {
        // A single-shot message reports at least one provider step.
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![text_resp("hi")]);
        let mut gate = agent_core::AutoApprove;
        let steps = drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("hello"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        assert!(steps >= 1, "a completed turn reports its step count");
        let _ = drain(&mut rx);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn reflection_fires_once_when_over_threshold() {
        // steps >= min_steps → exactly one reflection turn (the provider has only
        // one scripted reply, so a second turn would exhaust it and error).
        let (storage, tools, _goal, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![text_resp("saved skill: triage")]);
        let mut gate = agent_core::AutoApprove;
        maybe_reflect(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            &mut gate,
            5,
            5,
        )
        .await
        .unwrap();
        let (dones, texts) = terminal_frames(&drain(&mut rx));
        assert_eq!(dones, 1, "exactly one reflection turn ran");
        assert_eq!(texts, vec!["saved skill: triage".to_string()]);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn reflection_skipped_under_threshold_or_disabled() {
        // No scripted replies: if any reflection turn ran it would error, so the
        // calls succeeding proves nothing ran.
        let (storage, tools, _goal, home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![]);
        let mut gate = agent_core::AutoApprove;
        // Below threshold.
        maybe_reflect(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            &mut gate,
            3,
            5,
        )
        .await
        .unwrap();
        // Disabled (min_steps == 0), even with a high step count.
        maybe_reflect(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            &mut gate,
            99,
            0,
        )
        .await
        .unwrap();
        let (dones, _) = terminal_frames(&drain(&mut rx));
        assert_eq!(dones, 0, "no reflection turn ran");
        let _ = std::fs::remove_dir_all(home);
    }

    /// The attention hint on the (single) emitted Assistant frame, if any.
    fn frame_attention(msgs: &[ServerMsg]) -> Option<AttentionHint> {
        msgs.iter()
            .find_map(|m| match m {
                ServerMsg::Assistant { attention, .. } => Some(attention.clone()),
                _ => None,
            })
            .flatten()
    }

    #[tokio::test]
    async fn voice_on_terminal_turn_carries_attention() {
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let raw = format!(
            "done\n{}\nspoken\n{}\ndevice=nas; look=the plex log; url=http://nas/log",
            agent_core::SPEECH_SENTINEL,
            agent_core::ATTENTION_SENTINEL
        );
        let provider = MockProvider::new(vec![text_resp(&raw)]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("status"),
            &mut gate,
            &goal_state,
            5,
            true,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        let a = frame_attention(&drain(&mut rx)).expect("attention on terminal turn");
        assert_eq!(a.device, "nas");
        assert_eq!(a.look_at, "the plex log");
        assert_eq!(a.url.as_deref(), Some("http://nas/log"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn voice_off_terminal_turn_has_no_attention() {
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        // Even if the text contains the marker, voice off → no parsing, no hint.
        let raw = format!("hi\n{}\ndevice=x; look=y", agent_core::ATTENTION_SENTINEL);
        let provider = MockProvider::new(vec![text_resp(&raw)]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("status"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
        )
        .await
        .unwrap();
        assert_eq!(frame_attention(&drain(&mut rx)), None);
        let _ = std::fs::remove_dir_all(home);
    }

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
            hostname: None,
        }
    }

    #[test]
    fn resolve_identity_token_authoritative_and_migrates() {
        use crate::auth::AuthStore;
        let home = std::env::temp_dir().join(format!("fleety-ident-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Storage::new(home.clone());
        let auth = AuthStore::load(storage.auth_path(), None, true);
        let code = auth.create_pairing().expect("code");
        let token = auth.redeem(&code, "old-host").expect("redeem"); // bound to legacy id
        storage
            .ensure_device("old-host", "client_session")
            .expect("dev");

        // Authed device now reports a machine id → migrate + rebind, identity = machine id.
        let resolved =
            resolve_device_identity(&auth, &storage, "machine-1", Some(&token), Some("old-host"));
        assert_eq!(resolved, "machine-1");
        assert_eq!(auth.verify(&token).as_deref(), Some("machine-1")); // token rebound
        assert!(storage.history_path("machine-1").parent().unwrap().exists());
        assert!(!storage.history_path("old-host").parent().unwrap().exists());

        // No clobber: if the machine-id dir already exists for another device, keep the bound id.
        let code2 = auth.create_pairing().expect("code2");
        let token2 = auth.redeem(&code2, "bound-b").expect("redeem2");
        storage
            .ensure_device("bound-b", "client_session")
            .expect("dev b");
        storage
            .ensure_device("machine-1", "client_session")
            .expect("dev m1"); // occupied
        let resolved2 = resolve_device_identity(&auth, &storage, "machine-1", Some(&token2), None);
        assert_eq!(resolved2, "bound-b"); // not hijacked onto machine-1
        assert_eq!(auth.verify(&token2).as_deref(), Some("bound-b"));

        // Unauthenticated: the asserted machine id is used; legacy hostname dir migrates.
        storage
            .ensure_device("host-c", "client_session")
            .expect("dev c");
        let resolved3 = resolve_device_identity(&auth, &storage, "machine-3", None, Some("host-c"));
        assert_eq!(resolved3, "machine-3");
        assert!(storage.history_path("machine-3").parent().unwrap().exists());
        let _ = std::fs::remove_dir_all(&home);
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
                hostname: None,
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
                hostname: None,
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
                hostname: None,
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
                voice: false,
                acting_user: None,
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
                voice: false,
                acting_user: None,
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
                hostname: None,
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
                hostname: None,
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
