//! Per-connection handling: WebSocket handshake, session, and the turn loop.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_tungstenite::tungstenite::Message as WsMessage;
// The raw-TCP WebSocket cluster (Tx/Rx, handle_conn, WsInbound/WsFrameWriter,
// read_client, is_disconnect) is the in-process test harness; production serves
// WebSocket through axum (see crate::http). These imports support that harness.
#[cfg(test)]
use futures::stream::{SplitSink, SplitStream};
#[cfg(test)]
use futures::{SinkExt, StreamExt};
#[cfg(test)]
use tokio::net::TcpStream;
#[cfg(test)]
use tokio_tungstenite::tungstenite::error::ProtocolError;
#[cfg(test)]
use tokio_tungstenite::tungstenite::Error as WsErr;
#[cfg(test)]
use tokio_tungstenite::WebSocketStream;

use tokio::sync::mpsc;

use agent_core::{
    reconstruct_messages, run_turn, run_turn_streaming, run_turn_streaming_cached,
    ApprovalDecision, ApprovalGate, AutoDeny, CoreError, GoalState, LoopConfig, Message,
    ModelProvider, Policy, Result, RiskLevel, Role, Terminal, ToolRegistry,
};
use fleety_protocol::{
    AttentionHint, ClientMsg, InterjectionDisposition, ServerMsg, WireError, PROTOCOL_VERSION,
};

use crate::auth::{self, AuthStore};
use crate::bridge::{self, DeviceTools, Handles, Hub, Pending};
use crate::storage::Storage;

/// Outbound frame sender (drained by the connection's writer task).
pub(crate) type Out = mpsc::UnboundedSender<WsMessage>;

const MAX_PENDING_INTERJECTIONS: usize = 16;
const MAX_PENDING_INTERJECTION_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIENT_MESSAGE_ID_BYTES: usize = 128;
const WRITER_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

fn pending_interjection_fits(count: usize, bytes: usize, incoming_bytes: usize) -> bool {
    count < MAX_PENDING_INTERJECTIONS
        && bytes.saturating_add(incoming_bytes) <= MAX_PENDING_INTERJECTION_BYTES
}

fn client_message_wire_bytes(message: &ClientMsg) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter(0);
    if serde_json::to_writer(&mut counter, message).is_err() {
        usize::MAX
    } else {
        counter.0
    }
}

fn client_message_id_is_valid(message_id: &str) -> bool {
    !message_id.is_empty() && message_id.len() <= MAX_CLIENT_MESSAGE_ID_BYTES
}

fn trusted_same_host_origin(loopback_trusted: bool) -> bool {
    loopback_trusted
}

fn protocol_is_supported(protocol: u32) -> bool {
    protocol == PROTOCOL_VERSION
}

fn interjection_decision(
    action: crate::triage::TriageAction,
) -> (InterjectionDisposition, &'static str, bool) {
    match action {
        crate::triage::TriageAction::InterruptNow => (
            InterjectionDisposition::Interrupting,
            "interrupting the current task to handle your new message",
            true,
        ),
        crate::triage::TriageAction::QueueAfter => (
            InterjectionDisposition::Queued,
            "got it — I'll handle that right after the current task",
            true,
        ),
        crate::triage::TriageAction::Ignore => (
            InterjectionDisposition::Ignored,
            "noted — this message won't start another turn",
            false,
        ),
    }
}

#[cfg(test)]
fn server_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

#[cfg(test)]
type Tx = SplitSink<WebSocketStream<TcpStream>, WsMessage>;
#[cfg(test)]
type Rx = SplitStream<WebSocketStream<TcpStream>>;

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

// ---- Claude Code hooks compatibility: conn-layer production wiring ----------
//
// The pure hook engine (parse / match / wrap / deny / audit / env policy) lives
// in `hooks_compat`. Here we supply the production runner (local shell for a
// same-host origin, `run_command` routed to the origin device for cross-device)
// and the audit sink (append to the device history), then wrap the bound
// conversation's tools. Everything is best-effort: a hook that cannot run is
// recorded and skipped; only a completed non-zero `PreToolUse` run denies.

/// Runs a hook command on the origin device. Same-host (`device` is `None`) runs
/// a local shell; cross-device routes `run_command` to the origin device.
struct OriginHookRunner {
    device: Option<String>,
    cwd: Option<String>,
    hub: Hub,
    pending: Pending,
}

/// Modest ceiling so a hung hook can't block a tool call forever (local path;
/// the cross-device path inherits the origin's `run_command` timeout).
const HOOK_LOCAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn run_local_hook(command: &str, cwd: Option<&str>) -> crate::hooks_compat::HookOutcome {
    use crate::hooks_compat::HookOutcome;
    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let mut child = tokio::process::Command::new(shell);
    child
        .arg(flag)
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        child.current_dir(cwd);
    }
    let spawned = match child.spawn() {
        Ok(c) => c,
        Err(e) => return HookOutcome::Failed(format!("cannot run hook: {e}")),
    };
    match tokio::time::timeout(HOOK_LOCAL_TIMEOUT, spawned.wait_with_output()).await {
        Ok(Ok(out)) => HookOutcome::Exited(out.status.code().unwrap_or(-1)),
        Ok(Err(e)) => HookOutcome::Failed(format!("hook failed: {e}")),
        Err(_) => HookOutcome::Failed("hook timed out".to_string()),
    }
}

/// Interpret an on-device `run_command` result (`{ exit_code, timed_out }`) as a
/// [`HookOutcome`](crate::hooks_compat::HookOutcome).
fn outcome_from_run_command(v: &serde_json::Value) -> crate::hooks_compat::HookOutcome {
    use crate::hooks_compat::HookOutcome;
    if v.get("timed_out")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return HookOutcome::Failed("hook timed out on origin device".to_string());
    }
    match v.get("exit_code") {
        Some(c) if c.is_null() => HookOutcome::Failed("hook produced no exit code".to_string()),
        Some(c) => HookOutcome::Exited(c.as_i64().unwrap_or(-1) as i32),
        None => HookOutcome::Exited(0),
    }
}

#[async_trait::async_trait]
impl crate::hooks_compat::HookRunner for OriginHookRunner {
    async fn run(
        &self,
        entry: &crate::hooks_compat::HookEntry,
    ) -> crate::hooks_compat::HookOutcome {
        use crate::hooks_compat::HookOutcome;
        match &self.device {
            None => run_local_hook(&entry.command, self.cwd.as_deref()).await,
            Some(dev) => {
                let sender = self.hub.lock().await.get(dev).cloned();
                let Some(sender) = sender else {
                    return HookOutcome::Failed(format!("origin device '{dev}' not connected"));
                };
                let mut args = serde_json::json!({ "command": entry.command });
                if let Some(cwd) = &self.cwd {
                    args["cwd"] = serde_json::json!(cwd);
                }
                match bridge::route_run_tool_via(&sender, &self.pending, "run_command", args).await
                {
                    Ok(v) => outcome_from_run_command(&v),
                    Err(e) => HookOutcome::Failed(e.report().message),
                }
            }
        }
    }
}

/// Audits each hook execution by appending it to the device history (the same
/// stream `history_list` / the audit CLI read), tagged with its conversation.
struct HistoryHookAudit {
    storage: Arc<Storage>,
    device_id: String,
    conversation: String,
}

impl crate::hooks_compat::HookAudit for HistoryHookAudit {
    fn record(
        &self,
        entry: &crate::hooks_compat::HookEntry,
        outcome: &crate::hooks_compat::HookOutcome,
    ) {
        let ev = agent_core::Event::ToolResult {
            id: uuid::Uuid::new_v4().to_string(),
            result: crate::hooks_compat::audit_payload(entry, outcome),
        };
        // Best-effort: an audit write failure must not break the tool call.
        let _ = self
            .storage
            .append_history_tagged(&self.device_id, &self.conversation, &ev);
    }
}

/// Collect the conversation's Claude Code hooks and apply the env policy.
/// Same-host reads local `.claude/settings.json` (origin cwd + local home);
/// cross-device reads the origin device's project + user `settings.json` via the
/// bridge. Best-effort throughout: an offline device or missing/bad file yields
/// no hooks, never an error.
async fn collect_conversation_hooks(
    device: Option<&str>,
    origin_cwd: Option<&str>,
    origin_home: Option<&str>,
    local_home: Option<&str>,
    hub: &Hub,
    pending: &Pending,
) -> Vec<crate::hooks_compat::HookEntry> {
    use crate::hooks_compat::{apply_env_policy, collect_hooks, parse_hooks, HookScope};
    let hooks = match device {
        None => match (origin_cwd, local_home) {
            (Some(cwd), Some(home)) => {
                collect_hooks(std::path::Path::new(cwd), std::path::Path::new(home))
            }
            _ => Vec::new(),
        },
        Some(dev) => {
            let sender = hub.lock().await.get(dev).cloned();
            let Some(sender) = sender else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for (scope, base) in [
                (HookScope::Project, origin_cwd),
                (HookScope::User, origin_home),
            ] {
                let Some(base) = base else {
                    continue;
                };
                let path = std::path::Path::new(base)
                    .join(".claude")
                    .join("settings.json");
                // `read_file` takes `path` (not `file`), and returns only the
                // line-numbered view, so the numbering is undone before parsing.
                let args = serde_json::json!({ "path": path.to_string_lossy() });
                if let Ok(res) =
                    bridge::route_run_tool_via(&sender, pending, "read_file", args).await
                {
                    if let Some(numbered) = res.get("numbered").and_then(serde_json::Value::as_str)
                    {
                        let content = fleety_tools::strip_line_numbers(numbered);
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                            out.extend(parse_hooks(&v, scope));
                        }
                    }
                }
            }
            out
        }
    };
    apply_env_policy(hooks)
}

/// A conversation's collected hooks plus the origin location needed to run them.
/// Shared (via `Arc`) between the primary conversation registry and subagent
/// registries so a safety hook can't be bypassed by delegating to a subagent.
pub(crate) struct HookContext {
    pub hooks: Vec<crate::hooks_compat::HookEntry>,
    pub device: Option<String>,
    pub cwd: Option<String>,
}

/// Wrap a freshly built registry with `ctx`'s hooks, auditing under
/// `conversation`. A no-op when there are no hooks. The single place both the
/// primary conversation and subagent registries go through, so their hook
/// semantics stay identical.
pub(crate) fn wrap_registry_with_hooks(
    tools: &mut ToolRegistry,
    ctx: &HookContext,
    hub: &Hub,
    pending: &Pending,
    storage: &Arc<Storage>,
    device_id: &str,
    conversation: &str,
) {
    if ctx.hooks.is_empty() {
        return;
    }
    let runner: Arc<dyn crate::hooks_compat::HookRunner> = Arc::new(OriginHookRunner {
        device: ctx.device.clone(),
        cwd: ctx.cwd.clone(),
        hub: Arc::clone(hub),
        pending: Arc::clone(pending),
    });
    let audit: Arc<dyn crate::hooks_compat::HookAudit> = Arc::new(HistoryHookAudit {
        storage: Arc::clone(storage),
        device_id: device_id.to_string(),
        conversation: conversation.to_string(),
    });
    let wrapped = crate::hooks_compat::wrap_tools(tools.drain(), &ctx.hooks, runner, audit);
    for w in wrapped {
        tools.register(w);
    }
}

/// Run a conversation's lifecycle-event hooks (`UserPromptSubmit` / `Stop` /
/// `SubagentStop`) for `event`, auditing each. Returns whether to proceed — only
/// a non-zero `UserPromptSubmit` blocks. A no-op returning `true` when `ctx` has
/// no hook for this event (so callers pay nothing on the common path).
pub(crate) async fn run_conversation_event_hooks(
    event: crate::hooks_compat::HookEvent,
    ctx: &HookContext,
    hub: &Hub,
    pending: &Pending,
    storage: &Arc<Storage>,
    device_id: &str,
    conversation: &str,
) -> bool {
    if !ctx.hooks.iter().any(|h| h.event == event) {
        return true;
    }
    let runner: Arc<dyn crate::hooks_compat::HookRunner> = Arc::new(OriginHookRunner {
        device: ctx.device.clone(),
        cwd: ctx.cwd.clone(),
        hub: Arc::clone(hub),
        pending: Arc::clone(pending),
    });
    let audit: Arc<dyn crate::hooks_compat::HookAudit> = Arc::new(HistoryHookAudit {
        storage: Arc::clone(storage),
        device_id: device_id.to_string(),
        conversation: conversation.to_string(),
    });
    crate::hooks_compat::run_event_hooks(event, &ctx.hooks, &runner, &audit).await
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
    conversation_sources: &[std::path::PathBuf],
    conversation_mcp: Vec<crate::mcp::ServerCfg>,
) -> (
    agent_core::ToolRegistry,
    Arc<crate::subagent::FleetyHost>,
    Arc<tokio::sync::Mutex<GoalState>>,
    crate::effort::SessionEffort,
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
        conversation_sources,
        conversation_mcp,
    );
    // Conversation recall, scoped to the acting user (per-user history).
    crate::conversation_recall::register(
        &mut tools,
        Arc::clone(storage),
        acting.user_id().map(String::from),
    );
    // Let the user set their timezone (read side already feeds the system prompt).
    crate::tz::register(&mut tools, Arc::clone(storage), acting.clone());
    // Let a user share their own data with another user (cross-user grant).
    crate::privacy::register_grant(&mut tools, Arc::clone(storage), acting.clone());
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
    // The agent can adjust its own reasoning effort for subsequent turns.
    let session_effort = crate::effort::new_session_effort();
    crate::effort::register(&mut tools, Arc::clone(&session_effort));
    crate::conversation_lifecycle::register(&mut tools, Arc::clone(rollover_state));
    // Editor-backed tools (ACP delegation): when the connecting editor advertised
    // fs/terminal tools, the agent gets `editor_*` tools that route to this very
    // connection (its `out` sender) so file edits go through the user's editor.
    crate::editor_tools::register_editor(&mut tools, out, Arc::clone(pending), editor_specs);
    (tools, subagent_host, goal_state, session_effort)
}

/// Handle one client connection over a raw-TCP WebSocket. Production serves
/// WebSocket via axum ([`crate::http`]); this is the in-process test harness,
/// and like the axum path it delegates to [`run_connection`].
#[cfg(test)]
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
    handle_conn_with_peer(
        stream,
        storage,
        provider,
        workspace,
        policy,
        hub,
        pending,
        handles,
        auth,
        device_tools,
        false,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn handle_conn_with_peer(
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
    peer_is_loopback: bool,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| CoreError::Provider(format!("websocket handshake failed: {e}")))?;
    let (tx, rx) = ws.split();
    run_connection(
        Box::new(WsInbound { rx }),
        Box::new(WsFrameWriter { tx }),
        storage,
        provider,
        workspace,
        policy,
        hub,
        pending,
        handles,
        auth,
        device_tools,
        peer_is_loopback,
    )
    .await
}

/// Run one client connection over any transport: read `Hello` + authenticate,
/// then drive the conversation loop with an [`Out`] channel drained into the
/// transport's [`FrameWriter`]. WebSocket ([`handle_conn`]) and the SSE+POST
/// fallback both call this; only `inbound`/`writer` differ.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connection(
    inbound: Box<dyn ClientInbound>,
    writer: Box<dyn FrameWriter>,
    storage: Arc<Storage>,
    provider: Arc<dyn ModelProvider>,
    workspace: Arc<PathBuf>,
    policy: Policy,
    hub: Hub,
    pending: Pending,
    handles: Handles,
    auth: Arc<AuthStore>,
    device_tools: DeviceTools,
    peer_is_loopback: bool,
) -> Result<()> {
    // Before anything is said: if the client offers the secure channel, answer
    // it here so every frame below — Hello and its token included — travels
    // sealed. A client that does not offer one keeps the cleartext path.
    let (mut inbound, mut writer, sealed_session) =
        negotiate_secure_channel(inbound, writer, &auth).await?;

    // The first frame must be Hello; enforce auth if the server requires it.
    let (device_id, minted_token, loopback_trusted, daemon_capable, connection_tools) =
        match inbound.next_client().await? {
            Some(ClientMsg::Hello {
                device_id,
                protocol,
                token,
                pairing_code,
                local_tools_json,
                hostname,
            }) => {
                if !protocol_is_supported(protocol) {
                    tracing::warn!(
                        client_protocol = protocol,
                        server_protocol = PROTOCOL_VERSION,
                        "rejecting incompatible protocol version"
                    );
                    send_error_frame(
                        &mut *writer,
                        "protocol_mismatch",
                        &format!(
                            "client protocol {protocol} is incompatible with server protocol {PROTOCOL_VERSION}"
                        ),
                        "update all Fleety binaries to the same release",
                    )
                    .await;
                    return Ok(());
                }
                // Trusted-loopback = accepted on same-host trust with no credential
                // (a valid token still authenticates normally). Recorded so Welcome
                // can tell the client it needn't pair.
                let loopback_trusted = auth.required()
                    && token.is_none()
                    && pairing_code.is_none()
                    && peer_is_loopback
                    && trust_loopback_enabled();
                match authenticate(
                    &auth,
                    &device_id,
                    token.as_deref(),
                    pairing_code.as_deref(),
                    peer_is_loopback,
                ) {
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
                        let mut connection_tools = Vec::new();
                        let mut daemon_capable = false;
                        if let Some(json) = local_tools_json {
                            match serde_json::from_str::<Vec<agent_core::ToolSpec>>(&json) {
                                Ok(specs) => {
                                    daemon_capable = specs.iter().any(|spec| {
                                        matches!(spec.name.as_str(), "read_file" | "run_command")
                                    });
                                    connection_tools = specs;
                                }
                                Err(e) => {
                                    tracing::warn!(%device_id, error = %e, "could not parse advertised tools");
                                }
                            }
                        }
                        if daemon_capable {
                            device_tools
                                .lock()
                                .await
                                .insert(device_id.clone(), connection_tools.clone());
                        }
                        (
                            device_id,
                            minted,
                            loopback_trusted,
                            daemon_capable,
                            connection_tools,
                        )
                    }
                    Err(message) => {
                        tracing::warn!(%device_id, "rejected unauthenticated connection");
                        send_error_frame(
                        &mut *writer,
                        "unauthenticated",
                        &message,
                        "pass a valid token, or a pairing_code from `pair_create` on a paired device",
                    )
                    .await;
                        return Ok(());
                    }
                }
            }
            Some(_) => {
                send_error_frame(
                    &mut *writer,
                    "expected_hello",
                    "first frame must be Hello",
                    "send a Hello frame with your device_id before anything else",
                )
                .await;
                return Ok(());
            }
            None => return Ok(()),
        };

    // Register / refresh this device in the registry.
    storage.ensure_device(&device_id, "client_session")?;
    // A socket peer being loopback is not, by itself, proof that the origin is
    // the Server host: a reverse proxy can make a remote client look local.
    // Require the same explicit loopback-trust decision used by authentication;
    // auth-disabled servers therefore do not grant local-workspace access.
    let same_host_origin = trusted_same_host_origin(loopback_trusted);

    // A single writer task owns the transport sink; everything else (this handler,
    // the approval gate, and other connections routing RunTool here) sends frames
    // through `out`, registered in the hub under this device_id.
    let (out, mut out_rx) = mpsc::unbounded_channel::<WsMessage>();
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let WsMessage::Text(text) = frame else {
                continue; // only text frames are emitted
            };
            if !writer.send_text(text.to_string()).await {
                break;
            }
        }
    });
    if daemon_capable {
        hub.lock().await.insert(device_id.clone(), out.clone());
    }

    let result = serve(
        &mut *inbound,
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
        loopback_trusted,
        same_host_origin,
        &connection_tools,
        sealed_session,
    )
    .await;

    if daemon_capable {
        let mut active = hub.lock().await;
        if active
            .get(&device_id)
            .is_some_and(|sender| sender.same_channel(&out))
        {
            active.remove(&device_id);
            device_tools.lock().await.remove(&device_id);
        }
    }
    drop(out);
    let mut writer_task = writer_task;
    if tokio::time::timeout(WRITER_DRAIN_TIMEOUT, &mut writer_task)
        .await
        .is_err()
    {
        writer_task.abort();
    }
    tracing::info!(%device_id, "client disconnected");
    result
}

/// The session loop, factored out so `handle_conn` can always clean up the hub
/// entry and writer task afterward.
#[allow(clippy::too_many_arguments)]
async fn serve(
    inbound: &mut dyn ClientInbound,
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
    loopback_trusted: bool,
    same_host_origin: bool,
    connection_tools: &[agent_core::ToolSpec],
    sealed_session: bool,
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
            server_version: agent_core::VERSION.to_string(),
            audio_input: provider.capabilities().audio,
            config_protocol: fleety_protocol::CONFIG_PROTOCOL_VERSION,
            server_fingerprint: {
                let fp = crate::server_fingerprint();
                (!fp.is_empty()).then(|| fp.to_string())
            },
            // Only a sealed session may act on these, so an unsealed one has
            // nothing to gain from them — and this Server's interface list is
            // not something to hand an unauthenticated peer.
            server_endpoints: if sealed_session {
                crate::authenticated_server_endpoints()
            } else {
                Vec::new()
            },
            loopback_trusted,
            token: minted_token,
        },
    )?;

    // Proactively surface schedule outcomes finished since this owner last
    // connected (owner-scoped, best-effort; see scheduler::tick / schedules).
    deliver_pending_schedule_notifications(storage, out, device_id);

    // Build the tool stack rooted at the server's default workspace. A
    // conversation may rebind it to the originating CLI's cwd on its first
    // message (see the UserMessage arm), making Fleety a coding agent in that
    // directory. The orchestration/workflow/goal tools live only at this top
    // level (subagent child registries omit them, capping nesting at one level).
    let mut current_root: std::path::PathBuf = workspace.to_path_buf();
    // Before the first message we don't yet know the acting user; default recall
    // scope to the device owner (rebuilt with the resolved user on first message).
    let connect_acting = storage.acting_for_device(device_id);
    let rollover_state = crate::conversation_lifecycle::new_state();
    // Editor-backed tools to offer this connection: the `editor_*` subset it
    // advertised in Hello (an ACP editor gates these by the editor's capabilities).
    let editor_specs: Vec<agent_core::ToolSpec> = connection_tools
        .iter()
        .filter(|s| s.name.starts_with("editor_"))
        .cloned()
        .collect();
    let (mut tools, mut subagent_host, mut goal_state, mut session_effort) = build_connection_stack(
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
        &[],
        Vec::new(),
    );
    let mut workspace_bound = false;
    // The bound conversation's hooks, retained for lifecycle-event hooks
    // (UserPromptSubmit / Stop) fired around each user message in this loop.
    let mut conv_hook_ctx: Option<Arc<HookContext>> = None;
    let goal_max_continues = goal_max_continues_from_env();
    let skill_reflect_min_steps = skill_reflect_min_steps_from_env();
    // Approval gates temporarily own the transport reader. Any non-approval
    // frame they observe is returned here in arrival order for normal service
    // processing after the gated turn completes.
    let mut deferred_inbound = std::collections::VecDeque::new();
    let mut deferred_inbound_bytes = 0usize;

    loop {
        let msg = match deferred_inbound.pop_front() {
            Some(msg) => {
                deferred_inbound_bytes =
                    deferred_inbound_bytes.saturating_sub(client_message_wire_bytes(&msg));
                msg
            }
            None => match inbound.next_client().await? {
                Some(msg) => msg,
                None => break,
            },
        };
        match msg {
            // Channel-setup frames belong to the handshake, which has already
            // run by the time the service loop sees anything. On a secure
            // channel these are stripped by the inbound wrapper, so reaching
            // here means the client is either re-offering a handshake or trying
            // to seal on a cleartext link. Refuse rather than reinterpret.
            ClientMsg::SecureHandshake { .. } | ClientMsg::SecureFrame { .. } => {
                return Err(CoreError::Provider(
                    "client sent a secure-channel frame outside the handshake".to_string(),
                ))
            }
            ClientMsg::UserMessage {
                message_id: active_message_id,
                conversation_id,
                text,
                origin,
                attachments,
                voice,
                acting_user,
            } => {
                if !client_message_id_is_valid(&active_message_id) {
                    emit(
                        out,
                        &ServerMsg::Error {
                            error: WireError {
                                kind: "invalid_message_id".to_string(),
                                message: format!(
                                    "message_id must contain 1 to {MAX_CLIENT_MESSAGE_ID_BYTES} bytes"
                                ),
                                remediation: Some(
                                    "update the Fleety client and reconnect".to_string(),
                                ),
                            },
                        },
                    )?;
                    return Ok(());
                }
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
                                origin.os.as_deref(),
                                device_id,
                                same_host_origin,
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
                    // Conversation-scoped skill sources: same-host origins only
                    // (deeper project/user .claude/.agents skills); cross-device
                    // and absent origins fall back to the global tiers.
                    // Plus enabled-plugin resources: direct .agents/.claude
                    // skills first, then enabled-plugin skills (lower); plugin
                    // MCP servers become per-conversation servers.
                    let (conversation_sources, conversation_mcp): (
                        Vec<std::path::PathBuf>,
                        Vec<crate::mcp::ServerCfg>,
                    ) = if binding.device.is_none() {
                        match (
                            binding.origin_cwd.as_deref(),
                            std::env::var("HOME")
                                .ok()
                                .or_else(|| std::env::var("USERPROFILE").ok()),
                        ) {
                            (Some(cwd), Some(home)) => {
                                let cwd_p = std::path::Path::new(cwd);
                                let home_p = std::path::Path::new(&home);
                                let mut dirs: Vec<std::path::PathBuf> =
                                    crate::skill_sources::skill_sources(cwd_p, home_p)
                                        .into_iter()
                                        .filter(|d| d.is_dir())
                                        .collect();
                                let plugins =
                                    crate::plugin_sources::collect_plugin_sources(cwd_p, home_p);
                                dirs.extend(plugins.skill_dirs.into_iter().map(|(_s, d)| d));
                                let mut mcp: Vec<crate::mcp::ServerCfg> = plugins
                                    .mcp_servers
                                    .into_iter()
                                    .map(|(_s, m)| crate::mcp::ServerCfg {
                                        name: m.name,
                                        command: m.command,
                                        args: m.args,
                                        builtin: false,
                                    })
                                    .collect();
                                // Codex config.toml MCP servers (user scope), after plugins.
                                mcp.extend(
                                    crate::codex_sources::collect_codex_mcp(home_p)
                                        .into_iter()
                                        .map(|m| crate::mcp::ServerCfg {
                                            name: m.name,
                                            command: m.command,
                                            args: m.args,
                                            builtin: false,
                                        }),
                                );
                                (dirs, mcp)
                            }
                            _ => (Vec::new(), Vec::new()),
                        }
                    } else {
                        (Vec::new(), Vec::new())
                    };
                    let (t, h, g, se) = build_connection_stack(
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
                        &conversation_sources,
                        conversation_mcp,
                    );
                    tools = t;
                    subagent_host = h;
                    goal_state = g;
                    session_effort = se;
                    workspace_bound = true;

                    // Claude Code hooks: collect the origin device's
                    // Pre/PostToolUse hooks and wrap this conversation's tools so
                    // they run around each tool call (a non-zero PreToolUse run
                    // denies; every run is audited; project hooks are env
                    // kill-switchable). Same-host reads local settings; cross-
                    // device reads the origin's via the bridge. Best-effort — no
                    // hooks leaves the tools unwrapped (zero overhead).
                    let local_home = std::env::var("HOME")
                        .ok()
                        .or_else(|| std::env::var("USERPROFILE").ok());
                    let hooks = collect_conversation_hooks(
                        binding.device.as_deref(),
                        binding.origin_cwd.as_deref(),
                        origin.home.as_deref(),
                        local_home.as_deref(),
                        hub,
                        pending,
                    )
                    .await;
                    if !hooks.is_empty() {
                        // One shared HookContext: the primary registry and every
                        // subagent registry wrap through it, so a safety hook
                        // can't be bypassed by delegating to a subagent.
                        let ctx = Arc::new(HookContext {
                            hooks,
                            device: binding.device.clone(),
                            cwd: binding.origin_cwd.clone(),
                        });
                        wrap_registry_with_hooks(
                            &mut tools,
                            &ctx,
                            hub,
                            pending,
                            storage,
                            device_id,
                            &conversation,
                        );
                        subagent_host.set_hook_context(Arc::clone(&ctx));
                        conv_hook_ctx = Some(ctx);
                    }
                }

                // Hold the per-connection turn lock across BOTH recovery and this
                // turn so a background subagent's wake turn can't interleave
                // storage appends; record the active conversation so a `fork`
                // subagent inherits it.
                let _turn_guard = subagent_host.lock_turn().await;
                subagent_host.set_active_conversation(&conversation).await;

                // The agent's manual effort pin (set_effort) drives the recovery
                // of any interrupted prior turn below. The turn for THIS message
                // is driven by drive_to_goal, which re-reads the pin (and this
                // message's difficulty baseline) before every turn it runs, so
                // the effort here is only for recovery. None keeps the default.
                let session_eff = *session_effort.lock().await;
                let effort_provider = session_eff.and_then(|e| provider.with_effort(Some(e)));
                let turn_provider: &dyn ModelProvider =
                    effort_provider.as_deref().unwrap_or(provider);

                // Count this as an in-flight turn for the whole of recovery + the
                // turn itself, so a deferred `restart` waits for it (idle == no
                // in-flight turn). RAII: dropped at the end of this arm on every
                // path (normal, `?`, or `continue`), never left stuck above zero.
                let _inflight = crate::restart_watch::turn_guard();

                // First finish any turn left interrupted by a crash/redeploy, so
                // it isn't lost and doesn't interleave with this message. Best
                // effort: on failure the journal stays for a later retry.
                if let Err(e) = recover_incomplete_turn(
                    inbound,
                    out,
                    storage,
                    turn_provider,
                    &tools,
                    policy,
                    device_id,
                    &conversation,
                    RecoveryEmission::LiveTurn,
                    &mut deferred_inbound,
                    &mut deferred_inbound_bytes,
                )
                .await
                {
                    tracing::warn!(%device_id, conversation = %conversation, report = ?e.report(), "could not recover interrupted turn");
                }

                // Auto-effort: when the agent hasn't pinned an effort and
                // FLEETY_AUTO_EFFORT is on, classify this message's difficulty
                // once (economy tier) so even the FIRST inference of this request
                // starts in the right gear. The pick is NOT written to the pin
                // slot — it is this message's baseline only, so it never persists
                // of its own accord (drive_to_goal treats a manual pin as higher
                // precedence). See crate::effort and the per-task-effort spec.
                let turn_baseline = if session_eff.is_none() && crate::effort::auto_effort_enabled()
                {
                    let cheap = crate::providers::ProviderTiers::from_env().resolve("cheap");
                    crate::effort::assess_effort(&text, cheap.as_ref()).await
                } else {
                    None
                };

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
                // UserPromptSubmit hooks (origin device): run before processing;
                // a non-zero hook blocks this prompt entirely (its turn is not
                // run and the message is not stored), mirroring PreToolUse deny.
                if let Some(ctx) = &conv_hook_ctx {
                    let proceed = run_conversation_event_hooks(
                        crate::hooks_compat::HookEvent::UserPromptSubmit,
                        ctx,
                        hub,
                        pending,
                        storage,
                        device_id,
                        &conversation,
                    )
                    .await;
                    if !proceed {
                        let _ = emit(
                            out,
                            &ServerMsg::Assistant {
                                conversation_id: conversation.clone(),
                                text: "A UserPromptSubmit hook blocked this prompt \
                                       (non-zero exit); it was not processed."
                                    .to_string(),
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
                }
                let steps = if matches!(policy, Policy::FullAccess) {
                    // Parallel-read interruption (full-access only — the approval
                    // gate doesn't read inbound here, so we can read a mid-turn
                    // message while the turn runs). A triaged interjection or an
                    // explicit CancelTurn cancels the run at its next checkpoint;
                    // a queued interjection runs after.
                    let cancel = CancelFlag::new();
                    let mut gate = agent_core::AutoApprove;
                    let turn = drive_to_goal(
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
                        &cancel,
                        &session_effort,
                        turn_baseline,
                    );
                    tokio::pin!(turn);
                    let mut pending: VecDeque<(String, String, Vec<agent_core::Attachment>)> =
                        VecDeque::new();
                    let mut pending_bytes = 0usize;
                    let mut client_open = true;
                    let first_steps = loop {
                        if !client_open {
                            break (&mut turn).await;
                        }
                        tokio::select! {
                            res = &mut turn => break res,
                            next = inbound.next_client() => match next {
                                Ok(Some(ClientMsg::UserMessage { message_id, text: t2, attachments: a2, .. })) => {
                                    let duplicate_message_id = message_id == active_message_id
                                        || pending.iter().any(|(id, _, _)| id == &message_id);
                                    if !client_message_id_is_valid(&message_id) || duplicate_message_id {
                                        let _ = emit(out, &ServerMsg::InterjectionAcknowledged {
                                            conversation_id: conversation.to_string(),
                                            message_id,
                                            disposition: InterjectionDisposition::Rejected,
                                            message: "message_id is invalid or already in use — resend with a new id".to_string(),
                                        });
                                        continue;
                                    }
                                    let message_bytes = a2.iter().fold(
                                        t2.len().saturating_add(message_id.len()),
                                        |total, attachment| {
                                        total
                                            .saturating_add(attachment.mime.len())
                                            .saturating_add(attachment.bytes_b64.as_ref().map_or(0, String::len))
                                            .saturating_add(attachment.url.as_ref().map_or(0, String::len))
                                            .saturating_add(attachment.name.as_ref().map_or(0, String::len))
                                        },
                                    );
                                    let queue_has_capacity = pending_interjection_fits(
                                        pending.len(),
                                        pending_bytes,
                                        message_bytes,
                                    );
                                    if !queue_has_capacity {
                                        let _ = emit(out, &ServerMsg::InterjectionAcknowledged {
                                            conversation_id: conversation.to_string(),
                                            message_id,
                                            disposition: InterjectionDisposition::Rejected,
                                            message: "the interjection queue is full — resend after the current task".to_string(),
                                        });
                                        continue;
                                    }
                                    let summary = goal_state.lock().await.nudge_text();
                                    let action = crate::triage::triage(&t2, &summary, provider).await;
                                    let atts: Vec<agent_core::Attachment> = a2
                                        .into_iter()
                                        .map(|a| agent_core::Attachment {
                                            mime: a.mime,
                                            bytes_b64: a.bytes_b64,
                                            url: a.url,
                                            name: a.name,
                                        })
                                        .collect();
                                    let (disposition, ack, store) =
                                        interjection_decision(action);
                                    if disposition == InterjectionDisposition::Interrupting {
                                        cancel.request_triage();
                                    }
                                    let _ = emit(out, &ServerMsg::InterjectionAcknowledged {
                                        conversation_id: conversation.to_string(),
                                        message_id: message_id.clone(),
                                        disposition,
                                        message: ack.to_string(),
                                    });
                                    if store {
                                        pending_bytes = pending_bytes.saturating_add(message_bytes);
                                        pending.push_back((message_id, t2, atts));
                                    }
                                }
                                Ok(Some(ClientMsg::CancelTurn { .. })) => {
                                    // Explicit cancel: stop at the next checkpoint
                                    // and acknowledge immediately so the user sees
                                    // a response the moment they press cancel.
                                    cancel.request_explicit();
                                    let _ = emit(out, &ServerMsg::Assistant {
                                        conversation_id: conversation.to_string(),
                                        text: "cancelling — stopping at the next safe point (a \
                                               running tool finishes first)".to_string(),
                                        seq: 0,
                                        speech: None,
                                        attention: None,
                                    });
                                }
                                Ok(Some(_)) => {} // other kinds mid-turn: ignore (MVP)
                                Ok(None) | Err(_) => {
                                    // Client went away mid-turn; wind down cleanly.
                                    client_open = false;
                                    cancel.request_triage();
                                }
                            },
                        }
                    };
                    // Handle a queued/interrupting message as a follow-up turn
                    // (not itself interruptible in this MVP).
                    let first_steps = match first_steps {
                        Ok(first_steps) => first_steps,
                        Err(error @ CoreError::Provider(_)) => {
                            finish_provider_turn_failure(
                                out,
                                storage,
                                device_id,
                                &conversation,
                                &error,
                                &pending
                                    .iter()
                                    .map(|(id, _, _)| id.clone())
                                    .collect::<Vec<_>>(),
                            )?;
                            0
                        }
                        Err(error) => return Err(error),
                    };
                    let mut completed_steps = first_steps;
                    while let Some((_message_id, t, atts)) = pending.pop_front() {
                        let um = if atts.is_empty() {
                            Message::user(t)
                        } else {
                            Message::user_with_attachments(t, atts)
                        };
                        let cancel2 = CancelFlag::new();
                        let mut gate2 = agent_core::AutoApprove;
                        match drive_to_goal(
                            out,
                            storage,
                            provider,
                            &tools,
                            policy,
                            device_id,
                            &conversation,
                            um,
                            &mut gate2,
                            &goal_state,
                            goal_max_continues,
                            voice,
                            &acting,
                            &cancel2,
                            &session_effort,
                            // A queued follow-up is a distinct message; it isn't
                            // auto-classified here. A manual pin still applies via
                            // the slot re-read inside drive_to_goal.
                            None,
                        )
                        .await
                        {
                            Ok(second_steps) => completed_steps += second_steps,
                            Err(error @ CoreError::Provider(_)) => {
                                finish_provider_turn_failure(
                                    out,
                                    storage,
                                    device_id,
                                    &conversation,
                                    &error,
                                    &pending
                                        .iter()
                                        .map(|(id, _, _)| id.clone())
                                        .collect::<Vec<_>>(),
                                )?;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    Ok(completed_steps)
                } else {
                    // Require-approval stays sequential because the gate owns
                    // inbound while awaiting a decision. Non-approval frames are
                    // deferred in FIFO order instead of being discarded.
                    let cancel = CancelFlag::new();
                    let mut gate = ConnGate {
                        out: out.clone(),
                        inbound,
                        deferred: &mut deferred_inbound,
                        deferred_bytes: &mut deferred_inbound_bytes,
                        conversation: &conversation,
                    };
                    drive_to_goal(
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
                        &cancel,
                        &session_effort,
                        turn_baseline,
                    )
                    .await
                };
                let steps = match steps {
                    Ok(steps) => steps,
                    Err(error @ CoreError::Provider(_)) => {
                        finish_provider_turn_failure(
                            out,
                            storage,
                            device_id,
                            &conversation,
                            &error,
                            &[],
                        )?;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                // Apply an explicit rollover the agent requested during the turn
                // (fast, storage-only): set the old aside, switch active, tell the
                // client. Transparent redirect handles clients that ignore it.
                apply_rollover(storage, out, &conversation, &acting, &rollover_state).await;

                // Stop hooks (origin device): the agent finished handling this
                // user message. Best-effort, audited; never blocks or alters the
                // reply already sent.
                if let Some(ctx) = &conv_hook_ctx {
                    let _ = run_conversation_event_hooks(
                        crate::hooks_compat::HookEvent::Stop,
                        ctx,
                        hub,
                        pending,
                        storage,
                        device_id,
                        &conversation,
                    )
                    .await;
                }

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
                            &[],
                            Vec::new(),
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
                    inbound,
                    out,
                    storage,
                    provider,
                    &tools,
                    policy,
                    device_id,
                    &conversation_id,
                    RecoveryEmission::ResumeReplay,
                    &mut deferred_inbound,
                    &mut deferred_inbound_bytes,
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
            ClientMsg::ConversationList { limit } => {
                // Scope is the acting user resolved from this connection's device
                // (owner, else the device's own unattributed conversations); the
                // query never crosses into another user's data. Clamp the count to
                // a sane window (default 20, 1..=50). Infallible query → an empty
                // list is honest; only the JSON encode can fail, falling back to
                // "[]" like the audit listing.
                let limit = limit.unwrap_or(20).clamp(1, 50) as usize;
                let conversations = storage.recent_conversations(device_id, limit);
                let conversations_json =
                    serde_json::to_string(&conversations).unwrap_or_else(|_| "[]".to_string());
                emit(
                    out,
                    &ServerMsg::ConversationListResult { conversations_json },
                )?;
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
            ClientMsg::ConfigExec { target, args } => {
                // Remote write ⇒ auth must be on. A server not requiring auth
                // accepts any connection, so a *mutating* config frame on it would
                // let an unauthenticated peer reconfigure a wide-open server —
                // refuse it (reads still served) and tell the operator to enable
                // auth first. When auth is on, the connection was verified at
                // Hello, so all config frames pass through as before.
                if remote_mutation_denied(&args, auth.required()) {
                    let reply = ServerMsg::ConfigResult {
                        ok: false,
                        output: String::new(),
                        effect: None,
                        error: Some(fleety_protocol::WireError {
                            kind: "unauthenticated".to_string(),
                            message: "this server does not require authentication, so remote \
                                      config changes are refused — enable auth before changing \
                                      settings remotely"
                                .to_string(),
                            remediation: Some(
                                "on the server host set FLEETY_REQUIRE_AUTH=1 (or `fleety-server \
                                 config set FLEETY_REQUIRE_AUTH 1`) and restart"
                                    .to_string(),
                            ),
                        }),
                    };
                    emit(out, &reply)?;
                } else {
                    let reply = match target {
                        fleety_protocol::ConfigTarget::Device(ref id) => {
                            daemon_config_exec(hub, pending, id, &args).await
                        }
                        _ => config_apply(target, &args),
                    };
                    // A successful mutation (effect present) is auditable — with
                    // secret values masked: the audit log is readable via
                    // `fleety audit`, so `set FLEETY_MODEL_KEY sk-…` must not land
                    // there in plaintext (list/get/edit all mask; so must this).
                    if let ServerMsg::ConfigResult {
                        ok: true,
                        effect: Some(_),
                        ..
                    } = &reply
                    {
                        let event = agent_core::Event::ToolResult {
                            id: "config".to_string(),
                            result: serde_json::json!({ "config": redact_config_args(&args) }),
                        };
                        let _ = storage.append_history(device_id, &event);
                    }
                    emit(out, &reply)?;
                }
            }
            ClientMsg::ConfigSnapshot { target } => {
                // Structured read of the server's settings for the interactive
                // panel. Only the Server target is snapshotted here; Local is the
                // CLI's own job and Device is a follow-up.
                let reply = match target {
                    fleety_protocol::ConfigTarget::Server => build_config_snapshot(),
                    fleety_protocol::ConfigTarget::Local => config_err(
                        "invalid",
                        "the local device's settings are edited by the CLI, not snapshotted \
                         from the server"
                            .to_string(),
                        None,
                    ),
                    fleety_protocol::ConfigTarget::Device(id) => {
                        daemon_config_snapshot(hub, pending, &id).await
                    }
                };
                emit(out, &reply)?;
            }
            ClientMsg::ConfigApply {
                target,
                base_revision,
                changes,
                providers_json,
            } => {
                let gate = structured_apply_gate(
                    &target,
                    &changes,
                    providers_json.as_deref(),
                    auth.required(),
                );
                let reply = match gate {
                    StructuredApplyGate::InvalidLocal => local_structured_apply_error(),
                    StructuredApplyGate::Denied => auth_disabled_structured_apply_error(),
                    StructuredApplyGate::Continue => match target {
                        fleety_protocol::ConfigTarget::Server => {
                            let (reply, applied, sensitive, providers_changed, provider_audit) =
                                apply_structured_changes(
                                    &base_revision,
                                    &changes,
                                    providers_json.as_deref(),
                                    auth.required(),
                                );
                            // Audit a successful mutation (with any sensitive keys),
                            // like the ConfigExec path — the audit log is readable via
                            // `fleety audit`, so record the keys (plus whether the
                            // provider config changed), never their values.
                            if applied > 0 || providers_changed {
                                let event = agent_core::Event::ToolResult {
                                    id: "config_apply".to_string(),
                                    result: serde_json::json!({
                                        "applied": applied,
                                        "sensitive": sensitive,
                                        "providers": providers_changed,
                                        "provider_changes": provider_audit,
                                    }),
                                };
                                let _ = storage.append_history(device_id, &event);
                            }
                            reply
                        }
                        fleety_protocol::ConfigTarget::Device(id) => {
                            daemon_config_apply(
                                hub,
                                pending,
                                &id,
                                &base_revision,
                                &changes,
                                providers_json.as_deref(),
                            )
                            .await
                        }
                        fleety_protocol::ConfigTarget::Local => local_structured_apply_error(),
                    },
                };
                emit(out, &reply)?;
            }
            ClientMsg::CredentialPut {
                kind,
                provider,
                payload_json,
            } => {
                // Deliver-and-store: the CLI ran the OAuth flow, this server owns
                // the credential from here (its own protected per-provider token
                // store, the same path that provider reads). Audited without token
                // values, with the provider named; a failed write is audited too so
                // a broken store is visible.
                let reply = credential_put(
                    &credential_token_path(provider.as_deref()),
                    &kind,
                    provider.as_deref(),
                    &payload_json,
                    auth.required(),
                );
                let op = match &reply {
                    ServerMsg::CredentialResult { ok: true, .. } => Some("put"),
                    ServerMsg::CredentialResult {
                        ok: false,
                        error: Some(e),
                        ..
                    } if e.kind == "io" => Some("put_failed"),
                    _ => None,
                };
                if let Some(op) = op {
                    let _ = storage.append_history(
                        device_id,
                        &credential_audit_event(op, &kind, provider.as_deref()),
                    );
                }
                emit(out, &reply)?;
            }
            ClientMsg::CredentialStatus { kind, provider } => {
                let reply = credential_status(
                    &credential_token_path(provider.as_deref()),
                    &kind,
                    provider.as_deref(),
                    auth.required(),
                );
                emit(out, &reply)?;
            }
            ClientMsg::ProviderModelList { provider } => {
                let reply = provider_model_list(&provider, auth.required()).await;
                emit(out, &reply)?;
            }
            ClientMsg::CredentialDelete { kind, provider } => {
                let reply = credential_delete(
                    &credential_token_path(provider.as_deref()),
                    &kind,
                    provider.as_deref(),
                    auth.required(),
                );
                if matches!(&reply, ServerMsg::CredentialResult { ok: true, .. }) {
                    let _ = storage.append_history(
                        device_id,
                        &credential_audit_event("delete", &kind, provider.as_deref()),
                    );
                }
                emit(out, &reply)?;
            }
            ClientMsg::MintPairingCode => {
                // The connection is already past Hello auth / loopback trust, so
                // no extra privilege check is needed. Codes only matter when auth
                // is required.
                emit(out, &mint_pairing_code(auth))?;
            }
            ClientMsg::Unknown => {
                // A frame this build doesn't recognize (a newer client's additive
                // frame). Reply with an unsupported error and KEEP the connection
                // — dropping it would make every future additive frame fatal
                // (design §8, M4).
                emit(
                    out,
                    &ServerMsg::Error {
                        error: fleety_protocol::WireError {
                            kind: "unsupported".to_string(),
                            message: "this server does not recognize that request frame \
                                      (it may be from a newer client)"
                                .to_string(),
                            remediation: Some(
                                "update the server, or use a matching client version".to_string(),
                            ),
                        },
                    },
                )?;
            }
            ClientMsg::CancelTurn { .. } => {
                // Reached only when no turn is in flight (the mid-turn select
                // loop consumes CancelTurn while one runs): ignore silently — a
                // cancel racing a just-finished turn must not produce a stray
                // message.
            }
            ClientMsg::Colocation {
                fingerprint,
                subnet,
                peers: _,
            } => {
                // A device's periodic co-location report. Gated on the device's
                // presence opt-in inside `apply_colocation`; unopted devices record
                // nothing. No reply frame — the site + timeline update silently.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = crate::presence::apply_colocation(
                    storage,
                    &storage.sites_dir(),
                    device_id,
                    fingerprint.as_deref(),
                    subnet.as_deref(),
                    now,
                );
            }
        }
    }

    Ok(())
}

/// Mask secret values in `ConfigExec` args before they enter the audit log:
/// `set <KEY> <value…>` where the registry marks KEY secret, and any value
/// following a `--key` flag in the provider subcommands.
fn redact_config_args(args: &[String]) -> Vec<String> {
    let mut out = args.to_vec();
    if out.first().map(String::as_str) == Some("set") {
        let secret = out
            .get(1)
            .and_then(|k| fleety_tools::config::find(k))
            .map(|s| s.secret)
            .unwrap_or(false);
        if secret {
            for v in out.iter_mut().skip(2) {
                *v = "********".to_string();
            }
        }
    }
    let mut mask_next = false;
    for v in out.iter_mut() {
        if mask_next {
            *v = "********".to_string();
        }
        mask_next = v == "--key";
    }
    out
}

/// Whether a remote `config` frame must be refused because it would *mutate*
/// settings on a server that does not require auth (reads are always allowed).
/// "Remote write ⇒ auth must be on": a wide-open server takes any connection, so
/// an unauthenticated peer must not be able to reconfigure it. Pure.
fn remote_mutation_denied(args: &[String], require_auth: bool) -> bool {
    fleety_tools::config::config_effect(args).is_some() && !require_auth
}

/// The one credential kind this server knows how to store today.
const CREDENTIAL_KIND_CODEX_OAUTH: &str = "codex-oauth";

/// The gate every credential frame passes first: auth must be on (a wide-open
/// server takes any connection, and credentials are the crown jewels), the kind
/// must be known, and a `codex-oauth` credential MUST name the provider it
/// belongs to (the store is per-provider). `None` means the request may proceed.
/// Pure. A `provider` that is absent or blank is treated the same — rejected — so
/// an empty name can never map to a stray `codex-oauth/.json`.
fn credential_gate(
    kind: &str,
    provider: Option<&str>,
    auth_required: bool,
) -> Option<fleety_protocol::WireError> {
    if !auth_required {
        return Some(fleety_protocol::WireError {
            kind: "unauthenticated".to_string(),
            message: "this server does not require authentication, so remote credential \
                      operations are refused — enable auth and pair this device first"
                .to_string(),
            remediation: Some(
                "on the server host set FLEETY_REQUIRE_AUTH=1 (or `fleety-server config set \
                 FLEETY_REQUIRE_AUTH 1`), restart, then `fleety pair <code>`"
                    .to_string(),
            ),
        });
    }
    if kind != CREDENTIAL_KIND_CODEX_OAUTH {
        return Some(fleety_protocol::WireError {
            kind: "unsupported".to_string(),
            message: format!(
                "unsupported credential kind '{kind}' — this server knows: {CREDENTIAL_KIND_CODEX_OAUTH}"
            ),
            remediation: Some("update the server, or use a matching client version".to_string()),
        });
    }
    // codex-oauth credentials are stored per provider: an old client that omits
    // the provider (or sends a blank one) is rejected with an update-your-CLI
    // message rather than falling back to a global write.
    let name = provider.map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Some(fleety_protocol::WireError {
            kind: "invalid".to_string(),
            message: "this server stores Codex credentials per provider — update fleety and \
                      sign in per provider with `fleety provider login <provider>`"
                .to_string(),
            remediation: Some(
                "update the fleety CLI, then run `fleety provider login <provider>`".to_string(),
            ),
        });
    }
    // The provider name becomes the per-provider token filename, so a name with a
    // path separator or traversal could escape the token directory (e.g.
    // `../agent/auth` clobbering the connection-auth store). Reject anything that
    // isn't a plain provider name — the wire value is untrusted.
    if !fleety_tools::providers_config::valid_provider_name(name) {
        return Some(fleety_protocol::WireError {
            kind: "invalid".to_string(),
            message: format!(
                "invalid provider name '{name}' — use only letters, digits, '.', '_' or '-'"
            ),
            remediation: Some("check the provider name and try again".to_string()),
        });
    }
    None
}

/// The per-provider token store path for a credential frame. `provider` is
/// guaranteed present past [`credential_gate`]; the `None` arm is an unreachable
/// placeholder (the gate rejects a codex-oauth frame without a provider before any
/// path is read) that keeps the caller total.
fn credential_token_path(provider: Option<&str>) -> std::path::PathBuf {
    match provider.map(str::trim) {
        // Use the trimmed name the gate validated, so the write path matches the
        // read side (`token_path_for`) exactly — no `codex-oauth/ name .json` skew.
        Some(p) if !p.is_empty() => fleety_tools::oauth::token_path_for(p),
        _ => fleety_tools::oauth::default_token_path(),
    }
}

/// Handle `CredentialPut`: validate the payload shape for `kind` and persist it
/// at `token_path` (the server's own protected store). Nothing is written on
/// any rejection. Path-injected so tests run against a temp file.
fn credential_put(
    token_path: &std::path::Path,
    kind: &str,
    provider: Option<&str>,
    payload_json: &str,
    auth_required: bool,
) -> ServerMsg {
    if let Some(error) = credential_gate(kind, provider, auth_required) {
        return ServerMsg::CredentialResult {
            ok: false,
            error: Some(error),
        };
    }
    let tokens: fleety_tools::oauth::Tokens = match serde_json::from_str(payload_json) {
        Ok(t) => t,
        Err(e) => {
            return ServerMsg::CredentialResult {
                ok: false,
                error: Some(fleety_protocol::WireError {
                    kind: "invalid".to_string(),
                    message: format!("malformed {kind} payload: {e}"),
                    remediation: Some(
                        "re-run `fleety provider login` on an up-to-date CLI".to_string(),
                    ),
                }),
            }
        }
    };
    match fleety_tools::oauth::save_tokens(token_path, &tokens) {
        Ok(()) => ServerMsg::CredentialResult {
            ok: true,
            error: None,
        },
        Err(e) => ServerMsg::CredentialResult {
            ok: false,
            error: Some(fleety_protocol::WireError {
                kind: "io".to_string(),
                message: format!("could not persist the credential: {}", e.report().message),
                remediation: None,
            }),
        },
    }
}

/// Handle `CredentialStatus`: presence and expiry only — token values never
/// enter the reply.
fn credential_status(
    token_path: &std::path::Path,
    kind: &str,
    provider: Option<&str>,
    auth_required: bool,
) -> ServerMsg {
    if let Some(error) = credential_gate(kind, provider, auth_required) {
        return ServerMsg::CredentialStatusResult {
            present: false,
            expires_at_secs: None,
            detail: None,
            error: Some(error),
        };
    }
    match fleety_tools::oauth::load_tokens(token_path) {
        Some(t) => ServerMsg::CredentialStatusResult {
            present: true,
            expires_at_secs: Some(t.expires_at_secs),
            detail: t.account_id.map(|a| format!("account {a}")),
            error: None,
        },
        None => ServerMsg::CredentialStatusResult {
            present: false,
            expires_at_secs: None,
            detail: None,
            error: None,
        },
    }
}

/// Handle the structured provider-model discovery request. Credentials remain
/// server-side: the reply contains only model IDs or a sanitized error.
async fn provider_model_list(provider_name: &str, auth_required: bool) -> ServerMsg {
    // Model discovery is a credential operation: it may attach a Server-owned
    // API key or OAuth token to an outbound request. An auth-disabled Server has
    // no authenticated owner boundary, so fail before config or network access.
    if !auth_required {
        return model_list_error(
            provider_name,
            "unauthenticated",
            "provider model discovery is unavailable while Server authentication is disabled",
            "enable FLEETY_REQUIRE_AUTH=1 on the Server, restart it, then retry",
        );
    }
    let provider_name = provider_name.trim();
    if !fleety_tools::providers_config::valid_provider_name(provider_name) {
        return model_list_error(
            provider_name,
            "invalid",
            "the provider name is invalid",
            "check the provider name and try again",
        );
    }
    let Some(config) = fleety_tools::providers_config::load() else {
        return model_list_error(
            provider_name,
            "config",
            "the server has no usable providers.toml",
            "configure the provider on the server, then retry",
        );
    };
    let Some(provider) = config.provider(provider_name) else {
        return model_list_error(
            provider_name,
            "not_found",
            &format!("provider '{provider_name}' is not configured on the server"),
            "select a configured provider and try again",
        );
    };
    match crate::providers::discover_provider_models(provider_name, provider).await {
        Ok(model_ids) if !model_ids.is_empty() => ServerMsg::ProviderModelListResult {
            provider: provider_name.to_string(),
            model_ids,
            error: None,
        },
        Ok(_) => model_list_error(
            provider_name,
            "empty",
            &format!("provider '{provider_name}' returned no model IDs"),
            "enter a model ID manually or check the provider catalog",
        ),
        Err(error) => {
            let report = error.report();
            let detail = report.message.to_ascii_lowercase();
            if detail.contains("not signed in") || detail.contains("provider login") {
                model_list_error(
                    provider_name,
                    "not_authenticated",
                    &format!("provider '{provider_name}' is not signed in"),
                    &format!("run `fleety provider login {provider_name}`, then retry"),
                )
            } else {
                tracing::warn!(provider = %provider_name, error = %report.message, "provider model discovery failed");
                let message = model_discovery_error_message(
                    provider_name,
                    provider.is_oauth(),
                    &report.message,
                );
                model_list_error(
                    provider_name,
                    "provider_model_list",
                    &message,
                    "enter a model ID manually or retry the catalog request",
                )
            }
        }
    }
}

fn model_discovery_error_message(provider: &str, oauth: bool, detail: &str) -> String {
    if oauth {
        format!("could not fetch models for provider '{provider}': {detail}")
    } else {
        format!("could not fetch models for provider '{provider}'")
    }
}

fn model_list_error(provider: &str, kind: &str, message: &str, remediation: &str) -> ServerMsg {
    ServerMsg::ProviderModelListResult {
        provider: provider.to_string(),
        model_ids: Vec::new(),
        error: Some(WireError {
            kind: kind.to_string(),
            message: message.to_string(),
            remediation: Some(remediation.to_string()),
        }),
    }
}

/// Handle `CredentialDelete`: remove the stored credential (idempotent — a
/// missing file is a successful logout).
fn credential_delete(
    token_path: &std::path::Path,
    kind: &str,
    provider: Option<&str>,
    auth_required: bool,
) -> ServerMsg {
    if let Some(error) = credential_gate(kind, provider, auth_required) {
        return ServerMsg::CredentialResult {
            ok: false,
            error: Some(error),
        };
    }
    match fleety_tools::oauth::clear_tokens(token_path) {
        Ok(()) => ServerMsg::CredentialResult {
            ok: true,
            error: None,
        },
        Err(e) => ServerMsg::CredentialResult {
            ok: false,
            error: Some(fleety_protocol::WireError {
                kind: "io".to_string(),
                message: format!("could not remove the credential: {}", e.report().message),
                remediation: None,
            }),
        },
    }
}

/// Mint a short-lived pairing code for enrolling another device (the same store
/// the first-run code and the `pair_create` tool use). Only meaningful when auth
/// is required; when it is disabled, codes aren't used — say so.
fn mint_pairing_code(auth: &AuthStore) -> ServerMsg {
    if !auth.required() {
        return ServerMsg::PairingCode {
            code: None,
            error: Some(fleety_protocol::WireError {
                kind: "auth_disabled".to_string(),
                message: "this server does not require authentication, so pairing codes are not \
                          used — devices can connect without one"
                    .to_string(),
                remediation: Some(
                    "enable auth first: set FLEETY_REQUIRE_AUTH=1 on the server and restart"
                        .to_string(),
                ),
            }),
        };
    }
    match auth.create_pairing() {
        Ok(code) => ServerMsg::PairingCode {
            code: Some(code),
            error: None,
        },
        Err(e) => ServerMsg::PairingCode {
            code: None,
            error: Some(fleety_protocol::WireError {
                kind: "io".to_string(),
                message: format!("could not mint a pairing code: {}", e.report().message),
                remediation: None,
            }),
        },
    }
}

/// The audit event for an accepted (or failed) credential operation. Carries
/// the operation and kind only — token values never reach the audit log.
fn credential_audit_event(op: &str, kind: &str, provider: Option<&str>) -> agent_core::Event {
    agent_core::Event::ToolResult {
        id: "credential".to_string(),
        result: serde_json::json!({
            "credential": { "op": op, "kind": kind, "provider": provider }
        }),
    }
}

/// A process-unique boot id so a config revision is invalidated across a server
/// restart (a stale snapshot from a previous boot never silently applies).
fn boot_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().simple().to_string())
}

/// The current config revision (config.toml content hash + boot id) for the
/// `ConfigApply` optimistic lock.
fn config_revision() -> String {
    // Fingerprints BOTH remote-config surfaces: providers edits invalidate
    // stale key snapshots and vice versa (one revision, no lost updates).
    format!(
        "{}:{}:{}",
        fleety_tools::config::revision(&fleety_tools::config::config_path()),
        fleety_tools::config::revision(&fleety_tools::providers_config::providers_path()),
        boot_id()
    )
}

fn wire_effect(e: fleety_tools::config::ConfigEffect) -> fleety_protocol::Effect {
    match e {
        fleety_tools::config::ConfigEffect::NextConnection => {
            fleety_protocol::Effect::NextConnection
        }
        fleety_tools::config::ConfigEffect::Restart => fleety_protocol::Effect::Restart,
    }
}

/// Keys whose overwrite could redirect data or credentials off-box — changing
/// one warrants an audit entry (and a client-side confirm).
fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key,
        "FLEETY_MODEL_KEY"
            | "FLEETY_MODEL_BASE_URL"
            | "FLEETY_BACKUP_REPO"
            | "FLEETY_BACKUP_TOKEN"
            | "FLEETY_CODEX_AUTHORIZE_URL"
            | "FLEETY_CODEX_TOKEN_URL"
            | "FLEETY_CODEX_BACKEND_URL"
    )
}

/// Build the structured config snapshot for the `Server` target: registry
/// settings as `ConfigEntry`s (secrets carry no value, only `is_set`) plus the
/// structured provider/model config, tagged with the current revision.
fn build_config_snapshot() -> ServerMsg {
    match fleety_tools::config::with_server_transaction(|| Ok(build_config_snapshot_unlocked())) {
        Ok(reply) => reply,
        Err(error) => {
            let report = error.report();
            config_err(&report.kind, report.message, report.remediation)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct ProviderMutationAudit {
    provider: String,
    old_host: Option<String>,
    new_host: Option<String>,
    key_rotated: bool,
}

fn provider_host(endpoint: Option<&str>) -> Option<String> {
    let url = reqwest::Url::parse(endpoint?).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn provider_mutation_audit(
    before: &fleety_tools::providers_config::ProvidersConfig,
    after: &fleety_tools::providers_config::ProvidersConfig,
) -> Vec<ProviderMutationAudit> {
    let names = before
        .providers
        .keys()
        .chain(after.providers.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    names
        .into_iter()
        .filter_map(|provider| {
            let old = before.providers.get(&provider);
            let new = after.providers.get(&provider);
            let old_endpoint = old.and_then(|entry| entry.base_url.as_deref());
            let new_endpoint = new.and_then(|entry| entry.base_url.as_deref());
            let key_rotated = old.and_then(|entry| entry.key.as_deref())
                != new.and_then(|entry| entry.key.as_deref());
            (old_endpoint != new_endpoint || key_rotated).then(|| ProviderMutationAudit {
                provider,
                old_host: provider_host(old_endpoint),
                new_host: provider_host(new_endpoint),
                key_rotated,
            })
        })
        .collect()
}

fn build_config_snapshot_unlocked() -> ServerMsg {
    let map = match fleety_tools::config::load_strict(&fleety_tools::config::config_path()) {
        Ok(map) => map,
        Err(e) => {
            let report = e.report();
            return config_err(&report.kind, report.message, report.remediation);
        }
    };
    let entries =
        fleety_tools::config::snapshot_entries(&map, Some(fleety_tools::config::SERVER_SCOPES))
            .into_iter()
            .map(|e| fleety_protocol::ConfigEntry {
                key: e.key.to_string(),
                scope: e.scope.as_str().to_string(),
                value: e.value,
                default: e.default.to_string(),
                description: e.description.to_string(),
                secret: e.secret,
                is_set: e.is_set,
                effect: e.effect.map(wire_effect),
                choices: e.choices.iter().map(|c| c.to_string()).collect(),
            })
            .collect();
    let providers = match fleety_tools::providers_config::load_or_default(
        &fleety_tools::providers_config::providers_path(),
    ) {
        Ok(config) => config,
        Err(e) => {
            let report = e.report();
            return config_err(&report.kind, report.message, report.remediation);
        }
    };
    if let Err(error) = fleety_tools::providers_config::validate(&providers) {
        let report = error.report();
        return config_err(&report.kind, report.message, report.remediation);
    }
    let providers_json = match provider_snapshot_json(&providers) {
        Ok(json) => json,
        Err(e) => {
            return config_err(
                "serialization",
                format!("could not serialize provider snapshot: {e}"),
                None,
            )
        }
    };
    ServerMsg::ConfigSnapshotResult {
        revision: config_revision(),
        entries,
        providers_json,
    }
}

/// Serialize the Provider/Model snapshot without ever letting a Provider key
/// leave its Server owner. `key_present` is enough for UI/status decisions and
/// is ignored by older serde readers as an additive top-level field.
fn provider_snapshot_json(
    providers: &fleety_tools::providers_config::ProvidersConfig,
) -> serde_json::Result<String> {
    let key_present = providers
        .providers
        .iter()
        .filter_map(|(name, provider)| provider.key.is_some().then_some(name.clone()))
        .collect::<Vec<_>>();
    let mut redacted = providers.clone();
    for provider in redacted.providers.values_mut() {
        provider.key = None;
    }
    let mut value = serde_json::to_value(redacted)?;
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "key_present".to_string(),
            serde_json::to_value(key_present)?,
        );
    }
    serde_json::to_string(&value)
}

/// A `ConfigResult` error frame with the given kind/message.
fn config_err(kind: &str, message: String, remediation: Option<String>) -> ServerMsg {
    ServerMsg::ConfigResult {
        ok: false,
        output: String::new(),
        effect: None,
        error: Some(fleety_protocol::WireError {
            kind: kind.to_string(),
            message,
            remediation,
        }),
    }
}

fn server_structured_apply_mutates(
    changes: &[fleety_protocol::ConfigChange],
    providers_json: Option<&str>,
) -> bool {
    use fleety_protocol::ChangeOp;
    providers_json.is_some()
        || changes
            .iter()
            .any(|change| !matches!(change.op, ChangeOp::Keep))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StructuredApplyGate {
    InvalidLocal,
    Denied,
    Continue,
}

/// Reject structured remote writes while the Server accepts unauthenticated
/// connections. A Device apply is always a write boundary: fleetyd persists
/// even an empty or Keep-only apply. Local applies retain their `invalid`
/// response and never reach a remote owner.
fn structured_apply_gate(
    target: &fleety_protocol::ConfigTarget,
    changes: &[fleety_protocol::ConfigChange],
    providers_json: Option<&str>,
    require_auth: bool,
) -> StructuredApplyGate {
    match target {
        fleety_protocol::ConfigTarget::Local => StructuredApplyGate::InvalidLocal,
        _ if require_auth => StructuredApplyGate::Continue,
        fleety_protocol::ConfigTarget::Server
            if server_structured_apply_mutates(changes, providers_json) =>
        {
            StructuredApplyGate::Denied
        }
        fleety_protocol::ConfigTarget::Server => StructuredApplyGate::Continue,
        fleety_protocol::ConfigTarget::Device(_) => StructuredApplyGate::Denied,
    }
}

fn local_structured_apply_error() -> ServerMsg {
    config_err(
        "invalid",
        "local settings are applied by the CLI, not over the connection".to_string(),
        None,
    )
}

fn auth_disabled_structured_apply_error() -> ServerMsg {
    config_err(
        "unauthenticated",
        "this server does not require authentication, so remote config changes are refused — \
         enable auth before changing settings remotely"
            .to_string(),
        Some("set FLEETY_REQUIRE_AUTH=1 on the server and restart".to_string()),
    )
}

const INTERNAL_CONFIG_TOOL: &str = "__fleety_internal_config";

fn daemon_config_error(value: &serde_json::Value) -> Option<ServerMsg> {
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    Some(config_err(
        value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("daemon_config"),
        value
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the daemon rejected the configuration request")
            .to_string(),
        None,
    ))
}

async fn daemon_config_request(
    hub: &Hub,
    pending: &Pending,
    device: &str,
    args: serde_json::Value,
) -> std::result::Result<serde_json::Value, ServerMsg> {
    crate::bridge::route_run_tool_to_device(hub, pending, device, INTERNAL_CONFIG_TOOL, args)
        .await
        .map_err(|e| {
            let report = e.report();
            config_err(&report.kind, report.message, report.remediation)
        })
}

async fn daemon_config_exec(
    hub: &Hub,
    pending: &Pending,
    device: &str,
    args: &[String],
) -> ServerMsg {
    let value = match daemon_config_request(
        hub,
        pending,
        device,
        serde_json::json!({ "op": "exec", "args": args }),
    )
    .await
    {
        Ok(value) => value,
        Err(reply) => return reply,
    };
    if let Some(error) = daemon_config_error(&value) {
        return error;
    }
    let effect = value
        .get("effect")
        .cloned()
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value(v).ok());
    ServerMsg::ConfigResult {
        ok: true,
        output: value
            .get("output")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        effect,
        error: None,
    }
}

async fn daemon_config_snapshot(hub: &Hub, pending: &Pending, device: &str) -> ServerMsg {
    let value = match daemon_config_request(
        hub,
        pending,
        device,
        serde_json::json!({ "op": "snapshot" }),
    )
    .await
    {
        Ok(value) => value,
        Err(reply) => return reply,
    };
    if let Some(error) = daemon_config_error(&value) {
        return error;
    }
    let entries = match serde_json::from_value(
        value
            .get("entries")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    ) {
        Ok(entries) => entries,
        Err(e) => {
            return config_err(
                "protocol",
                format!("daemon returned malformed config entries: {e}"),
                None,
            )
        }
    };
    let Some(revision) = value.get("revision").and_then(serde_json::Value::as_str) else {
        return config_err(
            "protocol",
            "daemon config snapshot omitted its revision".to_string(),
            None,
        );
    };
    ServerMsg::ConfigSnapshotResult {
        revision: revision.to_string(),
        entries,
        providers_json: String::new(),
    }
}

async fn daemon_config_apply(
    hub: &Hub,
    pending: &Pending,
    device: &str,
    base_revision: &str,
    changes: &[fleety_protocol::ConfigChange],
    providers_json: Option<&str>,
) -> ServerMsg {
    if providers_json.is_some() {
        return config_err(
            "wrong_owner",
            "provider and model configuration is owned by fleety-server".to_string(),
            None,
        );
    }
    let value = match daemon_config_request(
        hub,
        pending,
        device,
        serde_json::json!({
            "op": "apply",
            "base_revision": base_revision,
            "changes": changes,
        }),
    )
    .await
    {
        Ok(value) => value,
        Err(reply) => return reply,
    };
    if let Some(error) = daemon_config_error(&value) {
        return error;
    }
    let applied = value
        .get("applied")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    ServerMsg::ConfigResult {
        ok: true,
        output: format!("applied {applied} change(s) on daemon '{device}'"),
        effect: Some(fleety_protocol::Effect::Restart),
        error: None,
    }
}

/// Apply a structured `ConfigApply` to the server's config.toml: check the
/// optimistic-lock revision, refuse mutation when auth is off, then apply the
/// sparse tri-state changes (validate all before writing, one atomic save).
/// Returns the result plus (applied count, sensitive keys touched) for auditing.
fn apply_structured_changes(
    base_revision: &str,
    changes: &[fleety_protocol::ConfigChange],
    providers_json: Option<&str>,
    require_auth: bool,
) -> (
    ServerMsg,
    u32,
    Vec<String>,
    bool,
    Vec<ProviderMutationAudit>,
) {
    match fleety_tools::config::with_server_transaction(|| {
        Ok(apply_structured_changes_unlocked(
            base_revision,
            changes,
            providers_json,
            require_auth,
        ))
    }) {
        Ok(result) => result,
        Err(error) => {
            let report = error.report();
            (
                config_err(&report.kind, report.message, report.remediation),
                0,
                vec![],
                false,
                vec![],
            )
        }
    }
}

fn apply_structured_changes_unlocked(
    base_revision: &str,
    changes: &[fleety_protocol::ConfigChange],
    providers_json: Option<&str>,
    require_auth: bool,
) -> (
    ServerMsg,
    u32,
    Vec<String>,
    bool,
    Vec<ProviderMutationAudit>,
) {
    use fleety_protocol::ChangeOp;

    if base_revision != config_revision() {
        return (
            config_err(
                "conflict",
                "the server config changed since you loaded it; reload and reapply".to_string(),
                Some("re-open the panel to fetch the current settings".to_string()),
            ),
            0,
            vec![],
            false,
            vec![],
        );
    }
    let mutates = server_structured_apply_mutates(changes, providers_json);
    if mutates && !require_auth {
        return (
            auth_disabled_structured_apply_error(),
            0,
            vec![],
            false,
            vec![],
        );
    }
    if providers_json.is_some() && changes.iter().any(|c| !matches!(c.op, ChangeOp::Keep)) {
        return (
            config_err(
                "invalid",
                "one ConfigApply cannot mix flat settings with provider configuration".to_string(),
                Some("save settings and providers as separate owner transactions".to_string()),
            ),
            0,
            vec![],
            false,
            vec![],
        );
    }
    // Parse every provider change before touching either file. Provider secrets
    // are absent from snapshots, so validation happens after Server-side Keep
    // merging under this transaction lease.
    let parsed_providers = match providers_json {
        Some(json) => {
            let value: serde_json::Value = match serde_json::from_str(json) {
                Ok(value) => value,
                Err(e) => {
                    return (
                        config_err(
                            "invalid",
                            format!("malformed providers payload: {e}"),
                            Some("re-open the provider editor and save again".to_string()),
                        ),
                        0,
                        vec![],
                        false,
                        vec![],
                    )
                }
            };
            let clear_keys = match value.get("clear_keys") {
                None => std::collections::BTreeSet::new(),
                Some(serde_json::Value::Array(names)) => {
                    let mut clear_keys = std::collections::BTreeSet::new();
                    for name in names {
                        let Some(name) = name.as_str() else {
                            return (
                                config_err(
                                    "invalid",
                                    "clear_keys must contain only Provider names".to_string(),
                                    Some("re-open the provider editor and save again".to_string()),
                                ),
                                0,
                                vec![],
                                false,
                                vec![],
                            );
                        };
                        clear_keys.insert(name.to_string());
                    }
                    clear_keys
                }
                Some(_) => {
                    return (
                        config_err(
                            "invalid",
                            "clear_keys must be an array of Provider names".to_string(),
                            Some("re-open the provider editor and save again".to_string()),
                        ),
                        0,
                        vec![],
                        false,
                        vec![],
                    )
                }
            };
            let cfg: fleety_tools::providers_config::ProvidersConfig =
                match serde_json::from_value(value) {
                    Ok(config) => config,
                    Err(e) => {
                        return (
                            config_err(
                                "invalid",
                                format!("malformed providers payload: {e}"),
                                Some("re-open the provider editor and save again".to_string()),
                            ),
                            0,
                            vec![],
                            false,
                            vec![],
                        )
                    }
                };
            Some((cfg, clear_keys))
        }
        None => None,
    };
    let path = fleety_tools::config::config_path();
    let mut map = match fleety_tools::config::load_strict(&path) {
        Ok(m) => m,
        Err(e) => {
            let r = e.report();
            return (
                config_err(&r.kind, r.message, r.remediation),
                0,
                vec![],
                false,
                vec![],
            );
        }
    };
    let mut applied = 0u32;
    let mut sensitive = Vec::new();
    for ch in changes {
        match ch.op {
            ChangeOp::Keep => continue,
            ChangeOp::Set => {
                let Some(setting) = fleety_tools::config::find(&ch.key) else {
                    return (
                        config_err("invalid", format!("unknown setting '{}'", ch.key), None),
                        0,
                        vec![],
                        false,
                        vec![],
                    );
                };
                if let Err(e) =
                    fleety_tools::config::ensure_scope(&ch.key, fleety_tools::config::SERVER_SCOPES)
                {
                    let report = e.report();
                    return (
                        config_err(&report.kind, report.message, report.remediation),
                        0,
                        vec![],
                        false,
                        vec![],
                    );
                }
                let v = ch.value.clone().unwrap_or_default();
                if let Err(e) = fleety_tools::config::validate(setting, &v) {
                    let r = e.report();
                    return (
                        config_err(&r.kind, r.message, r.remediation),
                        0,
                        vec![],
                        false,
                        vec![],
                    );
                }
                map.insert((setting.scope, ch.key.clone()), v);
                if is_sensitive_key(&ch.key) {
                    sensitive.push(ch.key.clone());
                }
                applied += 1;
            }
            ChangeOp::Clear => {
                let Some(setting) = fleety_tools::config::find(&ch.key) else {
                    return (
                        config_err("invalid", format!("unknown setting '{}'", ch.key), None),
                        0,
                        vec![],
                        false,
                        vec![],
                    );
                };
                if let Err(e) =
                    fleety_tools::config::ensure_scope(&ch.key, fleety_tools::config::SERVER_SCOPES)
                {
                    let report = e.report();
                    return (
                        config_err(&report.kind, report.message, report.remediation),
                        0,
                        vec![],
                        false,
                        vec![],
                    );
                }
                map.remove(&(setting.scope, ch.key.clone()));
                if is_sensitive_key(&ch.key) {
                    sensitive.push(ch.key.clone());
                }
                applied += 1;
            }
        }
    }
    if applied > 0 {
        if let Err(e) = fleety_tools::config::save(&path, &map) {
            let r = e.report();
            return (
                config_err(&r.kind, r.message, r.remediation),
                0,
                vec![],
                false,
                vec![],
            );
        }
    }
    // Full provider write-back (config protocol 2): parse, then validate +
    // atomic write via the same path the local editor uses. Nothing lands on a
    // parse or validation failure.
    let mut providers_changed = false;
    let mut provider_audit = Vec::new();
    if let Some((mut cfg, clear_keys)) = parsed_providers {
        let before = match fleety_tools::providers_config::load_or_default(
            &fleety_tools::providers_config::providers_path(),
        ) {
            Ok(config) => config,
            Err(error) => {
                let report = error.report();
                return (
                    config_err(&report.kind, report.message, report.remediation),
                    applied,
                    sensitive,
                    false,
                    vec![],
                );
            }
        };
        // A missing key on an existing, same-kind Provider means Keep. A newly
        // supplied value means Set. Provider removal omits the entry entirely;
        // changing kind does not carry a key across incompatible auth schemes.
        for name in &clear_keys {
            let Some(provider) = cfg.providers.get(name) else {
                return (
                    config_err(
                        "invalid",
                        format!("cannot clear API key for missing Provider '{name}'"),
                        Some("re-open the provider editor and retry".to_string()),
                    ),
                    applied,
                    sensitive,
                    false,
                    vec![],
                );
            };
            if !fleety_tools::providers_config::provider_type(&provider.kind)
                .is_some_and(|kind| kind.allows_key)
            {
                return (
                    config_err(
                        "invalid",
                        format!("Provider '{name}' does not use an API key"),
                        Some("remove --clear-key or select an API Provider".to_string()),
                    ),
                    applied,
                    sensitive,
                    false,
                    vec![],
                );
            }
        }
        for (name, provider) in &mut cfg.providers {
            if clear_keys.contains(name) {
                provider.key = None;
            } else if provider.key.is_none() {
                if let Some(existing) = before.providers.get(name) {
                    if existing.kind == provider.kind {
                        provider.key.clone_from(&existing.key);
                    }
                }
            }
        }
        if let Err(e) = fleety_tools::providers_config::validate(&cfg) {
            let report = e.report();
            return (
                config_err(&report.kind, report.message, report.remediation),
                applied,
                sensitive,
                false,
                vec![],
            );
        }
        provider_audit = provider_mutation_audit(&before, &cfg);
        if let Err(e) = fleety_tools::providers_config::write_providers(
            &fleety_tools::providers_config::providers_path(),
            &cfg,
        ) {
            let r = e.report();
            return (
                config_err(&r.kind, r.message, r.remediation),
                applied,
                sensitive,
                false,
                vec![],
            );
        }
        providers_changed = true;
    }
    (
        ServerMsg::ConfigResult {
            ok: true,
            output: format!("applied {applied} change(s)"),
            effect: Some(fleety_protocol::Effect::Restart),
            error: None,
        },
        applied,
        sensitive,
        providers_changed,
        provider_audit,
    )
}

/// Run a remote `config` request against this (the server's) own config files
/// and wrap it as a `ConfigResult`. `Server` reuses the shared config logic
/// (rendered to text, the same code the local command prints) and tags when the
/// change takes effect; `Local` is the CLI's job (rejected here); `Device` is a
/// follow-up (rejected as unsupported). Never panics — a bad request becomes an
/// error result.
fn config_apply(target: fleety_protocol::ConfigTarget, args: &[String]) -> ServerMsg {
    use fleety_protocol::{ConfigTarget, Effect};
    match target {
        ConfigTarget::Server => match fleety_tools::config::with_server_transaction(|| {
            fleety_tools::config::run_rendered_scoped(
                args,
                Some(fleety_tools::config::SERVER_SCOPES),
            )
        }) {
            Ok(output) => {
                let effect = match fleety_tools::config::config_effect(args) {
                    Some(fleety_tools::config::ConfigEffect::NextConnection) => {
                        Some(Effect::NextConnection)
                    }
                    Some(fleety_tools::config::ConfigEffect::Restart) => Some(Effect::Restart),
                    None => None,
                };
                ServerMsg::ConfigResult {
                    ok: true,
                    output,
                    effect,
                    error: None,
                }
            }
            Err(e) => ServerMsg::ConfigResult {
                ok: false,
                output: String::new(),
                effect: None,
                error: Some(WireError {
                    kind: e.report().kind,
                    message: e.report().message,
                    remediation: e.report().remediation,
                }),
            },
        },
        ConfigTarget::Local => ServerMsg::ConfigResult {
            ok: false,
            output: String::new(),
            effect: None,
            error: Some(WireError {
                kind: "config".to_string(),
                message: "local config is handled by the CLI, not sent to the server".to_string(),
                remediation: None,
            }),
        },
        ConfigTarget::Device(id) => ServerMsg::ConfigResult {
            ok: false,
            output: String::new(),
            effect: None,
            error: Some(WireError {
                kind: "unsupported".to_string(),
                message: format!("per-device config ('{id}') is a follow-up change, not yet supported"),
                remediation: Some(
                    "configure the device on its own host with `fleetyd config`, or use --target server".to_string(),
                ),
            }),
        },
    }
}

/// Whether same-host loopback connections are trusted (bypass auth). On unless
/// `FLEETY_TRUST_LOOPBACK=0` — a loopback peer already has filesystem access to
/// the server's token/config, so requiring a token is friction, not security.
fn trust_loopback_enabled() -> bool {
    std::env::var("FLEETY_TRUST_LOOPBACK").as_deref() != Ok("0")
}

/// Whether a connection's transport peer is a loopback address. Taken from the
/// connection socket, never a client-supplied field. Pure.
pub(crate) fn peer_is_loopback(peer: Option<std::net::SocketAddr>) -> bool {
    peer.map(|a| a.ip().is_loopback()).unwrap_or(false)
}

/// Authenticate a Hello. `Ok(Some(token))` = pairing minted a token to return;
/// `Ok(None)` = already authenticated, auth disabled, or accepted on loopback
/// trust; `Err(msg)` = rejected. `loopback` is whether the transport peer is on
/// the same host (from the connection socket).
fn authenticate(
    auth: &AuthStore,
    device_id: &str,
    token: Option<&str>,
    pairing_code: Option<&str>,
    loopback: bool,
) -> std::result::Result<Option<String>, String> {
    if !auth.required() {
        return Ok(None);
    }
    // A valid token still wins even on loopback (so an enrolled device keeps its
    // bound identity); a token-less same-host client is trusted when enabled.
    if token.is_none() && pairing_code.is_none() && loopback && trust_loopback_enabled() {
        return Ok(None);
    }
    // A valid token wins — authenticate with it and leave any (possibly
    // already-redeemed) pairing code untouched. A daemon keeps `FLEETY_PAIRING_CODE`
    // set across reconnects; checking the code first would redeem-fail the used
    // code and reject an otherwise-authenticated device.
    if let Some(tok) = token {
        if auth.verify(tok).is_some() {
            return Ok(None);
        }
    }
    // No valid token: a pairing code enrolls the device and mints a token.
    if let Some(code) = pairing_code {
        return auth
            .redeem(code, device_id)
            .map(Some)
            .map_err(|e| e.report().message);
    }
    // A token was supplied but did not verify (and there was no pairing code).
    if token.is_some() {
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
    /// Whether the turn stopped at a cancellation checkpoint (the flag was set
    /// while it ran). Distinguishes a cancelled turn from a naturally short one.
    pub cancelled: bool,
}

/// The cancellation signal shared across one user turn. `stop` is the actual
/// "halt at the next checkpoint" flag — read between goal iterations here and,
/// handed to agent-core, before each tool/model call. `explicit` records *why*
/// it was set: a user-pressed cancel (`CancelTurn` / ACP `session/cancel`)
/// versus a triaged mid-turn interjection or a disconnect — so the closing
/// message can word it correctly. Two `AtomicBool`s rather than one `AtomicU8`
/// so `stop` hands straight to agent-core's `Option<&AtomicBool>` with no proxy.
#[derive(Default)]
pub(crate) struct CancelFlag {
    stop: std::sync::atomic::AtomicBool,
    explicit: std::sync::atomic::AtomicBool,
}

impl CancelFlag {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    /// The stop signal to hand to agent-core / check between iterations.
    fn stop_flag(&self) -> &std::sync::atomic::AtomicBool {
        &self.stop
    }
    fn is_stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn is_explicit(&self) -> bool {
        self.explicit.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// A triaged interjection or a client disconnect: stop, framed as switching
    /// to the new message.
    fn request_triage(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    /// An explicit user cancel (`CancelTurn` / ACP `session/cancel`).
    fn request_explicit(&self) {
        self.explicit
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
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
    cancel: &CancelFlag,
    session_effort: &crate::effort::SessionEffort,
    turn_baseline: Option<agent_core::model::Effort>,
) -> Result<usize> {
    // Fresh goal state for this user message.
    *goal_state.lock().await = GoalState::new();
    let mut next_msg = user_msg;
    let mut continues: u32 = 0;
    let mut total_steps: usize = 0;
    loop {
        // Re-read the agent's effort before EACH turn it drives — including every
        // goal-continuation turn of this same request — so a mid-request
        // `set_effort` takes effect on the next continuation instead of being
        // deferred to the next user message. Precedence: the manual pin (slot)
        // wins; otherwise this message's difficulty baseline; otherwise the
        // provider's configured default (with_effort(None)).
        let effective_effort = (*session_effort.lock().await).or(turn_baseline);
        let effort_provider = effective_effort.and_then(|e| provider.with_effort(Some(e)));
        let turn_provider: &dyn ModelProvider = effort_provider.as_deref().unwrap_or(provider);
        // Run silently; whether this turn is terminal is only known afterward.
        // Each turn runs with the message's voice flag so the terminal turn can
        // carry a spoken reply; intermediate turns' speech is discarded.
        let turn = drive_turn(
            out,
            storage,
            turn_provider,
            tools,
            policy,
            device_id,
            conversation,
            next_msg,
            gate,
            false,
            voice,
            acting,
            Some(cancel.stop_flag()),
        )
        .await?;
        total_steps += turn.steps;
        let (active, terminal) = {
            let mut g = goal_state.lock().await;
            (g.is_active(), g.take_terminal())
        };
        let premature = active && matches!(terminal, Terminal::None);
        let hit_cap = continues >= max_continues;
        // Cancellation checkpoint (between goal iterations): the flag is set by a
        // triaged `interrupt_now`, an explicit `CancelTurn`, or a disconnect;
        // `turn.cancelled` additionally catches a stop that agent-core took at a
        // per-tool-call checkpoint inside the turn just run. Either way we stop
        // after the current turn rather than starting another iteration; a
        // running tool is never interrupted mid-flight.
        let cancelled = cancel.is_stopped() || turn.cancelled;
        if !premature || hit_cap || cancelled {
            // Terminal turn: emit the real reply (and Done). On a cap stop with an
            // unmet goal, tell the user it may be incomplete.
            let text = if cancelled {
                // Distinct wording: an explicit user cancel vs. switching to a
                // triaged mid-turn message. A cancelled turn may have no partial
                // reply (stopped before any text), so the note can stand alone.
                let note = if cancel.is_explicit() {
                    "[Cancelled at your request — work completed so far is preserved.]"
                } else {
                    "[Stopped between steps to handle your new message.]"
                };
                if turn.reply.trim().is_empty() {
                    note.to_string()
                } else {
                    format!("{}\n\n{note}", turn.reply)
                }
            } else if premature && hit_cap {
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
/// Format collected instruction files into one injected block, or `None` when
/// nothing was collected.
fn format_instruction_files(files: &[crate::instructions::InstructionFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut s = String::from(
        "Project & user instruction files, auto-loaded for this workspace (deeper / more \
         specific files override shallower ones):",
    );
    for f in files {
        s.push_str(&format!(
            "\n\n===== {} =====\n{}",
            f.path.display(),
            f.content
        ));
    }
    Some(s)
}

/// Build the per-turn instruction-file preamble for a conversation from its
/// bound origin: the project-layer AGENTS.md / CLAUDE.md (cwd up to
/// `server_home`) plus the user-global files. Re-read every turn so edits
/// hot-reload, and never persisted. Same-host origins only here; cross-device
/// origins read via `device_exec` are wired separately. `None` when the
/// conversation has no usable origin.
fn build_instruction_preamble(
    storage: &Storage,
    conversation: &str,
    server_home: &std::path::Path,
) -> Option<String> {
    let binding = storage.conversation_workspace(conversation)?;
    if binding.device.is_some() {
        return None;
    }
    let cwd_str = binding.origin_cwd.as_deref()?;
    let cwd = std::path::Path::new(cwd_str);
    let paths = crate::instructions::collect_instruction_paths(server_home, cwd, server_home);
    let files = crate::instructions::read_instruction_files(
        &paths,
        crate::instructions::per_file_cap(),
        crate::instructions::total_cap(),
        |p| std::fs::read_to_string(p).ok(),
    );
    format_instruction_files(&files)
}

/// Cross-device version: when the conversation's origin is another device, read
/// that device's cwd `AGENTS.md` / `CLAUDE.md` via `device_exec` (best-effort —
/// an offline or failed read is skipped, never fatal). Re-read each turn like
/// the local path. `None` for same-host or no-origin conversations.
async fn build_instruction_preamble_remote(
    storage: &Storage,
    conversation: &str,
    tools: &ToolRegistry,
) -> Option<String> {
    let binding = storage.conversation_workspace(conversation)?;
    let device = binding.device.clone()?; // Some(_) → cross-device
    let cwd_str = binding.origin_cwd.as_deref()?;
    let cwd = std::path::Path::new(cwd_str);
    let mut items = Vec::new();
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = cwd.join(name);
        // `read_file` takes `path` (not `file`), and returns only the
        // line-numbered view, so the numbering is undone before injection —
        // instruction files must reach the model as their original text.
        let args = serde_json::json!({
            "device": device,
            "tool": "read_file",
            "args": { "path": path.to_string_lossy() },
        });
        if let Ok(res) = tools.call("device_exec", args).await {
            if let Some(numbered) = res.get("numbered").and_then(|c| c.as_str()) {
                items.push((path, fleety_tools::strip_line_numbers(numbered)));
            }
        }
    }
    let files = crate::instructions::cap_instruction_contents(
        items,
        crate::instructions::per_file_cap(),
        crate::instructions::total_cap(),
    );
    format_instruction_files(&files)
}

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
    cancel: Option<&std::sync::atomic::AtomicBool>,
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
    // Ephemeral per-turn origin context: which device + directory this
    // conversation was started from. Rebuilt each turn from the persisted
    // binding and never appended to the conversation, so long-context
    // compaction can't drop it. Same-host vs cross-device wording differs; a
    // conversation with no usable origin injects nothing.
    if let Some(origin) = storage
        .conversation_workspace(conversation)
        .and_then(|b| crate::workspace::origin_preamble(&b))
    {
        messages.push(Message::system(origin));
    }
    // Auto-loaded project & user instruction files, re-read each turn (so edits
    // hot-reload) and never persisted. Same-host origins only for now.
    if let Some(home) = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
    {
        if let Some(instr) =
            build_instruction_preamble(storage, conversation, std::path::Path::new(&home))
        {
            messages.push(Message::system(instr));
        }
    }
    // Cross-device origin: read the origin device's cwd instruction files via
    // device_exec (best-effort; offline / failed reads are skipped).
    if let Some(instr) = build_instruction_preamble_remote(storage, conversation, tools).await {
        messages.push(Message::system(instr));
    }
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
    // Progressive tool loading: open on the resident set plus whatever this
    // conversation has activated before. `tool_search` widens this handle during
    // the turn, and the loop re-reads the offered specs each step, so a group
    // found mid-turn is usable immediately.
    let active_tools = tools.active_tools();
    crate::tool_groups::apply_activation(
        &active_tools,
        storage.load_active_tools(device_id, conversation).as_ref(),
    );
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
        // Per-tool-call / per-model-call cancellation checkpoint. `None` (the
        // reflection turn) keeps run-to-completion behavior.
        cancel,
    )
    .await?;
    if let Some(cache) = &compaction {
        if let Err(e) = storage.save_compaction(device_id, conversation, cache) {
            tracing::warn!(%conversation, error = %e, "could not persist compaction cache");
        }
    }
    // Persist what the turn activated, so a group found once need not be found
    // again. Non-fatal: losing this only costs a future search.
    if let Some(active) = active_tools.snapshot() {
        if let Err(e) = storage.save_active_tools(device_id, conversation, &active) {
            tracing::warn!(%conversation, error = %e, "could not persist activated tools");
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
                // Load only messages added since the last index pass, not the whole
                // conversation (which would be O(conversation length) every turn).
                let watermark = crate::conversation_embed::open_index(&path)
                    .ok()
                    .map(|c| crate::conversation_embed::max_indexed_seq(&c, &conversation))
                    .unwrap_or(0);
                let msgs: Vec<crate::conversation_embed::IndexMsg> = storage
                    .load_user_conversation_after(&user, &conversation, watermark)
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
        cancelled: outcome.cancelled,
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
        // Reflection is not user-cancellable — run to completion.
        None,
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

fn finish_provider_turn_failure(
    out: &Out,
    storage: &Arc<Storage>,
    device_id: &str,
    conversation: &str,
    error: &CoreError,
    queued_message_ids: &[String],
) -> Result<()> {
    let report = error.report();
    tracing::warn!(
        %device_id,
        %conversation,
        report = ?report,
        "model provider failed during user turn"
    );
    let cleanup = storage.journal_end(device_id, conversation);
    if let Err(cleanup_error) = cleanup {
        tracing::warn!(
            %device_id,
            %conversation,
            report = ?cleanup_error.report(),
            "could not close failed turn journal"
        );
        let cleanup_report = cleanup_error.report();
        for message_id in queued_message_ids {
            emit(
                out,
                &ServerMsg::InterjectionAcknowledged {
                    conversation_id: conversation.to_string(),
                    message_id: message_id.clone(),
                    disposition: InterjectionDisposition::Rejected,
                    message: "the earlier queue acknowledgement was revoked because the failed turn could not be closed safely — reconnect and resend this message"
                        .to_string(),
                },
            )?;
        }
        emit(
            out,
            &ServerMsg::Error {
                error: WireError {
                    kind: "turn_cleanup_failed".to_string(),
                    message: format!(
                        "{} Fleety also could not close the failed turn journal: {}",
                        report.message, cleanup_report.message
                    ),
                    remediation: Some(
                        "Reconnect, then resend any queued messages before retrying the failed turn."
                            .to_string(),
                    ),
                },
            },
        )?;
        return Err(cleanup_error);
    }
    emit(
        out,
        &ServerMsg::Error {
            error: WireError {
                kind: report.kind,
                message: report.message,
                remediation: report.remediation,
            },
        },
    )?;
    emit(
        out,
        &ServerMsg::Done {
            conversation_id: conversation.to_string(),
        },
    )
}

/// On connect, proactively deliver each schedule outcome that has completed
/// since the owner was last notified — one `ServerMsg::Assistant` per outcome,
/// addressed to its `schedule-<id>` conversation, with failures prominently
/// marked and pointing at `fleety resume`.
///
/// Owner-scoped: only a connection whose acting user is the scheduler owner's
/// (and non-Guest) receives them; a Guest or a device owned by someone else gets
/// nothing (v0 single-owner model — schedules belong to the scheduler device's
/// owner). Best-effort throughout — a read, emit, or watermark-write failure for
/// one schedule is logged and skipped, never blocking the connection.
fn deliver_pending_schedule_notifications(storage: &Arc<Storage>, out: &Out, device_id: &str) {
    let acting = storage.acting_for_device(device_id);
    // Guest connections never receive schedule notifications.
    if acting == crate::identity::ActingUser::Guest {
        return;
    }
    // Only the scheduler owner's own devices (same acting user) are notified.
    if acting != storage.acting_for_device(crate::scheduler::SCHED_DEVICE) {
        return;
    }
    let dir = storage.schedules_dir();
    for pending in crate::schedules::pending_notifications(&dir) {
        let conversation = format!("schedule-{}", pending.id);
        let text = if pending.status == "error" {
            format!(
                "⚠ Schedule {} FAILED: {}\nResume with: fleety conversations resume {}",
                pending.id, pending.summary, conversation
            )
        } else {
            format!("Schedule {} ran OK: {}", pending.id, pending.summary)
        };
        if let Err(e) = emit(
            out,
            &ServerMsg::Assistant {
                conversation_id: conversation,
                text,
                seq: 0,
                speech: None,
                attention: None,
            },
        ) {
            tracing::warn!(schedule = %pending.id, report = ?e.report(), "could not deliver schedule notification");
            continue;
        }
        // Advance the watermark only after a successful emit, so an undelivered
        // outcome is retried on the next connect rather than lost.
        if let Err(e) = crate::schedules::mark_notified(&dir, &pending.id, pending.ts) {
            tracing::warn!(schedule = %pending.id, report = ?e.report(), "could not advance schedule notification watermark");
        }
    }
}

/// A client going away (close frame, reset, or a disconnect-shaped IO error) is
/// a normal end of connection, not an error.
#[cfg(test)]
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
    conversation_sources: &[std::path::PathBuf],
    conversation_mcp: Vec<crate::mcp::ServerCfg>,
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
        &storage.skills_synced_dir(),
        conversation_sources,
    );
    crate::web::register(&mut tools, &storage.cookies_dir(), workspace);
    crate::mcp::register(
        &mut tools,
        &storage.mcp_builtin_config_path(),
        &storage.mcp_installed_config_path(),
        conversation_mcp,
    );
    crate::wiki::register(
        &mut tools,
        &storage.wiki_dir(),
        &storage.models_dir(),
        &storage.backups_dir(),
    );
    crate::ssh::register(&mut tools);
    fleety_tools::register_browser(&mut tools);
    fleety_tools::register_computer(&mut tools);
    crate::sites::register(&mut tools, &storage.sites_dir(), &storage.devices_dir());
    crate::presence::register_presence(&mut tools, storage.home(), storage.sites_dir());
    auth::register(&mut tools, Arc::clone(auth));
    bridge::register(
        &mut tools,
        Arc::clone(hub),
        Arc::clone(pending),
        Arc::clone(handles),
        Arc::clone(device_tools),
    );
    // Cross-device file transfer: relays bytes between two endpoints (a device
    // or the server's own workspace). Uses the server's workspace root/backups
    // for the `server` endpoint.
    bridge::register_transfer(
        &mut tools,
        Arc::clone(hub),
        Arc::clone(pending),
        workspace.to_path_buf(),
        storage.backups_dir(),
    );
    // The model's route from the resident set to everything else. Holds the
    // registry's own activation handle, so a search widens what the next model
    // call of the same turn is shown.
    let active = tools.active_tools();
    tools.register(Box::new(crate::tool_groups::ToolSearch::new(active)));
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
        &[],
        Vec::new(),
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
/// second crash is still recoverable. Resume persists the result silently and
/// lets the following Replay stream deliver it; a live UserMessage emits the
/// recovered turn immediately before continuing with the new turn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecoveryEmission {
    LiveTurn,
    ResumeReplay,
}

#[allow(clippy::too_many_arguments)]
async fn recover_incomplete_turn(
    inbound: &mut dyn ClientInbound,
    out: &Out,
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    policy: Policy,
    device_id: &str,
    conversation: &str,
    emission: RecoveryEmission,
    deferred: &mut std::collections::VecDeque<ClientMsg>,
    deferred_bytes: &mut usize,
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

    let mut on_delta: Box<dyn FnMut(&str) + Send> = match emission {
        RecoveryEmission::LiveTurn => {
            let delta_out = out.clone();
            let delta_conv = conversation.to_string();
            Box::new(move |chunk: &str| {
                let frame = ServerMsg::AssistantDelta {
                    conversation_id: delta_conv.clone(),
                    chunk: chunk.to_string(),
                };
                if let Ok(json) = serde_json::to_string(&frame) {
                    let _ = delta_out.send(WsMessage::Text(json));
                }
            })
        }
        RecoveryEmission::ResumeReplay => Box::new(|_| {}),
    };

    let mut log = storage.journaling_log(device_id, conversation);
    let outcome = {
        let mut gate = ConnGate {
            out: out.clone(),
            inbound,
            deferred,
            deferred_bytes,
            conversation,
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
    if emission == RecoveryEmission::LiveTurn {
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
    }
    Ok(())
}

/// Approval gate that asks the connected client over the WebSocket and waits
/// for an Approve/Deny reply. Sequential within the connection, so it can read
/// the reply directly from `rx`.
struct ConnGate<'a> {
    out: Out,
    inbound: &'a mut dyn ClientInbound,
    deferred: &'a mut std::collections::VecDeque<ClientMsg>,
    deferred_bytes: &'a mut usize,
    conversation: &'a str,
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
            match self.inbound.next_client().await? {
                Some(ClientMsg::Approve { approval_id: id }) if id == approval_id => {
                    return Ok(ApprovalDecision::Approve)
                }
                Some(ClientMsg::Deny { approval_id: id }) if id == approval_id => {
                    return Ok(ApprovalDecision::Deny)
                }
                Some(other) => {
                    let incoming_bytes = client_message_wire_bytes(&other);
                    if pending_interjection_fits(
                        self.deferred.len(),
                        *self.deferred_bytes,
                        incoming_bytes,
                    ) {
                        *self.deferred_bytes = self.deferred_bytes.saturating_add(incoming_bytes);
                        self.deferred.push_back(other);
                    } else {
                        match other {
                            ClientMsg::UserMessage { message_id, .. } => emit(
                                &self.out,
                                &ServerMsg::InterjectionAcknowledged {
                                    conversation_id: self.conversation.to_string(),
                                    message_id,
                                    disposition: InterjectionDisposition::Rejected,
                                    message: "the interjection queue is full — resend after the current task"
                                        .to_string(),
                                },
                            )?,
                            _ => emit(
                                &self.out,
                                &ServerMsg::Error {
                                    error: WireError {
                                        kind: "approval_deferred_queue_full".to_string(),
                                        message: "the approval wait queue is full".to_string(),
                                        remediation: Some(
                                            "retry the request after the current approval is resolved"
                                                .to_string(),
                                        ),
                                    },
                                },
                            )?,
                        }
                    }
                }
                None => return Ok(ApprovalDecision::Deny),
            }
        }
    }
}

#[cfg(test)]
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

/// Inbound half of a client transport: yields the next `ClientMsg`, or `None` at
/// end-of-stream. The connection service loop ([`serve`]) and the approval gate
/// read through this, so the loop is agnostic to whether frames arrive over a
/// WebSocket or the SSE+POST fallback. The outbound half is already transport-
/// agnostic — everything emits into the [`Out`] channel and a per-transport
/// writer task drains it.
#[async_trait::async_trait]
pub(crate) trait ClientInbound: Send {
    /// The next client message, or `None` once the client half is closed.
    async fn next_client(&mut self) -> Result<Option<ClientMsg>>;
}

/// WebSocket inbound: reads `ClientMsg` frames off the split WS stream (test
/// harness; production uses the axum adapter in [`crate::http`]).
#[cfg(test)]
pub(crate) struct WsInbound {
    pub(crate) rx: Rx,
}

#[cfg(test)]
#[async_trait::async_trait]
impl ClientInbound for WsInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        read_client(&mut self.rx).await
    }
}

/// Outbound writer for a transport: sends one serialized frame (a `ServerMsg`
/// JSON string). The connection core drains the [`Out`] channel into this, so
/// WebSocket and the SSE+POST fallback differ only in how a frame reaches the
/// wire. Returns `false` once the transport is closed.
#[async_trait::async_trait]
pub(crate) trait FrameWriter: Send {
    async fn send_text(&mut self, text: String) -> bool;
}

/// WebSocket outbound: text frames over the split WS sink (test harness).
#[cfg(test)]
pub(crate) struct WsFrameWriter {
    pub(crate) tx: Tx,
}

#[cfg(test)]
#[async_trait::async_trait]
impl FrameWriter for WsFrameWriter {
    async fn send_text(&mut self, text: String) -> bool {
        self.tx.send(WsMessage::Text(text)).await.is_ok()
    }
}

/// Holds one already-read frame so the caller can look at the first frame and
/// still hand the stream on untouched.
pub(crate) struct PushbackInbound {
    inner: Box<dyn ClientInbound>,
    pending: Option<ClientMsg>,
}

#[async_trait::async_trait]
impl ClientInbound for PushbackInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        match self.pending.take() {
            Some(frame) => Ok(Some(frame)),
            None => self.inner.next_client().await,
        }
    }
}

/// Inbound half of an established secure channel: opens each sealed frame and
/// hands on the control frame inside it.
struct SecureInbound {
    inner: Box<dyn ClientInbound>,
    session: SharedSecureSession,
}

#[async_trait::async_trait]
impl ClientInbound for SecureInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        let Some(frame) = self.inner.next_client().await? else {
            return Ok(None);
        };
        // Once the channel is up, an unsealed control frame is a protocol
        // violation. Accepting one would hand an active relay the downgrade it
        // was denied at the handshake.
        let ClientMsg::SecureFrame { payload } = frame else {
            return Err(CoreError::Provider(
                "client sent an unsealed control frame on a secure channel".to_string(),
            ));
        };
        let opened = {
            let mut session = self.session.lock().map_err(|_| {
                CoreError::Provider("the secure channel state was lost".to_string())
            })?;
            session.open(&payload)?
        };
        serde_json::from_str(&opened).map(Some).map_err(|e| {
            CoreError::Provider(format!(
                "malformed client frame: {e}; expected a ClientMsg JSON object"
            ))
        })
    }
}

/// Outbound half of an established secure channel.
struct SecureFrameWriter {
    inner: Box<dyn FrameWriter>,
    session: SharedSecureSession,
}

#[async_trait::async_trait]
impl FrameWriter for SecureFrameWriter {
    async fn send_text(&mut self, text: String) -> bool {
        let sealed = {
            let Ok(mut session) = self.session.lock() else {
                return false;
            };
            match session.seal(&text) {
                Ok(payload) => payload,
                Err(_) => return false,
            }
        };
        match serde_json::to_string(&ServerMsg::SecureFrame { payload: sealed }) {
            Ok(frame) => self.inner.send_text(frame).await,
            Err(_) => false,
        }
    }
}

type SharedSecureSession = Arc<std::sync::Mutex<fleety_tools::secure::SecureSession>>;

/// Answer a secure-channel opening if that is what the client led with.
///
/// Returns the (possibly wrapped) halves. A client that does not offer a
/// handshake keeps the pre-existing cleartext path, which is what lets an
/// already-paired device reach a Server that is not yet updated; a client that
/// *does* offer one and cannot be answered is refused outright rather than
/// silently served in the clear.
async fn negotiate_secure_channel(
    inbound: Box<dyn ClientInbound>,
    mut writer: Box<dyn FrameWriter>,
    auth: &AuthStore,
) -> Result<(Box<dyn ClientInbound>, Box<dyn FrameWriter>, bool)> {
    let mut inbound = PushbackInbound {
        inner: inbound,
        pending: None,
    };
    let first = inbound.next_client().await?;
    let Some(ClientMsg::SecureHandshake {
        version,
        key_id,
        msg,
    }) = first
    else {
        inbound.pending = first;
        return Ok((Box::new(inbound), writer, false));
    };

    let fingerprint = crate::server_fingerprint();
    let answer =
        if version == fleety_tools::secure::SECURE_CHANNEL_VERSION && !fingerprint.is_empty() {
            auth.token_for_key_id(&key_id, fingerprint)
                .and_then(|token| {
                    fleety_tools::secure::Responder::accept(version, &token, fingerprint, &msg).ok()
                })
        } else {
            None
        };
    let Some((responder, reply)) = answer else {
        // One reason for every failure: which device tokens this Server holds,
        // and whether the handshake failed on the key or on the message, are
        // not things an unauthenticated peer gets to learn.
        send_error_frame(
            writer.as_mut(),
            "secure_unavailable",
            "this Server cannot open a secure channel for that device",
            "pair this device again with `fleety init` or `fleety pair <code>`",
        )
        .await;
        return Err(CoreError::Provider(
            "secure channel opening refused".to_string(),
        ));
    };

    let accept = serde_json::to_string(&ServerMsg::SecureAccept {
        version,
        msg: reply,
    })
    .map_err(|e| CoreError::Provider(format!("could not frame the secure acceptance: {e}")))?;
    if !writer.send_text(accept).await {
        return Err(CoreError::Provider(
            "client went away during the secure handshake".to_string(),
        ));
    }
    let session: SharedSecureSession = Arc::new(std::sync::Mutex::new(responder.finish()?));
    Ok((
        Box::new(SecureInbound {
            inner: Box::new(inbound),
            session: session.clone(),
        }),
        Box::new(SecureFrameWriter {
            inner: writer,
            session,
        }),
        true,
    ))
}

/// Send a `ServerMsg::Error` through a `FrameWriter` (used before the `Out`
/// channel exists — i.e. handshake/auth rejections). Best-effort.
async fn send_error_frame(
    writer: &mut dyn FrameWriter,
    kind: &str,
    message: &str,
    remediation: &str,
) {
    let error = WireError {
        kind: kind.to_string(),
        message: message.to_string(),
        remediation: Some(remediation.to_string()),
    };
    if let Ok(json) = serde_json::to_string(&ServerMsg::Error { error }) {
        writer.send_text(json).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::echo::EchoProvider;
    use agent_core::{Event, MockProvider, ModelResponse, Role as CoreRole, ToolCall, ToolSpec};
    use fleety_protocol::OriginContext;
    use tokio_tungstenite::MaybeTlsStream;

    type ClientWs = WebSocketStream<MaybeTlsStream<TcpStream>>;

    #[test]
    fn outcome_from_run_command_maps_exit_and_timeout() {
        use crate::hooks_compat::HookOutcome;
        assert_eq!(
            outcome_from_run_command(&serde_json::json!({ "exit_code": 0 })),
            HookOutcome::Exited(0)
        );
        assert_eq!(
            outcome_from_run_command(&serde_json::json!({ "exit_code": 2 })),
            HookOutcome::Exited(2)
        );
        assert!(matches!(
            outcome_from_run_command(&serde_json::json!({ "exit_code": null })),
            HookOutcome::Failed(_)
        ));
        assert!(
            matches!(
                outcome_from_run_command(&serde_json::json!({ "timed_out": true, "exit_code": 0 })),
                HookOutcome::Failed(_)
            ),
            "a timed-out origin run is a Failed outcome, not a spurious success"
        );
    }

    #[test]
    #[serial_test::serial]
    fn config_revision_covers_providers_and_apply_writes_them_back() {
        // Redirect both config files to a temp home (process-global env: serial).
        let dir = std::env::temp_dir().join(format!("fleety-provrev-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let config_path = dir.join("config.toml");
        let providers_path = dir.join("providers.toml");
        std::env::set_var("FLEETY_CONFIG", &config_path);
        std::env::set_var("FLEETY_PROVIDERS", &providers_path);

        // A provider change alone bumps the revision (stale snapshots of either
        // file are invalidated by edits to the other).
        let r0 = config_revision();
        std::fs::write(&providers_path, "x").expect("write");
        let r1 = config_revision();
        assert_ne!(r0, r1, "providers.toml content is part of the revision");
        std::fs::remove_file(&providers_path).expect("rm");

        // A full provider write-back through apply: valid config lands on disk.
        let providers = r#"{
            "providers": { "p1": { "type": "api", "base_url": "https://x/v1" } },
            "models": {}
        }"#;
        let rev = config_revision();
        let (reply, _applied, _sensitive, providers_changed, provider_audit) =
            apply_structured_changes(&rev, &[], Some(providers), true);
        match reply {
            ServerMsg::ConfigResult { ok, error, .. } => {
                assert!(ok, "valid providers write-back succeeds: {error:?}")
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(providers_changed);
        assert_eq!(provider_audit.len(), 1);
        assert_eq!(provider_audit[0].provider, "p1");
        assert_eq!(provider_audit[0].new_host.as_deref(), Some("x"));
        assert!(!provider_audit[0].key_rotated);
        assert!(providers_path.exists(), "providers.toml written");

        let rotated = r#"{
            "providers": {
                "p1": {
                    "type": "api",
                    "base_url": "https://new.example:9443/v1",
                    "key": "new-secret-key"
                }
            },
            "models": {}
        }"#;
        let revision = config_revision();
        let (reply, _, _, changed, audit) =
            apply_structured_changes(&revision, &[], Some(rotated), true);
        assert!(matches!(reply, ServerMsg::ConfigResult { ok: true, .. }));
        assert!(changed);
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].old_host.as_deref(), Some("x"));
        assert_eq!(audit[0].new_host.as_deref(), Some("new.example:9443"));
        assert!(audit[0].key_rotated);
        let audit_json = serde_json::to_string(&audit).expect("serialize provider audit");
        assert!(!audit_json.contains("new-secret-key"), "audit leaked key");

        // Two editors applying the same revision concurrently cannot both win:
        // the revision check and write share the Server transaction lease.
        let concurrent_revision = config_revision();
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = ["one", "two"]
            .into_iter()
            .map(|name| {
                let start = std::sync::Arc::clone(&start);
                let revision = concurrent_revision.clone();
                std::thread::spawn(move || {
                    let payload = format!(
                        r#"{{"providers":{{"{name}":{{"type":"api","base_url":"https://{name}.example/v1"}}}},"models":{{}}}}"#
                    );
                    start.wait();
                    apply_structured_changes(&revision, &[], Some(&payload), true).0
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let replies = workers
            .into_iter()
            .map(|worker| worker.join().expect("config apply worker"))
            .collect::<Vec<_>>();
        let successful = replies
            .iter()
            .filter(|reply| matches!(reply, ServerMsg::ConfigResult { ok: true, .. }))
            .count();
        let conflicts = replies
            .iter()
            .filter(|reply| {
                matches!(
                    reply,
                    ServerMsg::ConfigResult {
                        ok: false,
                        error: Some(error),
                        ..
                    } if error.kind == "conflict"
                )
            })
            .count();
        assert_eq!(successful, 1, "exactly one same-revision apply wins");
        assert_eq!(conflicts, 1, "the other same-revision apply conflicts");

        // A single request may not span both owner files. Reject it before
        // touching either file so an I/O failure can never leave a half-apply.
        std::fs::write(&config_path, "[server]\nFLEETY_ADDR = \"before:8787\"\n")
            .expect("seed config");
        let config_before = std::fs::read(&config_path).expect("read config bytes");
        let providers_before = std::fs::read(&providers_path).expect("read provider bytes");
        let rev = config_revision();
        let mixed = [fleety_protocol::ConfigChange {
            key: "FLEETY_ADDR".to_string(),
            op: fleety_protocol::ChangeOp::Set,
            value: Some("after:8787".to_string()),
        }];
        let (reply, applied, _, changed, _) =
            apply_structured_changes(&rev, &mixed, Some(providers), true);
        match reply {
            ServerMsg::ConfigResult { ok, error, .. } => {
                assert!(!ok);
                assert_eq!(error.expect("mixed apply error").kind, "invalid");
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert_eq!(applied, 0);
        assert!(!changed);
        assert_eq!(
            std::fs::read(&config_path).expect("config unchanged"),
            config_before
        );
        assert_eq!(
            std::fs::read(&providers_path).expect("providers unchanged"),
            providers_before
        );

        // Malformed JSON is rejected without touching the file.
        let before = std::fs::read_to_string(&providers_path).expect("read");
        let rev = config_revision();
        let (reply, _, _, changed, _) =
            apply_structured_changes(&rev, &[], Some("{not json"), true);
        match reply {
            ServerMsg::ConfigResult { ok, .. } => assert!(!ok),
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!changed);
        assert_eq!(
            std::fs::read_to_string(&providers_path).expect("read"),
            before,
            "rejected payload leaves the file untouched"
        );

        // A stale revision (taken before a provider edit) conflicts and writes
        // nothing.
        let stale = config_revision();
        std::fs::write(&providers_path, "# hand edit").expect("write");
        let (reply, _, _, changed, _) =
            apply_structured_changes(&stale, &[], Some(providers), true);
        match reply {
            ServerMsg::ConfigResult { ok, error, .. } => {
                assert!(!ok);
                assert_eq!(error.expect("conflict").kind, "conflict");
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!changed);

        // Auth-off server refuses a providers write-back (it mutates).
        let rev = config_revision();
        let (reply, _, _, changed, _) = apply_structured_changes(&rev, &[], Some(providers), false);
        match reply {
            ServerMsg::ConfigResult { ok, error, .. } => {
                assert!(!ok);
                assert_eq!(error.expect("gate").kind, "unauthenticated");
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!changed);

        std::env::remove_var("FLEETY_CONFIG");
        std::env::remove_var("FLEETY_PROVIDERS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mint_pairing_code_gated_on_auth() {
        // Auth required → a code is minted (and is redeemable in-memory).
        let path = std::env::temp_dir().join(format!("fleety-mint-{}.json", uuid::Uuid::new_v4()));
        let auth = AuthStore::load(path, None, true);
        match mint_pairing_code(&auth) {
            ServerMsg::PairingCode {
                code: Some(code),
                error: None,
            } => {
                assert!(!code.is_empty());
                // The freshly minted code redeems (same in-memory store).
                assert!(auth.redeem(&code, "dev").is_ok());
            }
            other => panic!("expected a minted code, got {other:?}"),
        }

        // Auth disabled → no code, an actionable error.
        let path2 = std::env::temp_dir().join(format!("fleety-mint-{}.json", uuid::Uuid::new_v4()));
        let auth_off = AuthStore::load(path2, None, false);
        match mint_pairing_code(&auth_off) {
            ServerMsg::PairingCode {
                code: None,
                error: Some(e),
            } => {
                assert_eq!(e.kind, "auth_disabled");
                assert!(e.remediation.is_some());
            }
            other => panic!("expected an auth-disabled error, got {other:?}"),
        }
    }

    #[test]
    fn credential_put_status_delete_lifecycle() {
        let dir = std::env::temp_dir().join(format!("fleety-cred-{}", uuid::Uuid::new_v4()));
        let path = dir.join("codex-oauth.json");
        let valid = r#"{"access_token":"at-1","refresh_token":"rt-1","expires_at_secs":123,"account_id":"acc-1"}"#;

        // Auth-off server refuses every credential operation and stores nothing.
        match credential_put(&path, "codex-oauth", Some("codex-a"), valid, false) {
            ServerMsg::CredentialResult { ok, error } => {
                assert!(!ok);
                let e = error.expect("gate error");
                assert_eq!(e.kind, "unauthenticated");
                assert!(e.remediation.is_some(), "gate names the remedy");
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!path.exists());
        match credential_status(&path, "codex-oauth", Some("codex-a"), false) {
            ServerMsg::CredentialStatusResult { present, error, .. } => {
                assert!(!present);
                assert!(error.is_some(), "status is refused on an auth-off server");
            }
            other => panic!("unexpected reply {other:?}"),
        }

        // Unknown kind is rejected by name; nothing is stored.
        match credential_put(&path, "something-else", Some("codex-a"), valid, true) {
            ServerMsg::CredentialResult { ok, error } => {
                assert!(!ok);
                let e = error.expect("kind error");
                assert_eq!(e.kind, "unsupported");
                assert!(e.message.contains("something-else"));
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!path.exists());

        // A codex-oauth frame with no provider is rejected (per-provider store).
        match credential_put(&path, "codex-oauth", None, valid, true) {
            ServerMsg::CredentialResult { ok, error } => {
                assert!(!ok);
                let e = error.expect("provider error");
                assert_eq!(e.kind, "invalid");
                assert!(e.message.contains("per provider"));
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!path.exists());

        // A path-traversal provider name is rejected before any write — it must
        // never escape the codex-oauth/ directory (e.g. clobber the auth store).
        match credential_put(&path, "codex-oauth", Some("../agent/auth"), valid, true) {
            ServerMsg::CredentialResult { ok, error } => {
                assert!(!ok);
                let e = error.expect("provider error");
                assert_eq!(e.kind, "invalid");
                assert!(e.message.contains("invalid provider name"));
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!path.exists());

        // A payload missing a required field is rejected naming the field, and
        // has no side effects.
        match credential_put(
            &path,
            "codex-oauth",
            Some("codex-a"),
            r#"{"access_token":"a"}"#,
            true,
        ) {
            ServerMsg::CredentialResult { ok, error } => {
                assert!(!ok);
                assert!(error
                    .expect("payload error")
                    .message
                    .contains("refresh_token"));
            }
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!path.exists());

        // A valid put persists exactly the delivered tokens.
        match credential_put(&path, "codex-oauth", Some("codex-a"), valid, true) {
            ServerMsg::CredentialResult { ok, error } => {
                assert!(ok, "valid put succeeds: {error:?}");
            }
            other => panic!("unexpected reply {other:?}"),
        }
        let stored = fleety_tools::oauth::load_tokens(&path).expect("tokens stored");
        assert_eq!(stored.access_token, "at-1");
        assert_eq!(stored.refresh_token, "rt-1");
        assert_eq!(stored.expires_at_secs, 123);
        assert_eq!(stored.account_id.as_deref(), Some("acc-1"));

        // Status reports presence + expiry and, by serialized shape, carries no
        // token material.
        let status = credential_status(&path, "codex-oauth", Some("codex-a"), true);
        match &status {
            ServerMsg::CredentialStatusResult {
                present,
                expires_at_secs,
                error,
                ..
            } => {
                assert!(present);
                assert_eq!(*expires_at_secs, Some(123));
                assert!(error.is_none());
            }
            other => panic!("unexpected reply {other:?}"),
        }
        let json = serde_json::to_string(&status).expect("ser");
        assert!(!json.contains("at-1") && !json.contains("rt-1"));

        // Delete removes the file and is idempotent.
        match credential_delete(&path, "codex-oauth", Some("codex-a"), true) {
            ServerMsg::CredentialResult { ok, .. } => assert!(ok),
            other => panic!("unexpected reply {other:?}"),
        }
        assert!(!path.exists());
        match credential_delete(&path, "codex-oauth", Some("codex-a"), true) {
            ServerMsg::CredentialResult { ok, .. } => assert!(ok, "idempotent delete"),
            other => panic!("unexpected reply {other:?}"),
        }

        // The audit event names the operation, kind, and provider, never tokens.
        let ev = credential_audit_event("put", "codex-oauth", Some("codex-a"));
        let ev_json = serde_json::to_string(&ev).expect("ser");
        assert!(ev_json.contains("credential"));
        assert!(ev_json.contains("put") && ev_json.contains("codex-oauth"));
        assert!(ev_json.contains("codex-a"), "audit names the provider");
        assert!(!ev_json.contains("at-1") && !ev_json.contains("access_token"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credentials_are_isolated_per_provider() {
        // Two providers, two token files: a put/delete on one never touches the
        // other (verified through the path-injected helpers with distinct paths).
        let dir = std::env::temp_dir().join(format!("fleety-cred2-{}", uuid::Uuid::new_v4()));
        let path_a = dir.join("codex-oauth").join("a.json");
        let path_b = dir.join("codex-oauth").join("b.json");
        let toks = |acc: &str| {
            format!(
                r#"{{"access_token":"at-{acc}","refresh_token":"rt-{acc}","expires_at_secs":9,"account_id":"{acc}"}}"#
            )
        };

        assert!(matches!(
            credential_put(&path_a, "codex-oauth", Some("a"), &toks("acct-a"), true),
            ServerMsg::CredentialResult { ok: true, .. }
        ));
        assert!(matches!(
            credential_put(&path_b, "codex-oauth", Some("b"), &toks("acct-b"), true),
            ServerMsg::CredentialResult { ok: true, .. }
        ));
        // Each file holds its own account.
        assert_eq!(
            fleety_tools::oauth::load_tokens(&path_a)
                .unwrap()
                .account_id
                .as_deref(),
            Some("acct-a")
        );
        assert_eq!(
            fleety_tools::oauth::load_tokens(&path_b)
                .unwrap()
                .account_id
                .as_deref(),
            Some("acct-b")
        );
        // Deleting a leaves b present.
        assert!(matches!(
            credential_delete(&path_a, "codex-oauth", Some("a"), true),
            ServerMsg::CredentialResult { ok: true, .. }
        ));
        assert!(!path_a.exists());
        match credential_status(&path_b, "codex-oauth", Some("b"), true) {
            ServerMsg::CredentialStatusResult { present, .. } => {
                assert!(present, "b's credential survives a's deletion")
            }
            other => panic!("unexpected reply {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn collect_conversation_hooks_same_host_reads_local_settings() {
        let home = std::env::temp_dir().join(format!("fleety-connhook-{}", uuid::Uuid::new_v4()));
        let proj = home.join("proj");
        std::fs::create_dir_all(proj.join(".claude")).expect("mk proj/.claude");
        std::fs::write(
            proj.join(".claude").join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"p.sh"}]}]}}"#,
        )
        .expect("w settings");
        let hub = crate::bridge::new_hub();
        let pending = crate::bridge::new_pending();
        // device None → same-host: reads local project + user settings, no hub use.
        let got =
            collect_conversation_hooks(None, proj.to_str(), None, home.to_str(), &hub, &pending)
                .await;
        assert_eq!(got.len(), 1, "the project hook is collected");
        assert_eq!(got[0].command, "p.sh");
        assert_eq!(got[0].scope, crate::hooks_compat::HookScope::Project);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The cross-device hook read must ask `read_file` for a `path` and cope with
    /// its line-numbered reply. The fake device here asserts the argument key and
    /// answers in `read_file`'s real shape, so a wrong key or a wrong result field
    /// fails the test instead of silently yielding no hooks.
    #[tokio::test]
    async fn collect_conversation_hooks_cross_device_reads_settings_via_bridge() {
        use fleety_protocol::ServerMsg;

        let hub = crate::bridge::new_hub();
        let pending = crate::bridge::new_pending();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        hub.lock().await.insert("dev2".to_string(), tx);

        // Stand in for the device: answer every RunTool as `read_file` would.
        let device_pending = pending.clone();
        let seen_paths = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorder = Arc::clone(&seen_paths);
        let device = tokio::spawn(async move {
            while let Some(WsMessage::Text(text)) = rx.recv().await {
                let Ok(ServerMsg::RunTool {
                    call_id,
                    tool,
                    args_json,
                }) = serde_json::from_str::<ServerMsg>(&text)
                else {
                    continue;
                };
                assert_eq!(tool, "read_file");
                let args: serde_json::Value = serde_json::from_str(&args_json).expect("args parse");
                let path = args
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("read_file requires a 'path' argument; got {args_json}")
                    })
                    .to_string();
                recorder.lock().expect("lock").push(path.clone());
                // Only the project settings exist; the user one is missing.
                let reply = if path.contains("proj") {
                    let body = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"x.sh"}]}]}}"#;
                    Ok(serde_json::json!({
                        "path": path,
                        "numbered": fleety_tools::line_numbered(body, 1),
                        "line_count": 1,
                    }))
                } else {
                    Err("no such file".to_string())
                };
                if let Some(slot) = device_pending.lock().await.remove(&call_id) {
                    let _ = slot.send(reply);
                }
            }
        });

        let got = collect_conversation_hooks(
            Some("dev2"),
            Some("/home/bob/proj"),
            Some("/home/bob"),
            None,
            &hub,
            &pending,
        )
        .await;

        device.abort();
        assert_eq!(got.len(), 1, "the project hook is parsed: {got:?}");
        assert_eq!(got[0].command, "x.sh");
        assert_eq!(got[0].scope, crate::hooks_compat::HookScope::Project);
        let paths = seen_paths.lock().expect("lock").clone();
        assert_eq!(paths.len(), 2, "project and user settings both attempted");
        assert!(
            paths.iter().all(|p| p.ends_with("settings.json")),
            "{paths:?}"
        );
    }

    /// Build the registry a real connection gets, for tests that need the whole
    /// tool surface rather than the base one.
    fn full_registry_for_test(home: &std::path::Path) -> (Arc<Storage>, ToolRegistry) {
        let storage = Arc::new(Storage::new(home.to_path_buf()));
        let workspace = std::env::current_dir().expect("cwd");
        let tools = build_full_registry(
            &storage,
            &workspace,
            "dev1",
            &crate::bridge::new_hub(),
            &crate::bridge::new_pending(),
            &crate::bridge::new_handles(),
            &Arc::new(crate::auth::AuthStore::load(
                home.join("auth.toml"),
                None,
                false,
            )),
            &crate::bridge::new_device_tools(),
            &[],
            Vec::new(),
        );
        (storage, tools)
    }

    /// The resident set and the groups must between them name every registered
    /// tool, and name nothing that is not registered. Without this, adding a tool
    /// would silently make it unreachable — the model would never be shown it and
    /// no search would ever surface it.
    #[tokio::test]
    async fn groups_cover_every_registered_tool() {
        use std::collections::BTreeSet;
        let home = std::env::temp_dir().join(format!("fleety-groups-{}", uuid::Uuid::new_v4()));
        let (_storage, tools) = full_registry_for_test(&home);

        let registered: BTreeSet<String> =
            tools.specs().into_iter().map(|spec| spec.name).collect();
        let mut classified: BTreeSet<String> = crate::tool_groups::RESIDENT
            .iter()
            .map(|t| t.to_string())
            .collect();
        for group in crate::tool_groups::GROUPS {
            classified.extend(group.tools.iter().map(|t| t.to_string()));
        }
        // The search entry point is resident by construction, never in a group.
        classified.insert(crate::tool_groups::SEARCH_TOOL.to_string());

        let unclassified: Vec<&String> = registered.difference(&classified).collect();
        let phantom: Vec<&String> = classified.difference(&registered).collect();
        assert!(
            unclassified.is_empty(),
            "registered tools missing from RESIDENT/GROUPS: {unclassified:?}"
        );
        assert!(
            phantom.is_empty(),
            "RESIDENT/GROUPS name tools that are not registered: {phantom:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Measure the whole per-request harness footprint: the system prompt plus
    /// every tool schema the model is shown. Run with `--nocapture` to print it.
    ///
    /// A measurement, not a budget assertion — it exists so the cost of the
    /// context is a number someone can look up instead of estimating.
    #[tokio::test]
    async fn measure_harness_footprint() {
        let home = std::env::temp_dir().join(format!("fleety-footprint-{}", uuid::Uuid::new_v4()));
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = std::env::current_dir().expect("cwd");
        let tools = build_full_registry(
            &storage,
            &workspace,
            "dev1",
            &crate::bridge::new_hub(),
            &crate::bridge::new_pending(),
            &crate::bridge::new_handles(),
            &Arc::new(crate::auth::AuthStore::load(
                home.join("auth.toml"),
                None,
                false,
            )),
            &crate::bridge::new_device_tools(),
            &[],
            Vec::new(),
        );
        let wire = |specs: &[agent_core::ToolSpec]| -> usize {
            specs
                .iter()
                .map(|s| s.description.len() + s.parameters.to_string().len() + s.name.len() + 60)
                .sum()
        };
        // Everything registered — what every request cost before this change.
        let registered = tools.specs();
        let registered_bytes = wire(&registered);
        // What a fresh conversation is actually offered.
        crate::tool_groups::apply_activation(&tools.active_tools(), None);
        let offered = tools.specs();
        let offered_bytes = wire(&offered);

        let prompt = storage
            .system_prompt_for(&storage.acting_for_device("dev1"))
            .expect("system prompt");
        let total = prompt.len() + offered_bytes;

        println!("\n=== per-request harness footprint ===");
        println!(
            "system prompt   : {} chars / {} bytes",
            prompt.chars().count(),
            prompt.len()
        );
        println!(
            "tools offered   : {} tools / {offered_bytes} bytes",
            offered.len()
        );
        println!(
            "tools registered: {} tools / {registered_bytes} bytes (deferred until searched)",
            registered.len()
        );
        println!("total per call  : {total} bytes (~{} KB)", total / 1024);
        println!("est. tokens     : ~{} (bytes/4)", total / 4);
        println!(
            "schema deferred : {} bytes (~{} tokens)",
            registered_bytes - offered_bytes,
            (registered_bytes - offered_bytes) / 4
        );

        assert!(
            registered.len() > 50,
            "full registry should be the whole surface"
        );
        assert!(
            offered.len() < registered.len(),
            "a fresh conversation is offered less than the whole registry"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    // A minimal tool to wrap in the hook-registry tests below.
    struct RanTool(Arc<std::sync::Mutex<bool>>);
    #[async_trait::async_trait]
    impl agent_core::Tool for RanTool {
        fn spec(&self) -> agent_core::ToolSpec {
            agent_core::ToolSpec {
                name: "Bash".to_string(),
                description: "test".into(),
                parameters: serde_json::json!({ "type": "object" }),
                risk: agent_core::RiskLevel::Read,
            }
        }
        async fn call(&self, _args: serde_json::Value) -> Result<serde_json::Value> {
            *self.0.lock().unwrap() = true;
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    fn temp_storage() -> Arc<Storage> {
        let dir = std::env::temp_dir().join(format!("fleety-hookreg-{}", uuid::Uuid::new_v4()));
        Arc::new(Storage::new(dir))
    }

    #[tokio::test]
    async fn wrap_registry_with_hooks_denies_on_nonzero_pre() {
        // A same-host (device None) PreToolUse hook whose command exits non-zero
        // must deny the wrapped tool. `exit 1` works under both `sh -c` and
        // `cmd /C`.
        let ran = Arc::new(std::sync::Mutex::new(false));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(RanTool(Arc::clone(&ran))));
        let ctx = HookContext {
            hooks: vec![crate::hooks_compat::HookEntry {
                event: crate::hooks_compat::HookEvent::PreToolUse,
                matcher: "*".into(),
                command: "exit 1".into(),
                scope: crate::hooks_compat::HookScope::User,
            }],
            device: None,
            cwd: None,
        };
        let hub = crate::bridge::new_hub();
        let pending = crate::bridge::new_pending();
        let storage = temp_storage();
        wrap_registry_with_hooks(
            &mut tools, &ctx, &hub, &pending, &storage, "dev-1", "conv-1",
        );
        let out = tools.call("Bash", serde_json::json!({})).await.unwrap();
        assert_eq!(out["denied"], serde_json::json!(true));
        assert!(!*ran.lock().unwrap(), "denied tool must not run");
        // The hook execution was audited under the conversation.
        let audit = storage.list_audit("dev-1", None, None).unwrap();
        assert!(!audit.is_empty(), "hook execution is audited");
    }

    #[tokio::test]
    async fn empty_hook_context_leaves_registry_unwrapped() {
        let ran = Arc::new(std::sync::Mutex::new(false));
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(RanTool(Arc::clone(&ran))));
        let ctx = HookContext {
            hooks: Vec::new(),
            device: None,
            cwd: None,
        };
        let hub = crate::bridge::new_hub();
        let pending = crate::bridge::new_pending();
        let storage = temp_storage();
        wrap_registry_with_hooks(
            &mut tools, &ctx, &hub, &pending, &storage, "dev-1", "conv-1",
        );
        let out = tools.call("Bash", serde_json::json!({})).await.unwrap();
        assert_eq!(out["ok"], serde_json::json!(true), "tool runs normally");
        assert!(*ran.lock().unwrap(), "no hooks ⇒ unwrapped ⇒ tool ran");
    }

    #[tokio::test]
    async fn run_conversation_event_hooks_proceeds_when_no_such_event() {
        // A context with only a PostToolUse hook: a UserPromptSubmit event has no
        // matching hook, so it proceeds and runs (audits) nothing.
        let ctx = HookContext {
            hooks: vec![crate::hooks_compat::HookEntry {
                event: crate::hooks_compat::HookEvent::PostToolUse,
                matcher: "*".into(),
                command: "post".into(),
                scope: crate::hooks_compat::HookScope::User,
            }],
            device: None,
            cwd: None,
        };
        let hub = crate::bridge::new_hub();
        let pending = crate::bridge::new_pending();
        let storage = temp_storage();
        let proceed = run_conversation_event_hooks(
            crate::hooks_compat::HookEvent::UserPromptSubmit,
            &ctx,
            &hub,
            &pending,
            &storage,
            "dev-1",
            "conv-1",
        )
        .await;
        assert!(proceed, "no matching event ⇒ proceed");
        let audit = storage.list_audit("dev-1", None, None).unwrap();
        assert!(audit.is_empty(), "no hook ran ⇒ nothing audited");
    }

    #[test]
    fn valid_token_authenticates_even_when_a_used_pairing_code_is_resent() {
        // A daemon keeps FLEETY_PAIRING_CODE set across reconnects and sends both
        // the (now-redeemed) code and its saved token. Auth must accept the token
        // and not reject on the spent code.
        let path = std::env::temp_dir().join(format!("fleety-auth-{}.json", uuid::Uuid::new_v4()));
        let auth = AuthStore::load(path, None, true);
        let code = auth.create_pairing().expect("mint code");

        // First connect: the code enrolls the device and mints a token.
        let token = match authenticate(&auth, "dev", None, Some(&code), false) {
            Ok(Some(t)) => t,
            other => panic!("expected a minted token, got {other:?}"),
        };

        // Reconnect: the same (now-used) code is resent alongside the valid token.
        assert!(
            matches!(
                authenticate(&auth, "dev", Some(&token), Some(&code), false),
                Ok(None)
            ),
            "a valid token must authenticate even when the spent pairing code is resent"
        );

        // Sanity: a bad token with no code is still rejected.
        assert!(authenticate(&auth, "dev", Some("bogus"), None, false).is_err());
    }

    #[test]
    fn pending_interjection_limits_cover_count_and_total_payload_bytes() {
        assert!(pending_interjection_fits(
            MAX_PENDING_INTERJECTIONS - 1,
            MAX_PENDING_INTERJECTION_BYTES - 1,
            1,
        ));
        assert!(!pending_interjection_fits(MAX_PENDING_INTERJECTIONS, 0, 1,));
        assert!(!pending_interjection_fits(
            0,
            MAX_PENDING_INTERJECTION_BYTES,
            1,
        ));
        assert!(!pending_interjection_fits(0, usize::MAX, usize::MAX));
    }

    #[test]
    fn interjection_decisions_distinguish_accepted_and_ignored() {
        assert_eq!(
            interjection_decision(crate::triage::TriageAction::QueueAfter).0,
            InterjectionDisposition::Queued
        );
        assert_eq!(
            interjection_decision(crate::triage::TriageAction::Ignore),
            (
                InterjectionDisposition::Ignored,
                "noted — this message won't start another turn",
                false,
            )
        );
    }

    #[test]
    fn same_host_origin_requires_explicit_loopback_trust() {
        assert!(trusted_same_host_origin(true));
        assert!(!trusted_same_host_origin(false));
    }

    #[test]
    #[serial_test::serial]
    fn loopback_peer_is_trusted_only_when_enabled() {
        assert!(peer_is_loopback(Some("127.0.0.1:5".parse().unwrap())));
        assert!(peer_is_loopback(Some("127.0.5.9:5".parse().unwrap())));
        assert!(peer_is_loopback(Some("[::1]:5".parse().unwrap())));
        assert!(!peer_is_loopback(Some("192.168.1.10:5".parse().unwrap())));
        assert!(!peer_is_loopback(None));

        let path = std::env::temp_dir().join(format!("fleety-lb-{}.json", uuid::Uuid::new_v4()));
        let auth = AuthStore::load(path, None, true); // auth required

        std::env::remove_var("FLEETY_TRUST_LOOPBACK"); // default: trust on
        assert!(trust_loopback_enabled());
        // A token-less loopback client is accepted (Ok(None)) when trust is on.
        assert!(matches!(
            authenticate(&auth, "dev", None, None, true),
            Ok(None)
        ));
        // A non-loopback token-less client is still rejected.
        assert!(authenticate(&auth, "dev", None, None, false).is_err());

        std::env::set_var("FLEETY_TRUST_LOOPBACK", "0");
        assert!(!trust_loopback_enabled());
        // With trust off, even a loopback token-less client is rejected.
        assert!(authenticate(&auth, "dev", None, None, true).is_err());
        std::env::remove_var("FLEETY_TRUST_LOOPBACK");
    }

    #[test]
    #[serial_test::serial]
    fn config_apply_server_local_device() {
        use fleety_protocol::{ConfigTarget, Effect};
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let dir = std::env::temp_dir().join(format!("fleety-cfgapply-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk dir");
        std::env::set_var("FLEETY_CONFIG", dir.join("config.toml"));
        std::env::set_var("FLEETY_PROVIDERS", dir.join("providers.toml"));
        // Don't let an ambient env var shadow the config read.
        std::env::remove_var("FLEETY_MODEL");

        // Server flat set → ok + Restart effect.
        match config_apply(ConfigTarget::Server, &s(&["set", "FLEETY_MODEL", "gpt-5"])) {
            ServerMsg::ConfigResult {
                ok, effect, error, ..
            } => {
                assert!(ok, "server config set failed: {error:?}");
                assert_eq!(effect, Some(Effect::Restart));
            }
            other => panic!("expected ConfigResult, got {other:?}"),
        }
        // list reflects the change; a read has no effect.
        match config_apply(ConfigTarget::Server, &s(&["list"])) {
            ServerMsg::ConfigResult {
                ok, output, effect, ..
            } => {
                assert!(ok);
                assert!(output.contains("FLEETY_MODEL"));
                assert_eq!(effect, None);
            }
            other => panic!("got {other:?}"),
        }
        // Unknown key → error result (no crash, nothing written).
        match config_apply(ConfigTarget::Server, &s(&["set", "FLEETY_NOPE", "x"])) {
            ServerMsg::ConfigResult { ok, error, .. } => {
                assert!(!ok);
                assert!(error.is_some());
            }
            other => panic!("got {other:?}"),
        }
        // Remote-write auth gate (pure): a mutating frame is denied only when the
        // server does not require auth; reads are always allowed.
        assert!(remote_mutation_denied(
            &s(&["set", "FLEETY_POLICY", "require_approval"]),
            false
        ));
        assert!(!remote_mutation_denied(
            &s(&["set", "FLEETY_POLICY", "require_approval"]),
            true
        ));
        assert!(!remote_mutation_denied(&s(&["list"]), false)); // reads allowed
        assert!(remote_mutation_denied(
            &s(&["provider", "add", "p", "--type", "api", "--base-url", "u"]),
            false
        ));

        // Provider add → ok + NextConnection effect.
        match config_apply(
            ConfigTarget::Server,
            &s(&[
                "provider",
                "add",
                "p",
                "--type",
                "api",
                "--base-url",
                "https://api.example.test/v1",
            ]),
        ) {
            ServerMsg::ConfigResult { ok, effect, .. } => {
                assert!(ok);
                assert_eq!(effect, Some(Effect::NextConnection));
            }
            other => panic!("got {other:?}"),
        }
        // Local is the CLI's job → error.
        match config_apply(ConfigTarget::Local, &s(&["list"])) {
            ServerMsg::ConfigResult { ok, .. } => assert!(!ok),
            other => panic!("got {other:?}"),
        }
        // Device is a follow-up → unsupported error.
        match config_apply(ConfigTarget::Device("pi".into()), &s(&["list"])) {
            ServerMsg::ConfigResult { ok, error, .. } => {
                assert!(!ok);
                assert_eq!(error.expect("err").kind, "unsupported");
            }
            other => panic!("got {other:?}"),
        }

        std::env::remove_var("FLEETY_CONFIG");
        std::env::remove_var("FLEETY_PROVIDERS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial_test::serial]
    fn structured_snapshot_rejects_blank_provider_keys() {
        let dir = std::env::temp_dir().join(format!("fleety-snap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::env::set_var("FLEETY_CONFIG", dir.join("config.toml"));
        std::env::set_var("FLEETY_PROVIDERS", dir.join("providers.toml"));
        std::env::remove_var("FLEETY_TOKEN");
        std::fs::write(
            dir.join("providers.toml"),
            "[providers.private-api]\ntype = \"api\"\nbase_url = \"https://api.example.test/v1\"\nkey = \"   \"\n",
        )
        .expect("provider config");

        let reply = build_config_snapshot();

        let ServerMsg::ConfigResult {
            ok: false,
            error: Some(error),
            ..
        } = reply
        else {
            panic!("blank Provider key must reject the snapshot")
        };
        assert!(
            error.message.contains("must not be empty"),
            "{}",
            error.message
        );

        std::env::remove_var("FLEETY_CONFIG");
        std::env::remove_var("FLEETY_PROVIDERS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial_test::serial]
    fn structured_snapshot_omits_secret_values_and_tags_revision() {
        let dir = std::env::temp_dir().join(format!("fleety-snap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::env::set_var("FLEETY_CONFIG", dir.join("config.toml"));
        std::env::set_var("FLEETY_PROVIDERS", dir.join("providers.toml"));
        std::env::remove_var("FLEETY_TOKEN");
        std::fs::write(
            dir.join("providers.toml"),
            "[providers.private-api]\ntype = \"api\"\nbase_url = \"https://api.example.test/v1\"\nkey = \"sk-snapshot-secret\"\n",
        )
        .expect("provider config");

        match build_config_snapshot() {
            ServerMsg::ConfigSnapshotResult {
                revision,
                entries,
                providers_json,
            } => {
                assert!(
                    revision.contains(':'),
                    "revision carries the boot id: {revision}"
                );
                let tok = entries
                    .iter()
                    .find(|e| e.key == "FLEETY_TOKEN")
                    .expect("token entry");
                assert!(tok.secret && tok.value.is_empty(), "secret value omitted");
                assert!(!providers_json.contains("sk-snapshot-secret"));
                let value: serde_json::Value =
                    serde_json::from_str(&providers_json).expect("providers_json parses");
                assert_eq!(
                    value.get("key_present"),
                    Some(&serde_json::json!(["private-api"]))
                );
                assert!(value["providers"]["private-api"].get("key").is_none());

                // Applying the redacted snapshot means Keep, not Clear. The key
                // remains Server-owned and never needs to round-trip through CLI.
                let (reply, _, _, changed, _) =
                    apply_structured_changes(&revision, &[], Some(&providers_json), true);
                assert!(matches!(reply, ServerMsg::ConfigResult { ok: true, .. }));
                assert!(changed);
                let stored =
                    fleety_tools::providers_config::load_or_default(&dir.join("providers.toml"))
                        .expect("stored providers");
                assert_eq!(
                    stored
                        .provider("private-api")
                        .and_then(|provider| provider.key.as_deref()),
                    Some("sk-snapshot-secret")
                );

                // Clear is a separate, explicit intent. A redacted `None`
                // alone can never remove the Server-owned secret.
                let ServerMsg::ConfigSnapshotResult {
                    revision: clear_revision,
                    providers_json: clear_json,
                    ..
                } = build_config_snapshot()
                else {
                    panic!("expected refreshed snapshot");
                };
                let mut clear_value: serde_json::Value =
                    serde_json::from_str(&clear_json).expect("clear snapshot parses");
                clear_value
                    .as_object_mut()
                    .expect("object")
                    .insert("clear_keys".into(), serde_json::json!(["private-api"]));
                let clear_payload = serde_json::to_string(&clear_value).expect("clear payload");
                let (reply, _, _, changed, _) =
                    apply_structured_changes(&clear_revision, &[], Some(&clear_payload), true);
                assert!(matches!(reply, ServerMsg::ConfigResult { ok: true, .. }));
                assert!(changed);
                let stored =
                    fleety_tools::providers_config::load_or_default(&dir.join("providers.toml"))
                        .expect("stored providers after clear");
                assert!(stored
                    .provider("private-api")
                    .and_then(|provider| provider.key.as_ref())
                    .is_none());
            }
            other => panic!("expected ConfigSnapshotResult, got {other:?}"),
        }
        std::env::remove_var("FLEETY_CONFIG");
        std::env::remove_var("FLEETY_PROVIDERS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn auth_disabled_catalog_fails_before_provider_or_network_access() {
        let reply = provider_model_list("private-api", false).await;
        match reply {
            ServerMsg::ProviderModelListResult {
                provider,
                model_ids,
                error: Some(error),
            } => {
                assert_eq!(provider, "private-api");
                assert!(model_ids.is_empty());
                assert_eq!(error.kind, "unauthenticated");
            }
            other => panic!("expected credential gate error, got {other:?}"),
        }
    }

    #[test]
    #[serial_test::serial]
    fn structured_apply_lock_tristate_auth_and_sensitive() {
        use fleety_protocol::{ChangeOp, ConfigChange};
        let dir = std::env::temp_dir().join(format!("fleety-apply-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::env::set_var("FLEETY_CONFIG", &path);
        std::env::remove_var("FLEETY_POLICY");
        let set = |key: &str, v: &str| ConfigChange {
            key: key.into(),
            op: ChangeOp::Set,
            value: Some(v.into()),
        };

        // Stale base_revision → conflict, nothing applied.
        let (r, applied, _, _, _) = apply_structured_changes(
            "stale:rev",
            &[set("FLEETY_POLICY", "require_approval")],
            None,
            true,
        );
        assert!(
            matches!(&r, ServerMsg::ConfigResult { ok: false, error: Some(e), .. } if e.kind == "conflict")
        );
        assert_eq!(applied, 0);

        // Correct revision + auth on → the Set applies (validated, atomic save).
        let rev = config_revision();
        let (r, applied, _, _, _) = apply_structured_changes(
            &rev,
            &[set("FLEETY_POLICY", "require_approval")],
            None,
            true,
        );
        assert!(matches!(r, ServerMsg::ConfigResult { ok: true, .. }));
        assert_eq!(applied, 1);
        assert_eq!(
            fleety_tools::config::load_strict(&path)
                .unwrap()
                .get(&(fleety_tools::config::Scope::Server, "FLEETY_POLICY".into()))
                .map(String::as_str),
            Some("require_approval")
        );

        // Auth off + a mutating change → refused (unauthenticated).
        let (r, _, _, _, _) = apply_structured_changes(
            &config_revision(),
            &[set("FLEETY_TZ", "Asia/Taipei")],
            None,
            false,
        );
        assert!(
            matches!(&r, ServerMsg::ConfigResult { ok: false, error: Some(e), .. } if e.kind == "unauthenticated")
        );
        // Auth off + a Keep-only apply (no mutation) → allowed no-op.
        let (r, applied, _, _, _) = apply_structured_changes(
            &config_revision(),
            &[ConfigChange {
                key: "FLEETY_TZ".into(),
                op: ChangeOp::Keep,
                value: None,
            }],
            None,
            false,
        );
        assert!(matches!(r, ServerMsg::ConfigResult { ok: true, .. }));
        assert_eq!(applied, 0);

        // A sensitive key is reported for auditing.
        let (_, _, sensitive, _, _) = apply_structured_changes(
            &config_revision(),
            &[set("FLEETY_MODEL_KEY", "sk-x")],
            None,
            true,
        );
        assert_eq!(sensitive, vec!["FLEETY_MODEL_KEY".to_string()]);

        std::env::remove_var("FLEETY_CONFIG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A non-WebSocket `ClientInbound` backed by a queue — proves the connection
    /// loop's inbound is transport-agnostic (the SSE+POST transport plugs in the
    /// same way). The end-to-end WS path is covered by the integration tests below.
    #[tokio::test]
    async fn client_inbound_is_transport_agnostic() {
        struct QueueInbound(std::collections::VecDeque<ClientMsg>);
        #[async_trait::async_trait]
        impl ClientInbound for QueueInbound {
            async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
                Ok(self.0.pop_front())
            }
        }
        let mut inbound: Box<dyn ClientInbound> = Box::new(QueueInbound(
            vec![
                ClientMsg::Approve {
                    approval_id: "a1".into(),
                },
                ClientMsg::Deny {
                    approval_id: "a2".into(),
                },
            ]
            .into(),
        ));
        // Drains in order, then signals end-of-stream with None.
        assert!(matches!(
            inbound.next_client().await.unwrap(),
            Some(ClientMsg::Approve { approval_id }) if approval_id == "a1"
        ));
        assert!(matches!(
            inbound.next_client().await.unwrap(),
            Some(ClientMsg::Deny { approval_id }) if approval_id == "a2"
        ));
        assert!(inbound.next_client().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn approval_gate_defers_non_approval_frames_in_bounded_fifo_order() {
        struct QueueInbound(std::collections::VecDeque<ClientMsg>);
        #[async_trait::async_trait]
        impl ClientInbound for QueueInbound {
            async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
                Ok(self.0.pop_front())
            }
        }
        let message = |id: String| ClientMsg::UserMessage {
            message_id: id.clone(),
            conversation_id: None,
            text: id,
            origin: Default::default(),
            attachments: Vec::new(),
            voice: false,
            acting_user: None,
        };
        let submitted: Vec<ClientMsg> = (0..=MAX_PENDING_INTERJECTIONS)
            .map(|index| message(format!("message-{index:02}")))
            .collect();
        let mut inbound = QueueInbound(submitted.into());
        let (out, mut rx) = mpsc::unbounded_channel();
        let mut deferred = std::collections::VecDeque::new();
        let mut deferred_bytes = 0usize;
        let decision = ConnGate {
            out,
            inbound: &mut inbound,
            deferred: &mut deferred,
            deferred_bytes: &mut deferred_bytes,
            conversation: "c1",
        }
        .request(
            "write_file",
            &serde_json::json!({"path": "note.txt"}),
            RiskLevel::Mutate,
        )
        .await
        .expect("closed approval stream denies without dropping frames");

        assert_eq!(decision, ApprovalDecision::Deny);
        assert_eq!(deferred.len(), MAX_PENDING_INTERJECTIONS);
        assert!(deferred_bytes <= MAX_PENDING_INTERJECTION_BYTES);
        let ids: Vec<String> = deferred
            .iter()
            .filter_map(|message| match message {
                ClientMsg::UserMessage { message_id, .. } => Some(message_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids.first().map(String::as_str), Some("message-00"));
        assert_eq!(ids.last().map(String::as_str), Some("message-15"));
        assert!(drain(&mut rx).iter().any(|frame| matches!(
            frame,
            ServerMsg::InterjectionAcknowledged {
                message_id,
                disposition: InterjectionDisposition::Rejected,
                ..
            } if message_id == "message-16"
        )));
    }

    #[tokio::test]
    async fn incompatible_protocol_is_rejected_before_device_registration() {
        struct QueueInbound(std::collections::VecDeque<ClientMsg>);
        #[async_trait::async_trait]
        impl ClientInbound for QueueInbound {
            async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
                Ok(self.0.pop_front())
            }
        }
        struct CollectWriter(Arc<std::sync::Mutex<Vec<String>>>);
        #[async_trait::async_trait]
        impl FrameWriter for CollectWriter {
            async fn send_text(&mut self, text: String) -> bool {
                self.0.lock().expect("writer lock").push(text);
                true
            }
        }

        let home =
            std::env::temp_dir().join(format!("fleety-protocol-mismatch-{}", uuid::Uuid::new_v4()));
        let storage = Arc::new(Storage::new(home.clone()));
        let frames = Arc::new(std::sync::Mutex::new(Vec::new()));
        let inbound = QueueInbound(
            vec![ClientMsg::Hello {
                device_id: "old-client".into(),
                protocol: PROTOCOL_VERSION - 1,
                token: None,
                pairing_code: None,
                local_tools_json: None,
                hostname: None,
            }]
            .into(),
        );
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![]));

        run_connection(
            Box::new(inbound),
            Box::new(CollectWriter(Arc::clone(&frames))),
            Arc::clone(&storage),
            provider,
            Arc::new(home.join("workspace")),
            Policy::FullAccess,
            bridge::new_hub(),
            bridge::new_pending(),
            bridge::new_handles(),
            open_auth(),
            bridge::new_device_tools(),
            true,
        )
        .await
        .expect("protocol rejection is handled in band");

        let frames = frames.lock().expect("frames lock");
        assert_eq!(frames.len(), 1);
        let reply: ServerMsg = serde_json::from_str(&frames[0]).expect("error frame");
        assert!(matches!(
            reply,
            ServerMsg::Error { error } if error.kind == "protocol_mismatch"
        ));
        assert!(
            !storage.devices_dir().join("old-client").exists(),
            "an incompatible Hello must not register the asserted device"
        );

        let _ = std::fs::remove_dir_all(home);
    }

    // ---- proactive schedule-notification delivery ----------------------------

    #[tokio::test]
    async fn connect_delivers_unnotified_schedule_outcomes() {
        let home = std::env::temp_dir().join(format!("fleety-notify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        // The scheduler device and the connecting device share owner "alice".
        storage
            .set_device_ownership(crate::scheduler::SCHED_DEVICE, Some("alice"), &[], false)
            .expect("own sched");
        storage
            .set_device_ownership("phone", Some("alice"), &[], false)
            .expect("own phone");
        // A schedule with a failed outcome, not yet notified.
        let sdir = storage.schedules_dir();
        std::fs::create_dir_all(&sdir).expect("mk sdir");
        std::fs::write(
            sdir.join("s1.json"),
            r#"{"id":"s1","trigger":"at:1","prompt":"p","enabled":true}"#,
        )
        .expect("write schedule");
        crate::schedules::record_outcome(&sdir, "s1", "error", "it broke", 100).expect("outcome");

        let (out, mut rx) = mpsc::unbounded_channel::<WsMessage>();
        deliver_pending_schedule_notifications(&storage, &out, "phone");
        let assistant: Vec<(String, String)> = drain(&mut rx)
            .into_iter()
            .filter_map(|m| match m {
                ServerMsg::Assistant {
                    conversation_id,
                    text,
                    ..
                } => Some((conversation_id, text)),
                _ => None,
            })
            .collect();
        assert_eq!(assistant.len(), 1);
        assert_eq!(assistant[0].0, "schedule-s1");
        // Failure is prominently marked and points the user at the conversation.
        assert!(assistant[0].1.contains("FAILED"));
        assert!(assistant[0].1.contains("it broke"));
        assert!(assistant[0]
            .1
            .contains("fleety conversations resume schedule-s1"));

        // A second connect from the same device does not redeliver the outcome.
        deliver_pending_schedule_notifications(&storage, &out, "phone");
        assert!(drain(&mut rx).is_empty());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn guest_connection_gets_no_schedule_notifications() {
        let home =
            std::env::temp_dir().join(format!("fleety-notify-guest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        storage
            .set_device_ownership(crate::scheduler::SCHED_DEVICE, Some("alice"), &[], false)
            .expect("own sched");
        // "bobphone" is owned by a different user; "kiosk" has no owner (Guest).
        storage
            .set_device_ownership("bobphone", Some("bob"), &[], false)
            .expect("own bob");
        let sdir = storage.schedules_dir();
        std::fs::create_dir_all(&sdir).expect("mk sdir");
        std::fs::write(
            sdir.join("s1.json"),
            r#"{"id":"s1","trigger":"at:1","prompt":"p","enabled":true}"#,
        )
        .expect("write schedule");
        crate::schedules::record_outcome(&sdir, "s1", "ok", "done", 100).expect("outcome");

        let (out, mut rx) = mpsc::unbounded_channel::<WsMessage>();
        // Guest device (no owner record): nothing delivered.
        deliver_pending_schedule_notifications(&storage, &out, "kiosk");
        assert!(
            drain(&mut rx).is_empty(),
            "guest must not receive schedule notifications"
        );
        // Foreign-owner device: acting user differs from the scheduler owner.
        deliver_pending_schedule_notifications(&storage, &out, "bobphone");
        assert!(
            drain(&mut rx).is_empty(),
            "foreign-owner device must not receive schedule notifications"
        );
        // The outcome is still pending for the real owner (watermark untouched).
        assert_eq!(crate::schedules::pending_notifications(&sdir).len(), 1);

        let _ = std::fs::remove_dir_all(&home);
    }

    // ---- drive-to-goal loop tests --------------------------------------------

    /// A scripted assistant turn that calls one tool.
    fn call_resp(id: &str, name: &str, args: serde_json::Value) -> ModelResponse {
        ModelResponse::new(Message {
            role: CoreRole::Assistant,
            content: None,
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments: args,
            }],
            tool_call_id: None,
            attachments: Vec::new(),
        })
    }

    /// A scripted assistant turn that just replies with text (ends the turn).
    fn text_resp(text: &str) -> ModelResponse {
        ModelResponse::new(Message::assistant(text))
    }

    /// A pre-set cancel flag stops the run cleanly rather than looping and
    /// exhausting the (single-scripted) provider. With agent-core's per-checkpoint
    /// cancellation, a flag already set when the turn begins is observed at the
    /// first pre-model checkpoint, so the turn stops before spending a call — the
    /// run still returns Ok, emits Done, and carries the triage stop wording.
    #[tokio::test]
    async fn cancel_stops_goal_loop_between_iterations() {
        let (storage, tools, goal_state, _home, out, mut rx) = goal_env();
        // One scripted reply only: a non-cancelled run that looped would exhaust it.
        let provider = MockProvider::new(vec![text_resp("did the first step")]);
        let mut gate = agent_core::AutoApprove;
        let cancel = CancelFlag::new();
        cancel.request_triage(); // a triaged interjection interrupted the run
        let _steps = drive_to_goal(
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
            &cancel,
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .expect("cancelled run still returns Ok (did not loop/exhaust the provider)");
        let frames = drain(&mut rx);
        // Stopped cleanly: a terminal Done was emitted, and the reply notes the stop.
        assert!(frames.iter().any(|m| matches!(m, ServerMsg::Done { .. })));
        assert!(frames.iter().any(|m| matches!(
            m,
            ServerMsg::Assistant { text, .. } if text.contains("Stopped between steps")
        )));
    }

    /// An explicit cancel (vs. a triaged interjection) uses distinct wording so
    /// the user knows the turn was cancelled at their request.
    #[tokio::test]
    async fn explicit_cancel_uses_at_your_request_wording() {
        let (storage, tools, goal_state, _home, out, mut rx) = goal_env();
        let provider = MockProvider::new(vec![
            call_resp("a", "set_goal", serde_json::json!({ "goal": "keep going" })),
            text_resp("did the first step"),
        ]);
        let mut gate = agent_core::AutoApprove;
        let cancel = CancelFlag::new();
        cancel.request_explicit();
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
            &cancel,
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .expect("explicit cancel returns Ok");
        let frames = drain(&mut rx);
        assert!(frames.iter().any(|m| matches!(
            m,
            ServerMsg::Assistant { text, .. } if text.contains("Cancelled at your request")
        )));
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .unwrap();
        assert!(steps >= 1, "a completed turn reports its step count");
        let _ = drain(&mut rx);
        let _ = std::fs::remove_dir_all(home);
    }

    // Shared state for the effort-recording provider below: the effort each
    // `drive_turn` was invoked at (via `with_effort`), the scripted responses,
    // and the same session-effort slot drive_to_goal reads — so `complete` can
    // mutate it mid-request the way `set_effort` would.
    struct EffortRecShared {
        log: std::sync::Mutex<Vec<Option<agent_core::model::Effort>>>,
        script: std::sync::Mutex<std::collections::VecDeque<ModelResponse>>,
        slot: crate::effort::SessionEffort,
        calls: std::sync::atomic::AtomicUsize,
    }

    struct EffortRec {
        my_effort: Option<agent_core::model::Effort>,
        shared: Arc<EffortRecShared>,
    }

    #[async_trait::async_trait]
    impl ModelProvider for EffortRec {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> agent_core::Result<ModelResponse> {
            // Record the effort this variant represents, then (once) simulate the
            // agent pinning high mid-request as set_effort would.
            self.shared.log.lock().unwrap().push(self.my_effort);
            if self
                .shared
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                *self.shared.slot.lock().await = Some(agent_core::model::Effort::High);
            }
            let resp = self
                .shared
                .script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| ModelResponse::new(Message::assistant("done")));
            Ok(resp)
        }

        fn with_effort(
            &self,
            effort: Option<agent_core::model::Effort>,
        ) -> Option<Arc<dyn ModelProvider>> {
            Some(Arc::new(EffortRec {
                my_effort: effort,
                shared: Arc::clone(&self.shared),
            }))
        }
    }

    /// drive_to_goal re-reads the effort before EACH turn: the first turn runs at
    /// this message's difficulty baseline, and after a mid-request pin the next
    /// goal-continuation turn picks it up — not deferred to the next message.
    #[tokio::test]
    async fn effort_reread_per_continuation_picks_up_mid_request_pin() {
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        let slot = crate::effort::new_session_effort();
        let shared = Arc::new(EffortRecShared {
            log: std::sync::Mutex::new(Vec::new()),
            // Turn 1: set a goal (forces a continuation) then finish. Turn 2:
            // complete the goal (terminal) then finish.
            script: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
                call_resp("g", "set_goal", serde_json::json!({ "goal": "x" })),
                text_resp("t1"),
                call_resp(
                    "d",
                    "complete_goal",
                    serde_json::json!({ "summary": "done" }),
                ),
                text_resp("all done"),
            ])),
            slot: Arc::clone(&slot),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let base = EffortRec {
            my_effort: None,
            shared: Arc::clone(&shared),
        };
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &base,
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
            &CancelFlag::new(),
            &slot,
            // This message's difficulty baseline (no manual pin yet). Turn 1 uses
            // it; had the slot stayed None a continuation would fall back to it too
            // (the `auto`-cleared case).
            Some(agent_core::model::Effort::Low),
        )
        .await
        .unwrap();
        let log = shared.log.lock().unwrap().clone();
        assert_eq!(
            log.first(),
            Some(&Some(agent_core::model::Effort::Low)),
            "turn 1 runs at the difficulty baseline"
        );
        assert_eq!(
            log.last(),
            Some(&Some(agent_core::model::Effort::High)),
            "a continuation picks up the mid-request pin without a new user message"
        );
        let _ = drain(&mut rx);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn turn_injects_ephemeral_origin() {
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        // A cross-device binding: the conversation was started on another device.
        let binding = crate::workspace::WorkspaceBinding {
            root: home.clone(),
            device: Some("dev2".to_string()),
            origin_cwd: Some("/home/bob/proj".to_string()),
            origin_hostname: Some("bob-laptop".to_string()),
            origin_os: Some("macos".to_string()),
        };
        storage
            .set_conversation_workspace("c1", &binding)
            .expect("set binding");
        // This binding produces a cross-device origin preamble (what the turn
        // injects). Same signal the drive_turn injection point reads.
        let pre = crate::workspace::origin_preamble(&binding).expect("preamble");
        assert!(pre.contains("device_exec") && pre.contains("/home/bob/proj"));

        let provider = MockProvider::new(vec![text_resp("ok")]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("hi"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .unwrap();

        // The origin preamble is ephemeral: injected into the turn's messages
        // but NEVER appended to the persisted conversation. If it leaked into
        // history, its `device_exec` marker would show up here.
        let history = storage.load("dev", "c1").expect("load");
        let dump = format!("{history:?}");
        assert!(
            !dump.contains("device_exec"),
            "origin preamble must not be persisted to the conversation history"
        );
        let _ = drain(&mut rx);
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn resume_reinjects_persisted_origin() {
        let (storage, tools, goal_state, home, out, mut rx) = goal_env();
        // Resume: the binding was persisted earlier; the resuming message carries
        // no origin. The preamble is rebuilt from the persisted binding, not the
        // message, so it stays identical across a resume reload.
        let binding = crate::workspace::WorkspaceBinding {
            root: home.clone(),
            device: Some("dev2".to_string()),
            origin_cwd: Some("/home/bob/proj".to_string()),
            origin_hostname: Some("bob-laptop".to_string()),
            origin_os: Some("macos".to_string()),
        };
        storage
            .set_conversation_workspace("c1", &binding)
            .expect("set");
        let before = crate::workspace::origin_preamble(&binding);
        let after = crate::workspace::origin_preamble(
            &storage.conversation_workspace("c1").expect("reload"),
        );
        assert_eq!(
            before, after,
            "origin preamble is identical after a resume reload"
        );
        assert!(after.expect("some").contains("dev2"));

        // A resume message with no origin still drives a turn cleanly.
        let provider = MockProvider::new(vec![text_resp("ok")]);
        let mut gate = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider,
            &tools,
            Policy::FullAccess,
            "dev",
            "c1",
            Message::user("resume"),
            &mut gate,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .unwrap();

        // A legacy binding (persisted before origin fields existed) has no origin
        // fields → the preamble is omitted and the resume does not error.
        let legacy = crate::workspace::WorkspaceBinding {
            root: home.clone(),
            device: None,
            origin_cwd: None,
            origin_hostname: None,
            origin_os: None,
        };
        storage
            .set_conversation_workspace("c2", &legacy)
            .expect("set legacy");
        assert!(crate::workspace::origin_preamble(&legacy).is_none());
        let provider2 = MockProvider::new(vec![text_resp("ok2")]);
        let mut gate2 = agent_core::AutoApprove;
        drive_to_goal(
            &out,
            &storage,
            &provider2,
            &tools,
            Policy::FullAccess,
            "dev",
            "c2",
            Message::user("resume legacy"),
            &mut gate2,
            &goal_state,
            5,
            false,
            &crate::identity::ActingUser::Guest,
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .unwrap();

        let _ = drain(&mut rx);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn instruction_preamble_injects_layered_files_per_conversation() {
        let home = std::env::temp_dir().join(format!("fleety-instr-{}", uuid::Uuid::new_v4()));
        let proj = home.join("proj");
        std::fs::create_dir_all(&proj).expect("mk proj");
        std::fs::write(home.join("AGENTS.md"), "root-rule").expect("w root");
        std::fs::write(proj.join("CLAUDE.md"), "proj-rule").expect("w proj");
        let storage = Arc::new(Storage::new(
            std::env::temp_dir().join(format!("fleety-instr-s-{}", uuid::Uuid::new_v4())),
        ));
        let binding = crate::workspace::WorkspaceBinding {
            root: proj.clone(),
            device: None,
            origin_cwd: Some(proj.to_string_lossy().to_string()),
            origin_hostname: None,
            origin_os: None,
        };
        storage
            .set_conversation_workspace("c1", &binding)
            .expect("set");
        // The bound conversation gets both the shallow (root AGENTS.md) and deep
        // (proj CLAUDE.md) instruction files, re-read live.
        let out = build_instruction_preamble(&storage, "c1", &home).expect("preamble");
        assert!(out.contains("root-rule"), "shallow layer injected");
        assert!(out.contains("proj-rule"), "deep layer injected");
        // Per-conversation: an unbound conversation gets nothing.
        assert!(build_instruction_preamble(&storage, "c2", &home).is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    fn cross_device_binding() -> crate::workspace::WorkspaceBinding {
        crate::workspace::WorkspaceBinding {
            root: std::env::temp_dir(),
            device: Some("dev2".to_string()),
            origin_cwd: Some("/home/bob/proj".to_string()),
            origin_hostname: None,
            origin_os: None,
        }
    }

    #[tokio::test]
    async fn cross_device_reads_via_device_exec() {
        use agent_core::{CoreError, RiskLevel, Tool, ToolSpec};
        /// Stands in for a device's `read_file`. Unlike a mock that answers
        /// anything, this one **enforces the tool's contract**: it demands the
        /// `path` argument `read_file` actually requires and replies in the shape
        /// `read_file` actually returns (a line-numbered view only). A caller that
        /// passes the wrong key, or reads the wrong field, therefore fails here
        /// instead of silently injecting nothing.
        struct MockExec;
        #[async_trait::async_trait]
        impl Tool for MockExec {
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "device_exec".to_string(),
                    description: String::new(),
                    parameters: serde_json::json!({}),
                    risk: RiskLevel::Read,
                }
            }
            async fn call(&self, args: serde_json::Value) -> agent_core::Result<serde_json::Value> {
                // The outer device_exec envelope must be right too, not just the
                // inner read_file arguments.
                assert_eq!(args["tool"], "read_file");
                assert_eq!(args["device"], "dev2");
                let inner = args.get("args").cloned().unwrap_or_default();
                let path = inner.get("path").and_then(serde_json::Value::as_str);
                let path = path.ok_or_else(|| {
                    CoreError::Message(format!("read_file requires a 'path' argument; got {inner}"))
                })?;
                assert!(path.ends_with(".md"), "reads an instruction file: {path}");
                Ok(serde_json::json!({
                    "path": path,
                    "numbered": fleety_tools::line_numbered("remote-rule\nsecond\tline", 1),
                    "start_line": 1,
                    "end_line": 2,
                    "line_count": 2,
                }))
            }
        }
        let home = std::env::temp_dir().join(format!("fleety-xdev-{}", uuid::Uuid::new_v4()));
        let storage = Arc::new(Storage::new(home.clone()));
        storage
            .set_conversation_workspace("c1", &cross_device_binding())
            .expect("set");
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(MockExec));
        let out = build_instruction_preamble_remote(&storage, "c1", &tools)
            .await
            .expect("remote preamble");
        assert!(out.contains("remote-rule"), "cross-device content injected");
        // Injected as the file's own text: the numbering is undone, and a tab
        // inside the content survives.
        assert!(
            out.contains("second\tline"),
            "content round-trips through the numbered view: {out}"
        );
        assert!(
            !out.contains("     1\tremote-rule"),
            "line-number prefixes must not reach the model: {out}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn cross_device_read_failure_is_skipped() {
        let home = std::env::temp_dir().join(format!("fleety-xdev2-{}", uuid::Uuid::new_v4()));
        let storage = Arc::new(Storage::new(home.clone()));
        storage
            .set_conversation_workspace("c1", &cross_device_binding())
            .expect("set");
        // No device_exec registered → every read errors → all skipped, no panic.
        let tools = ToolRegistry::new();
        let out = build_instruction_preamble_remote(&storage, "c1", &tools).await;
        assert!(out.is_none(), "failed reads are skipped without panicking");
        let _ = std::fs::remove_dir_all(&home);
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
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
            &CancelFlag::new(),
            &crate::effort::new_session_effort(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(frame_attention(&drain(&mut rx)), None);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn is_disconnect_classifies_close_and_disconnect_io_errors() {
        assert!(is_disconnect(&WsErr::ConnectionClosed));
        assert!(is_disconnect(&WsErr::AlreadyClosed));
        assert!(is_disconnect(&WsErr::Protocol(
            ProtocolError::ResetWithoutClosingHandshake
        )));
        for kind in [
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::UnexpectedEof,
        ] {
            assert!(is_disconnect(&WsErr::Io(std::io::Error::new(
                kind,
                "disconnect"
            ))));
        }
        assert!(!is_disconnect(&WsErr::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "not a disconnect"
        ))));
    }

    #[tokio::test]
    async fn startup_recovers_interrupted_interactive_turn() {
        use crate::auth::AuthStore;
        use crate::echo::EchoProvider;

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
            arguments: serde_json::json!({ "command": "echo hi" }),
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
                    arguments: serde_json::json!({ "command": "echo hi" }),
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

    #[tokio::test]
    async fn resume_recovery_emits_exactly_one_done_for_the_whole_stream() {
        use crate::echo::EchoProvider;

        let home =
            std::env::temp_dir().join(format!("fleety-resume-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let user = Message::user("status");
        storage.append("dev", "c1", &user).expect("append user");
        storage
            .journal_begin("dev", "c1", &user)
            .expect("begin journal");
        storage
            .journal_event(
                "dev",
                "c1",
                &Event::Assistant(Message::assistant("partial")),
            )
            .expect("seed interrupted event");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        {
            let storage = Arc::clone(&storage);
            let workspace = Arc::new(home.clone());
            tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await {
                    let _ = handle_conn(
                        stream,
                        storage,
                        Arc::new(EchoProvider),
                        workspace,
                        Policy::FullAccess,
                        bridge::new_hub(),
                        bridge::new_pending(),
                        bridge::new_handles(),
                        open_auth(),
                        bridge::new_device_tools(),
                    )
                    .await;
                }
            });
        }

        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();
        send_client(&mut tx, &hello("dev")).await;
        assert!(matches!(
            recv_server(&mut rx).await,
            Some(ServerMsg::Welcome { .. })
        ));
        send_client(
            &mut tx,
            &ClientMsg::Resume {
                conversation_id: "c1".into(),
                after_seq: 0,
            },
        )
        .await;
        send_client(&mut tx, &ClientMsg::ServerStatus).await;

        let mut resume_frames = Vec::new();
        loop {
            let frame = recv_server(&mut rx).await.expect("resume/status frame");
            if matches!(frame, ServerMsg::ServerStatusResult { .. }) {
                break;
            }
            resume_frames.push(frame);
        }
        assert!(resume_frames
            .iter()
            .any(|frame| matches!(frame, ServerMsg::Replay { .. })));
        assert!(
            !resume_frames.iter().any(|frame| matches!(
                frame,
                ServerMsg::Assistant { .. } | ServerMsg::AssistantDelta { .. }
            )),
            "Resume recovery is delivered only through Replay"
        );
        assert_eq!(
            resume_frames
                .iter()
                .filter(|frame| matches!(frame, ServerMsg::Done { .. }))
                .count(),
            1,
            "Resume owns one terminal marker even when it recovers a turn"
        );

        send_client(
            &mut tx,
            &ClientMsg::UserMessage {
                message_id: "follow-up-after-resume".into(),
                conversation_id: Some("c1".into()),
                text: "follow-up after resume".into(),
                origin: Default::default(),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            },
        )
        .await;
        let mut saw_follow_up = false;
        loop {
            match recv_server(&mut rx).await.expect("follow-up frame") {
                ServerMsg::Assistant {
                    conversation_id, ..
                } => {
                    assert_eq!(conversation_id, "c1");
                    saw_follow_up = true;
                }
                ServerMsg::Done { conversation_id } => {
                    assert_eq!(conversation_id, "c1");
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_follow_up);
        assert!(storage
            .load("dev", "c1")
            .expect("resumed history")
            .iter()
            .any(|message| message.content.as_deref() == Some("follow-up after resume")));

        let _ = std::fs::remove_dir_all(home);
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

    const PAIRED_TOKEN: &str = "paired-device-token-must-not-leak";

    /// A Server that knows exactly one device token, so the secure-channel
    /// tests can key a handshake the way a paired client does.
    fn paired_auth() -> Arc<AuthStore> {
        let path = std::env::temp_dir()
            .join(format!("fleety-auth-{}", uuid::Uuid::new_v4()))
            .join("auth.json");
        Arc::new(AuthStore::load(path, Some(PAIRED_TOKEN.to_string()), true))
    }

    /// Serve one raw-WebSocket connection with the given auth store, returning
    /// the address to connect to.
    async fn spawn_paired_server(auth: Arc<AuthStore>) -> std::net::SocketAddr {
        let home = std::env::temp_dir().join(format!("fleety-secure-{}", uuid::Uuid::new_v4()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let storage = Arc::new(Storage::new(home));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(
                    stream,
                    storage,
                    Arc::new(EchoProvider),
                    Arc::new(workspace),
                    Policy::FullAccess,
                    bridge::new_hub(),
                    bridge::new_pending(),
                    bridge::new_handles(),
                    auth,
                    bridge::new_device_tools(),
                )
                .await;
            }
        });
        addr
    }

    /// The full paired path: the client proves itself with the token it already
    /// holds, and from there nothing readable — the token least of all — is on
    /// the wire, yet the ordinary session proceeds untouched underneath.
    #[tokio::test]
    async fn a_paired_client_opens_a_secure_channel_and_the_session_runs_sealed() {
        let fingerprint = crate::test_server_fingerprint().to_string();
        let addr = spawn_paired_server(paired_auth()).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();

        let (initiator, opening) =
            fleety_tools::secure::Initiator::start(PAIRED_TOKEN, &fingerprint).expect("start");
        send_client(
            &mut tx,
            &ClientMsg::SecureHandshake {
                version: fleety_tools::secure::SECURE_CHANNEL_VERSION,
                key_id: fleety_tools::secure::key_id(PAIRED_TOKEN, &fingerprint).expect("key id"),
                msg: opening,
            },
        )
        .await;

        let Some(ServerMsg::SecureAccept { version, msg }) = recv_server(&mut rx).await else {
            panic!("the Server must answer a handshake it can key");
        };
        let mut session = initiator
            .finish(version, &msg)
            .expect("the Server proved itself");

        // Hello now travels sealed, so the token it carries never appears.
        let hello = serde_json::to_string(&ClientMsg::Hello {
            device_id: "dev".into(),
            protocol: PROTOCOL_VERSION,
            token: Some(PAIRED_TOKEN.to_string()),
            pairing_code: None,
            local_tools_json: None,
            hostname: None,
        })
        .expect("serialize");
        let payload = session.seal(&hello).expect("seal");
        assert!(
            !payload.contains(PAIRED_TOKEN),
            "the sealed Hello must not carry the token in the clear"
        );
        send_client(&mut tx, &ClientMsg::SecureFrame { payload }).await;

        let Some(ServerMsg::SecureFrame { payload }) = recv_server(&mut rx).await else {
            panic!("the Server must answer inside the channel, not beside it");
        };
        let opened = session.open(&payload).expect("open the Server's frame");
        assert!(
            opened.contains("\"type\":\"welcome\""),
            "the sealed frame carries the ordinary Welcome: {opened}"
        );
    }

    /// The endpoint list is only ever acted on by a sealed session, so sending
    /// it on an unsealed one is disclosure with nothing gained: it hands an
    /// unauthenticated peer this host's interface map.
    #[tokio::test]
    async fn an_unsealed_session_is_not_told_this_servers_other_addresses() {
        let _ = crate::test_server_fingerprint();
        // Without a listen address there is nothing to disclose and the
        // assertion below would hold vacuously.
        let advertised = crate::test_server_listen_addr();
        assert!(
            !advertised.is_empty(),
            "this host has no non-loopback interface, so the disclosure this test \
             guards cannot occur here"
        );
        let addr = spawn_paired_server(open_auth()).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();
        send_client(&mut tx, &hello("dev")).await;

        match recv_server(&mut rx).await {
            Some(ServerMsg::Welcome {
                server_endpoints, ..
            }) => assert!(
                server_endpoints.is_empty(),
                "an unsealed session must not receive this Server's address list: {server_endpoints:?}"
            ),
            other => panic!("expected a Welcome, got {other:?}"),
        }
    }

    /// A device this Server has no credential for gets one uninformative
    /// refusal and no session — it must not learn which tokens exist here, and
    /// it must not be quietly served in the clear instead.
    #[tokio::test]
    async fn an_unkeyable_handshake_is_refused_without_a_session_or_a_reason() {
        let fingerprint = crate::test_server_fingerprint().to_string();
        let addr = spawn_paired_server(paired_auth()).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();

        let stranger = "a-token-this-server-never-issued";
        let (_, opening) =
            fleety_tools::secure::Initiator::start(stranger, &fingerprint).expect("start");
        send_client(
            &mut tx,
            &ClientMsg::SecureHandshake {
                version: fleety_tools::secure::SECURE_CHANNEL_VERSION,
                key_id: fleety_tools::secure::key_id(stranger, &fingerprint).expect("key id"),
                msg: opening,
            },
        )
        .await;

        match recv_server(&mut rx).await {
            Some(ServerMsg::Error { error }) => {
                assert_eq!(error.kind, "secure_unavailable");
                assert!(
                    !error.message.contains(PAIRED_TOKEN),
                    "the refusal must not disclose what this Server holds"
                );
            }
            other => panic!("expected one uninformative refusal, got {other:?}"),
        }
        assert!(
            recv_server(&mut rx).await.is_none(),
            "a refused handshake must not go on to serve a session"
        );
    }

    /// Having opened the channel, the client may not step back out of it.
    #[tokio::test]
    async fn a_cleartext_frame_after_the_handshake_ends_the_connection() {
        let fingerprint = crate::test_server_fingerprint().to_string();
        let addr = spawn_paired_server(paired_auth()).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();

        let (initiator, opening) =
            fleety_tools::secure::Initiator::start(PAIRED_TOKEN, &fingerprint).expect("start");
        send_client(
            &mut tx,
            &ClientMsg::SecureHandshake {
                version: fleety_tools::secure::SECURE_CHANNEL_VERSION,
                key_id: fleety_tools::secure::key_id(PAIRED_TOKEN, &fingerprint).expect("key id"),
                msg: opening,
            },
        )
        .await;
        let Some(ServerMsg::SecureAccept { version, msg }) = recv_server(&mut rx).await else {
            panic!("the Server must answer a handshake it can key");
        };
        let _ = initiator
            .finish(version, &msg)
            .expect("channel established");

        // An unsealed Hello is what a relay stripping the encryption would send.
        send_client(&mut tx, &hello("dev")).await;

        assert!(
            recv_server(&mut rx).await.is_none(),
            "the Server must not fall back to serving cleartext once sealed"
        );
    }

    /// The compatibility path the release policy depends on: a client that does
    /// not offer a handshake is served exactly as before, so an already-paired
    /// device can still reach a Server it has not updated yet.
    #[tokio::test]
    async fn a_client_that_offers_no_handshake_is_still_served() {
        let addr = spawn_paired_server(open_auth()).await;

        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();
        send_client(&mut tx, &hello("dev")).await;

        assert!(
            matches!(recv_server(&mut rx).await, Some(ServerMsg::Welcome { .. })),
            "the cleartext path must keep working for clients that cannot seal"
        );
    }

    #[test]
    fn structured_apply_auth_gate_respects_owner_side_effects() {
        use fleety_protocol::{ChangeOp, ConfigChange, ConfigTarget};

        let keep = ConfigChange {
            key: "FLEETY_TZ".to_string(),
            op: ChangeOp::Keep,
            value: None,
        };
        let set = ConfigChange {
            key: "FLEETY_TZ".to_string(),
            op: ChangeOp::Set,
            value: Some("Asia/Taipei".to_string()),
        };
        let server = ConfigTarget::Server;
        let device = ConfigTarget::Device("remote-B".to_string());
        let local = ConfigTarget::Local;

        assert_eq!(
            structured_apply_gate(&server, &[], None, false),
            StructuredApplyGate::Continue
        );
        assert_eq!(
            structured_apply_gate(&server, std::slice::from_ref(&keep), None, false),
            StructuredApplyGate::Continue
        );
        assert_eq!(
            structured_apply_gate(&server, std::slice::from_ref(&set), None, false),
            StructuredApplyGate::Denied
        );
        assert_eq!(
            structured_apply_gate(&server, &[], Some(""), false),
            StructuredApplyGate::Denied
        );
        assert_eq!(
            structured_apply_gate(&device, &[], None, false),
            StructuredApplyGate::Denied
        );
        assert_eq!(
            structured_apply_gate(&device, &[keep], None, false),
            StructuredApplyGate::Denied
        );
        assert_eq!(
            structured_apply_gate(&local, std::slice::from_ref(&set), None, false),
            StructuredApplyGate::InvalidLocal
        );
        assert_eq!(
            structured_apply_gate(&server, std::slice::from_ref(&set), None, true),
            StructuredApplyGate::Continue
        );
        assert_eq!(
            structured_apply_gate(&device, &[set], None, true),
            StructuredApplyGate::Continue
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn auth_disabled_device_config_apply_is_rejected_before_run_tool() {
        struct EnvVarGuard {
            key: &'static str,
            previous: Option<std::ffi::OsString>,
        }

        impl EnvVarGuard {
            fn set(key: &'static str, value: &std::path::Path) -> Self {
                let previous = std::env::var_os(key);
                std::env::set_var(key, value);
                Self { key, previous }
            }
        }

        impl Drop for EnvVarGuard {
            fn drop(&mut self) {
                if let Some(previous) = &self.previous {
                    std::env::set_var(self.key, previous);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }

        let home = std::env::temp_dir().join(format!(
            "fleety-config-apply-auth-gate-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        let config_path = home.join("config.toml");
        let config_before = b"# auth-gate server config sentinel\n";
        std::fs::write(&config_path, config_before).expect("seed server config");
        let _config_guard = EnvVarGuard::set("FLEETY_CONFIG", &config_path);
        let storage = Arc::new(Storage::new(home.clone()));
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider);
        let hub = bridge::new_hub();
        let pending = bridge::new_pending();
        let handles = bridge::new_handles();
        let auth = open_auth();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("listener address");
        tokio::spawn({
            let storage = Arc::clone(&storage);
            let provider = Arc::clone(&provider);
            let workspace = Arc::new(workspace);
            let hub = Arc::clone(&hub);
            let pending = Arc::clone(&pending);
            let handles = Arc::clone(&handles);
            let auth = Arc::clone(&auth);
            async move {
                for _ in 0..2 {
                    let (stream, _) = listener.accept().await.expect("accept");
                    let task = handle_conn(
                        stream,
                        Arc::clone(&storage),
                        Arc::clone(&provider),
                        Arc::clone(&workspace),
                        Policy::FullAccess,
                        Arc::clone(&hub),
                        Arc::clone(&pending),
                        Arc::clone(&handles),
                        Arc::clone(&auth),
                        bridge::new_device_tools(),
                    );
                    tokio::spawn(async move {
                        let _ = task.await;
                    });
                }
            }
        });

        let url = format!("ws://{addr}");
        let (daemon_ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("daemon connect");
        let (mut daemon_tx, mut daemon_rx) = daemon_ws.split();
        send_client(
            &mut daemon_tx,
            &ClientMsg::Hello {
                device_id: "remote-B".to_string(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: None,
                local_tools_json: Some(
                    serde_json::to_string(&vec![agent_core::ToolSpec {
                        name: "run_command".to_string(),
                        description: "daemon marker".to_string(),
                        parameters: serde_json::json!({}),
                        risk: agent_core::RiskLevel::Mutate,
                    }])
                    .expect("tool specs"),
                ),
                hostname: None,
            },
        )
        .await;
        assert!(matches!(
            recv_server(&mut daemon_rx).await,
            Some(ServerMsg::Welcome { .. })
        ));

        let (user_ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("user connect");
        let (mut user_tx, mut user_rx) = user_ws.split();
        send_client(&mut user_tx, &hello("user")).await;
        assert!(matches!(
            recv_server(&mut user_rx).await,
            Some(ServerMsg::Welcome { .. })
        ));
        send_client(
            &mut user_tx,
            &ClientMsg::ConfigApply {
                target: fleety_protocol::ConfigTarget::Device("remote-B".to_string()),
                base_revision: "r1".to_string(),
                changes: vec![fleety_protocol::ConfigChange {
                    key: "FLEETY_UPDATE_INTERVAL_SECS".to_string(),
                    op: fleety_protocol::ChangeOp::Set,
                    value: Some("60".to_string()),
                }],
                providers_json: None,
            },
        )
        .await;

        let reply = tokio::select! {
            daemon_frame = recv_server(&mut daemon_rx) => {
                panic!("auth-disabled ConfigApply reached the Daemon as {daemon_frame:?}")
            }
            user_frame = recv_server(&mut user_rx) => user_frame,
        };
        assert!(matches!(
            reply,
            Some(ServerMsg::ConfigResult {
                ok: false,
                error: Some(fleety_protocol::WireError {
                    ref kind,
                    remediation: Some(ref remediation),
                    ..
                }),
                ..
            }) if kind == "unauthenticated" && !remediation.trim().is_empty()
        ));
        assert_eq!(
            std::fs::read(&config_path).expect("read server config"),
            config_before,
            "rejected Device mutation must not write Server configuration"
        );
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                recv_server(&mut daemon_rx)
            )
            .await
            .is_err(),
            "rejected Device mutation must emit zero RunTool frames"
        );

        let _ = std::fs::remove_dir_all(home);
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

    #[tokio::test]
    async fn management_messages_replay_audit_and_rollback_over_websocket() {
        use crate::echo::EchoProvider;

        let home = std::env::temp_dir().join(format!("fleety-mgmt-{}", uuid::Uuid::new_v4()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let storage = Arc::new(Storage::new(home.clone()));
        storage
            .append("dev", "c1", &Message::user("old question"))
            .expect("append user");
        storage
            .append("dev", "c1", &Message::assistant("old answer"))
            .expect("append assistant");
        storage
            .append_history(
                "dev",
                &Event::ToolResult {
                    id: "hist-1".into(),
                    result: serde_json::json!({ "ok": true }),
                },
            )
            .expect("history");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        {
            let storage = Arc::clone(&storage);
            let workspace = Arc::new(workspace.clone());
            tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await {
                    let _ = handle_conn(
                        stream,
                        storage,
                        Arc::new(EchoProvider),
                        workspace,
                        Policy::FullAccess,
                        bridge::new_hub(),
                        bridge::new_pending(),
                        bridge::new_handles(),
                        open_auth(),
                        bridge::new_device_tools(),
                    )
                    .await;
                }
            });
        }

        let url = format!("ws://{addr}");
        let (client, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();
        send_client(&mut tx, &hello("dev")).await;
        assert!(matches!(
            recv_server(&mut rx).await,
            Some(ServerMsg::Welcome { .. })
        ));

        send_client(
            &mut tx,
            &ClientMsg::Resume {
                conversation_id: "c1".into(),
                after_seq: 1,
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::Replay {
                seq, role, content, ..
            }) => {
                assert_eq!(seq, 2);
                assert_eq!(role, "assistant");
                assert_eq!(content, "old answer");
            }
            other => panic!("expected replay, got {other:?}"),
        }
        assert!(matches!(
            recv_server(&mut rx).await,
            Some(ServerMsg::Done { .. })
        ));

        send_client(
            &mut tx,
            &ClientMsg::AuditList {
                device_id: "dev".into(),
                since: Some(1),
                limit: Some(10),
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::AuditListResult { entries_json, .. }) => {
                assert!(entries_json.contains("tool_result"));
            }
            other => panic!("expected audit list, got {other:?}"),
        }

        send_client(
            &mut tx,
            &ClientMsg::AuditShow {
                device_id: "dev".into(),
                index: 0,
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::AuditShowResult { event_json, .. }) => {
                assert!(event_json.contains("tool_result"));
            }
            other => panic!("expected audit show, got {other:?}"),
        }

        send_client(
            &mut tx,
            &ClientMsg::AuditShow {
                device_id: "dev".into(),
                index: 99,
            },
        )
        .await;
        assert!(matches!(
            recv_server(&mut rx).await,
            Some(ServerMsg::Error { .. })
        ));

        send_client(
            &mut tx,
            &ClientMsg::RollbackList {
                device_id: "dev".into(),
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::RollbackListResult { backups_json, .. }) => {
                assert_eq!(backups_json, "[]");
            }
            other => panic!("expected rollback list, got {other:?}"),
        }

        send_client(
            &mut tx,
            &ClientMsg::RollbackApply {
                device_id: "dev".into(),
                backup_id: "missing".into(),
            },
        )
        .await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::RollbackResult { ok, message, .. }) => {
                assert!(!ok);
                assert!(message.contains("missing"));
            }
            other => panic!("expected rollback result, got {other:?}"),
        }
        let _ = tx.close().await;
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `ConversationList` returns a `ConversationListResult` scoped to the acting
    /// user (the connecting device's owner): the owner's conversations appear,
    /// another user's never do.
    #[tokio::test]
    async fn conversation_list_returns_owner_scoped_result() {
        use crate::echo::EchoProvider;
        use crate::identity::ActingUser;

        let home = std::env::temp_dir().join(format!("fleety-convlist-{}", uuid::Uuid::new_v4()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let storage = Arc::new(Storage::new(home.clone()));
        // Device "dev" is owned by alice → the listing is scoped to alice.
        storage
            .set_device_ownership("dev", Some("alice"), &["alice".to_string()], false)
            .expect("own");
        // Alice's conversation (user-primary) and bob's (a different owner).
        storage
            .register_conversation_owner("c-alice", &ActingUser::User("alice".into()))
            .expect("own alice conv");
        storage
            .append("dev", "c-alice", &Message::user("alice question"))
            .expect("append alice");
        storage
            .register_conversation_owner("c-bob", &ActingUser::User("bob".into()))
            .expect("own bob conv");
        storage
            .append("dev", "c-bob", &Message::user("bob question"))
            .expect("append bob");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        {
            let storage = Arc::clone(&storage);
            let workspace = Arc::new(workspace.clone());
            tokio::spawn(async move {
                if let Ok((stream, _)) = listener.accept().await {
                    let _ = handle_conn(
                        stream,
                        storage,
                        Arc::new(EchoProvider),
                        workspace,
                        Policy::FullAccess,
                        bridge::new_hub(),
                        bridge::new_pending(),
                        bridge::new_handles(),
                        open_auth(),
                        bridge::new_device_tools(),
                    )
                    .await;
                }
            });
        }

        let url = format!("ws://{addr}");
        let (client, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let (mut tx, mut rx) = client.split();
        send_client(&mut tx, &hello("dev")).await;
        assert!(matches!(
            recv_server(&mut rx).await,
            Some(ServerMsg::Welcome { .. })
        ));

        send_client(&mut tx, &ClientMsg::ConversationList { limit: Some(10) }).await;
        match recv_server(&mut rx).await {
            Some(ServerMsg::ConversationListResult { conversations_json }) => {
                let rows: Vec<serde_json::Value> =
                    serde_json::from_str(&conversations_json).expect("parse rows");
                let ids: Vec<&str> = rows
                    .iter()
                    .filter_map(|r| r["conversation_id"].as_str())
                    .collect();
                assert!(ids.contains(&"c-alice"), "owner's conversation is listed");
                assert!(!ids.contains(&"c-bob"), "another user's is not listed");
                let alice = rows
                    .iter()
                    .find(|r| r["conversation_id"] == serde_json::json!("c-alice"))
                    .expect("alice row");
                assert_eq!(alice["preview"], serde_json::json!("alice question"));
            }
            other => panic!("expected conversation list, got {other:?}"),
        }
        let _ = tx.close().await;
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
            ModelResponse::new(Message {
                role: CoreRole::Assistant,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "c1".to_string(),
                    name: "write_file".to_string(),
                    arguments: serde_json::json!({ "path": "x.txt", "content": "hi" }),
                }],
                tool_call_id: None,
                attachments: Vec::new(),
            }),
            ModelResponse::new(Message::assistant("done")),
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
                message_id: "please-write".into(),
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
    async fn provider_failure_ends_only_the_turn_and_keeps_the_connection_open() {
        let home =
            std::env::temp_dir().join(format!("fleety-provider-failure-{}", uuid::Uuid::new_v4()));
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(home.clone());
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(Vec::new()));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_storage = Arc::clone(&storage);
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(
                    stream,
                    server_storage,
                    provider,
                    workspace,
                    Policy::FullAccess,
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
                message_id: "provider-fail".into(),
                conversation_id: None,
                text: "make the provider fail".into(),
                origin: Default::default(),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            },
        )
        .await;

        assert!(matches!(
            recv_server(&mut crx).await,
            Some(ServerMsg::Error { error }) if error.kind == "provider"
        ));
        assert!(matches!(
            recv_server(&mut crx).await,
            Some(ServerMsg::Done { .. })
        ));
        assert!(
            storage
                .list_incomplete_turns()
                .expect("list journals")
                .is_empty(),
            "a reported provider failure must not replay as an interrupted turn"
        );

        send_client(&mut ctx, &ClientMsg::ServerStatus).await;
        assert!(matches!(
            recv_server(&mut crx).await,
            Some(ServerMsg::ServerStatusResult { .. })
        ));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn failed_journal_cleanup_never_reports_the_turn_done() {
        let home = std::env::temp_dir().join(format!(
            "fleety-provider-cleanup-failure-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = Arc::new(Storage::new(home.clone()));
        storage
            .journal_begin("d", "c", &Message::user("failed turn"))
            .expect("begin journal");
        let journal = home
            .join("fleet")
            .join("devices")
            .join("d")
            .join("conversations")
            .join("c.journal.jsonl");
        std::fs::remove_file(&journal).expect("remove journal file");
        std::fs::create_dir(&journal).expect("replace journal with a directory");
        let (out, mut rx) = mpsc::unbounded_channel();

        let result = finish_provider_turn_failure(
            &out,
            &storage,
            "d",
            "c",
            &CoreError::Provider("model unavailable".to_string()),
            &["queued-1".to_string(), "queued-2".to_string()],
        );
        let frames = drain(&mut rx);

        assert!(result.is_err(), "cleanup failure must end the connection");
        assert!(
            frames
                .iter()
                .any(|frame| matches!(frame, ServerMsg::Error { .. })),
            "the original provider failure is still surfaced"
        );
        assert_eq!(
            frames
                .iter()
                .filter(|frame| matches!(frame, ServerMsg::Error { .. }))
                .count(),
            1,
            "one provider failure must retire only the active turn"
        );
        let rejected_ids: Vec<&str> = frames
            .iter()
            .filter_map(|frame| match frame {
                ServerMsg::InterjectionAcknowledged {
                    message_id,
                    disposition: InterjectionDisposition::Rejected,
                    ..
                } => Some(message_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rejected_ids, ["queued-1", "queued-2"]);
        let error_index = frames
            .iter()
            .position(|frame| matches!(frame, ServerMsg::Error { .. }))
            .expect("cleanup error");
        assert!(
            frames[..error_index].iter().all(|frame| matches!(
                frame,
                ServerMsg::InterjectionAcknowledged {
                    disposition: InterjectionDisposition::Rejected,
                    ..
                }
            )),
            "seeing the terminal Error must prove every queued rejection was delivered first"
        );
        assert!(
            !frames
                .iter()
                .any(|frame| matches!(frame, ServerMsg::Done { .. })),
            "an incomplete journal must never be declared done"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn protocol_boundary_rejects_older_clients() {
        assert!(protocol_is_supported(PROTOCOL_VERSION));
        assert!(!protocol_is_supported(PROTOCOL_VERSION - 1));
        assert!(!protocol_is_supported(PROTOCOL_VERSION + 1));
    }

    #[test]
    fn client_message_id_boundary_is_nonempty_and_bounded() {
        assert!(!client_message_id_is_valid(""));
        assert!(client_message_id_is_valid(
            &"x".repeat(MAX_CLIENT_MESSAGE_ID_BYTES)
        ));
        assert!(!client_message_id_is_valid(
            &"x".repeat(MAX_CLIENT_MESSAGE_ID_BYTES + 1)
        ));
    }

    struct FailFirstTurnAfterInterjection {
        first_started: Arc<tokio::sync::Notify>,
        release_first: Arc<tokio::sync::Notify>,
        calls: std::sync::atomic::AtomicUsize,
    }

    struct FailFirstTurnAfterTwoInterjections {
        first_started: Arc<tokio::sync::Notify>,
        release_first: Arc<tokio::sync::Notify>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ModelProvider for FailFirstTurnAfterTwoInterjections {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> agent_core::Result<ModelResponse> {
            match self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => {
                    self.first_started.notify_one();
                    self.release_first.notified().await;
                    Err(CoreError::Provider("first turn failed".to_string()))
                }
                1 | 2 => Ok(ModelResponse::new(Message::assistant("queue_after"))),
                3 => Ok(ModelResponse::new(Message::assistant("second answer"))),
                _ => Ok(ModelResponse::new(Message::assistant("third answer"))),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for FailFirstTurnAfterInterjection {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> agent_core::Result<ModelResponse> {
            match self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => {
                    self.first_started.notify_one();
                    self.release_first.notified().await;
                    Err(CoreError::Provider("first turn failed".to_string()))
                }
                1 => Ok(ModelResponse::new(Message::assistant("queue_after"))),
                _ => Ok(ModelResponse::new(Message::assistant("second answer"))),
            }
        }
    }

    #[tokio::test]
    async fn queued_interjection_survives_a_provider_failure_in_the_active_turn() {
        let home = std::env::temp_dir().join(format!(
            "fleety-provider-interjection-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(home.clone());
        let first_started = Arc::new(tokio::sync::Notify::new());
        let release_first = Arc::new(tokio::sync::Notify::new());
        let provider: Arc<dyn ModelProvider> = Arc::new(FailFirstTurnAfterInterjection {
            first_started: Arc::clone(&first_started),
            release_first: Arc::clone(&release_first),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_storage = Arc::clone(&storage);
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(
                    stream,
                    server_storage,
                    provider,
                    workspace,
                    Policy::FullAccess,
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

        let message = |text: &str| ClientMsg::UserMessage {
            message_id: text.to_string(),
            conversation_id: None,
            text: text.to_string(),
            origin: Default::default(),
            attachments: Vec::new(),
            voice: false,
            acting_user: None,
        };
        send_client(&mut ctx, &message("first message")).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), first_started.notified())
            .await
            .expect("first provider turn started");
        send_client(&mut ctx, &message("second message")).await;

        let ack = tokio::time::timeout(std::time::Duration::from_secs(2), recv_server(&mut crx))
            .await
            .expect("interjection acknowledgement");
        let conversation = match ack {
            Some(ServerMsg::InterjectionAcknowledged {
                conversation_id,
                disposition: InterjectionDisposition::Queued,
                message,
                ..
            }) if message.contains("right after") => conversation_id,
            other => panic!("expected queued-interjection acknowledgement, got {other:?}"),
        };
        release_first.notify_one();

        let mut saw_first_error = false;
        let mut saw_second_answer = false;
        for _ in 0..6 {
            let frame =
                tokio::time::timeout(std::time::Duration::from_secs(2), recv_server(&mut crx))
                    .await
                    .expect("turn result before timeout");
            match frame {
                Some(ServerMsg::Error { error }) if error.message.contains("first turn") => {
                    saw_first_error = true;
                }
                Some(ServerMsg::Assistant { text, .. }) if text == "second answer" => {
                    saw_second_answer = true;
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(saw_first_error, "the failed active turn is reported");
        assert!(
            saw_second_answer,
            "the acknowledged queued interjection must still run"
        );

        let history = storage
            .load("d", &conversation)
            .expect("conversation history");
        assert!(history.iter().any(|message| {
            message.role == CoreRole::User && message.content.as_deref() == Some("second message")
        }));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn multiple_queued_interjections_survive_in_fifo_order() {
        let home = std::env::temp_dir().join(format!(
            "fleety-provider-interjection-fifo-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(home.clone());
        let first_started = Arc::new(tokio::sync::Notify::new());
        let release_first = Arc::new(tokio::sync::Notify::new());
        let provider: Arc<dyn ModelProvider> = Arc::new(FailFirstTurnAfterTwoInterjections {
            first_started: Arc::clone(&first_started),
            release_first: Arc::clone(&release_first),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server_storage = Arc::clone(&storage);
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn(
                    stream,
                    server_storage,
                    provider,
                    workspace,
                    Policy::FullAccess,
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
        let message = |text: &str| ClientMsg::UserMessage {
            message_id: text.to_string(),
            conversation_id: None,
            text: text.to_string(),
            origin: Default::default(),
            attachments: Vec::new(),
            voice: false,
            acting_user: None,
        };

        send_client(&mut ctx, &message("first message")).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), first_started.notified())
            .await
            .expect("first provider turn started");
        let mut conversation = None;
        for text in ["second message", "third message"] {
            send_client(&mut ctx, &message(text)).await;
            match tokio::time::timeout(std::time::Duration::from_secs(2), recv_server(&mut crx))
                .await
                .expect("interjection acknowledgement")
            {
                Some(ServerMsg::InterjectionAcknowledged {
                    conversation_id,
                    disposition: InterjectionDisposition::Queued,
                    message,
                    ..
                }) if message.contains("right after") => conversation = Some(conversation_id),
                other => panic!("expected queued-interjection acknowledgement, got {other:?}"),
            }
        }
        release_first.notify_one();

        let mut answers = Vec::new();
        for _ in 0..10 {
            match tokio::time::timeout(std::time::Duration::from_secs(2), recv_server(&mut crx))
                .await
                .expect("turn result before timeout")
            {
                Some(ServerMsg::Assistant { text, .. })
                    if text == "second answer" || text == "third answer" =>
                {
                    answers.push(text);
                    if answers.len() == 2 {
                        break;
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
        assert_eq!(answers, ["second answer", "third answer"]);

        let conversation = conversation.expect("generated conversation");
        let history = storage.load("d", &conversation).expect("history");
        let users: Vec<&str> = history
            .iter()
            .filter(|message| message.role == CoreRole::User)
            .filter_map(|message| message.content.as_deref())
            .collect();
        assert!(users.ends_with(&["second message", "third message"]));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn user_prompt_submit_hook_blocks_the_turn_end_to_end() {
        // End-to-end through the serve loop: a same-host conversation whose origin
        // cwd declares a UserPromptSubmit hook that exits non-zero must have its
        // prompt blocked — the provider is never reached, and the client is told a
        // hook blocked it. Exercises bind → collect_conversation_hooks →
        // conv_hook_ctx → run_conversation_event_hooks → block/emit/continue with
        // the real local shell runner.
        let home = std::env::temp_dir().join(format!("fleety-upshook-{}", uuid::Uuid::new_v4()));
        let ws_root = home.join("ws");
        let project = home.join("proj");
        std::fs::create_dir_all(&ws_root).expect("mk ws");
        std::fs::create_dir_all(project.join(".claude")).expect("mk proj/.claude");
        std::fs::write(
            project.join(".claude").join("settings.json"),
            r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":"exit 1"}]}]}}"#,
        )
        .expect("w settings");

        // Isolate the user-scope home read so the serve loop can't touch the real
        // ~/.claude, and make sure project hooks aren't disabled by a stray env.
        let saved_home = std::env::var("HOME").ok();
        let saved_loopback_trust = std::env::var("FLEETY_TRUST_LOOPBACK").ok();
        std::env::set_var("HOME", &home);
        std::env::remove_var("FLEETY_DISABLE_PROJECT_HOOKS");
        std::env::remove_var("FLEETY_TRUST_LOOPBACK");

        // If the block fails, the provider would answer "processed" — a clear
        // failure signal versus the expected "blocked" notice.
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(vec![ModelResponse::new(
                Message::assistant("PROVIDER_REACHED_SENTINEL"),
            )]));
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(ws_root.clone());
        let auth = Arc::new(AuthStore::load(home.join("auth.json"), None, true));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn_with_peer(
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
                    true,
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
                message_id: "hook-turn".into(),
                conversation_id: None,
                text: "please do the thing".into(),
                // Same host (hostname == server) so hooks are collected locally,
                // rooted at the project dir holding the UserPromptSubmit hook.
                origin: fleety_protocol::OriginContext {
                    hostname: Some(server_hostname()),
                    os: Some("test".into()),
                    cwd: Some(project.to_string_lossy().into_owned()),
                    home: None,
                },
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            },
        )
        .await;

        let mut reply = None;
        for _ in 0..10 {
            match recv_server(&mut crx).await {
                Some(ServerMsg::Assistant { text, .. }) => {
                    reply = Some(text);
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }

        // Restore the env before asserting so a failure can't leak it.
        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match saved_loopback_trust {
            Some(value) => std::env::set_var("FLEETY_TRUST_LOOPBACK", value),
            None => std::env::remove_var("FLEETY_TRUST_LOOPBACK"),
        }

        let reply = reply.expect("server should reply");
        assert!(
            reply.contains("blocked"),
            "UserPromptSubmit hook should block the prompt, got: {reply}"
        );
        assert!(
            !reply.contains("PROVIDER_REACHED_SENTINEL"),
            "the provider must not run when the prompt is blocked"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn pretooluse_hook_denies_a_tool_end_to_end() {
        // End-to-end: a same-host conversation whose origin declares a PreToolUse
        // hook (via the Claude name "Write", exercising the tool-name alias) that
        // exits non-zero must have the agent's write_file call denied — the file
        // is never written — while the turn still finishes. Exercises the tool
        // wrapper inside a live turn with the real local shell runner.
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse::new(Message {
                role: CoreRole::Assistant,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "c1".to_string(),
                    name: "write_file".to_string(),
                    arguments: serde_json::json!({ "path": "x.txt", "content": "hi" }),
                }],
                tool_call_id: None,
                attachments: Vec::new(),
            }),
            ModelResponse::new(Message::assistant("done")),
        ]));

        let home = std::env::temp_dir().join(format!("fleety-pretool-{}", uuid::Uuid::new_v4()));
        let ws_root = home.join("ws");
        let project = home.join("proj");
        std::fs::create_dir_all(&ws_root).expect("mk ws");
        std::fs::create_dir_all(project.join(".claude")).expect("mk proj/.claude");
        std::fs::write(
            project.join(".claude").join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"Write","hooks":[{"type":"command","command":"exit 1"}]}]}}"#,
        )
        .expect("w settings");

        let saved_home = std::env::var("HOME").ok();
        let saved_loopback_trust = std::env::var("FLEETY_TRUST_LOOPBACK").ok();
        std::env::set_var("HOME", &home);
        std::env::remove_var("FLEETY_DISABLE_PROJECT_HOOKS");
        std::env::remove_var("FLEETY_TRUST_LOOPBACK");

        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = Arc::new(ws_root.clone());
        let auth = Arc::new(AuthStore::load(home.join("auth.json"), None, true));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let _ = handle_conn_with_peer(
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
                    true,
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
                message_id: "write-file".into(),
                conversation_id: None,
                text: "please write the file".into(),
                origin: fleety_protocol::OriginContext {
                    hostname: Some(server_hostname()),
                    os: Some("test".into()),
                    cwd: Some(project.to_string_lossy().into_owned()),
                    home: None,
                },
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            },
        )
        .await;

        let mut saw_done = false;
        for _ in 0..12 {
            match recv_server(&mut crx).await {
                Some(ServerMsg::Done { .. }) | None => {
                    saw_done = true;
                    break;
                }
                _ => {}
            }
        }

        match saved_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match saved_loopback_trust {
            Some(value) => std::env::set_var("FLEETY_TRUST_LOOPBACK", value),
            None => std::env::remove_var("FLEETY_TRUST_LOOPBACK"),
        }

        assert!(saw_done, "turn should complete");
        // Same-host tools are rooted at the origin cwd; the denied write must not
        // have created the file there (nor in the server workspace fallback).
        assert!(
            !project.join("x.txt").exists(),
            "PreToolUse-denied write must not happen"
        );
        assert!(
            !ws_root.join("x.txt").exists(),
            "no write in the fallback either"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A provider that parks the first model call on a `Notify` so a turn is
    /// guaranteed in flight when the test sends `CancelTurn`; once released it
    /// returns a terminal reply. Deterministic mid-turn timing without a
    /// platform-specific slow tool.
    struct GateProvider {
        gate: Arc<tokio::sync::Notify>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ModelProvider for GateProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> agent_core::Result<ModelResponse> {
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                self.gate.notified().await;
            }
            Ok(ModelResponse::new(Message::assistant(
                "partial work before cancel",
            )))
        }
    }

    /// End-to-end: while a turn is in flight (provider parked), an explicit
    /// `CancelTurn` frame produces an immediate ack, then the turn closes with
    /// the "Cancelled at your request" wording and a Done.
    #[tokio::test]
    async fn cancel_turn_acks_then_closes_with_cancelled_wording() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let provider: Arc<dyn ModelProvider> = Arc::new(GateProvider {
            gate: Arc::clone(&gate),
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        let home = std::env::temp_dir().join(format!("fleety-cancel-{}", uuid::Uuid::new_v4()));
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
                    Policy::FullAccess,
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
                message_id: "long-turn".into(),
                conversation_id: None,
                text: "do a long thing".into(),
                origin: OriginContext::default(),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            },
        )
        .await;

        // The turn is now parked at the model call. Cancel it.
        send_client(
            &mut ctx,
            &ClientMsg::CancelTurn {
                conversation_id: None,
            },
        )
        .await;

        // First frame back is the immediate ack.
        let ack = recv_server(&mut crx).await;
        assert!(
            matches!(&ack, Some(ServerMsg::Assistant { text, .. }) if text.contains("cancelling")),
            "expected a cancelling ack, got {ack:?}"
        );

        // Release the provider so the turn winds down.
        gate.notify_one();

        // The closing reply carries the explicit-cancel wording, then Done.
        let mut saw_cancelled = false;
        let mut saw_done = false;
        for _ in 0..8 {
            match recv_server(&mut crx).await {
                Some(ServerMsg::Assistant { text, .. })
                    if text.contains("Cancelled at your request") =>
                {
                    saw_cancelled = true;
                }
                Some(ServerMsg::Done { .. }) => {
                    saw_done = true;
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(
            saw_cancelled,
            "closing reply should say it was cancelled at the user's request"
        );
        assert!(saw_done, "turn should complete with Done");

        let _ = std::fs::remove_dir_all(&home);
        let _ = ctx.close().await;
    }

    /// An idle `CancelTurn` (no turn running) is ignored silently — the server
    /// emits nothing for it, so a following ping-like turn still works.
    #[tokio::test]
    async fn idle_cancel_turn_is_ignored_silently() {
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(vec![ModelResponse::new(
                Message::assistant("hello there"),
            )]));
        let home = std::env::temp_dir().join(format!("fleety-idlecancel-{}", uuid::Uuid::new_v4()));
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
                    Policy::FullAccess,
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

        // Cancel with nothing running — must produce no frame.
        send_client(
            &mut ctx,
            &ClientMsg::CancelTurn {
                conversation_id: None,
            },
        )
        .await;

        // A real turn afterward still works (proving the idle cancel neither
        // emitted a frame nor wedged the connection).
        send_client(
            &mut ctx,
            &ClientMsg::UserMessage {
                message_id: "after-idle-cancel".into(),
                conversation_id: None,
                text: "hi".into(),
                origin: OriginContext::default(),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            },
        )
        .await;

        // The very next frame is the turn's reply — not a stray cancel artifact.
        let first = recv_server(&mut crx).await;
        assert!(
            matches!(&first, Some(ServerMsg::Assistant { text, .. }) if text.contains("hello there")),
            "idle cancel must emit nothing; first frame should be the new turn's reply, got {first:?}"
        );

        let _ = std::fs::remove_dir_all(&home);
        let _ = ctx.close().await;
    }

    #[tokio::test]
    async fn device_exec_routes_to_daemon_and_returns() {
        // The user's agent calls device_exec -> server routes RunTool to the "pi"
        // daemon connection -> daemon replies -> the result returns to the agent
        // loop and is audited. Exercises the full three-party bridge.
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse::new(Message {
                role: CoreRole::Assistant,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "c1".to_string(),
                    name: "device_exec".to_string(),
                    arguments: serde_json::json!({ "device": "pi", "tool": "read_file", "args": { "path": "x" } }),
                }],
                tool_call_id: None,
                attachments: Vec::new(),
            }),
            ModelResponse::new(Message::assistant("done")),
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
        send_client(
            &mut dtx,
            &ClientMsg::Hello {
                device_id: "pi".into(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: None,
                local_tools_json: Some(
                    serde_json::to_string(&vec![agent_core::ToolSpec {
                        name: "read_file".to_string(),
                        description: "test daemon tool".to_string(),
                        parameters: serde_json::json!({}),
                        risk: agent_core::RiskLevel::Read,
                    }])
                    .expect("tool specs"),
                ),
                hostname: None,
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
        send_client(&mut utx, &hello("user")).await;
        assert!(matches!(
            recv_server(&mut urx).await,
            Some(ServerMsg::Welcome { .. })
        ));
        send_client(
            &mut utx,
            &ClientMsg::UserMessage {
                message_id: "device-exec".into(),
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

    #[test]
    fn oauth_catalog_error_keeps_backend_detail_but_api_error_hides_endpoint_detail() {
        let oauth = model_discovery_error_message(
            "tingzhen-codex",
            true,
            "Codex model catalog returned HTTP 403: account is not eligible",
        );
        assert!(oauth.contains("HTTP 403"), "{oauth}");
        assert!(oauth.contains("account is not eligible"), "{oauth}");

        let api = model_discovery_error_message(
            "custom",
            false,
            "request failed for https://user:secret@example.test/models?token=secret",
        );
        assert_eq!(api, "could not fetch models for provider 'custom'");
        assert!(!api.contains("secret"));
    }
}
