//! Subagent delegation.
//!
//! The parent agent delegates a task to a **subagent**: a nested
//! [`agent_core::run_turn`] loop with its own messages, a chosen model tier, and
//! a tool registry equal to the parent's MINUS the orchestration tools. Because a
//! subagent has no orchestration tools it cannot spawn further subagents — a
//! one-level nesting cap enforced by tool absence. A subagent keeps every other
//! tool (including `device_exec`, so it can still act on other devices).
//!
//! Modes: `spawn` (fresh context seeded by the briefing) and `fork` (inherits
//! the parent conversation). Either runs on the `main` or `cheap` tier. A
//! foreground call awaits the subagent and returns its output; a background call
//! returns a `task_id` immediately, runs on a tokio task, and on completion
//! proactively drives a coordinator turn (via `conn::drive_turn`, under a shared
//! turn lock) seeded with the result — mirroring Claude Code's auto
//! re-invocation.
//!
//! The tier only changes which provider runs — never the policy, gate, or audit.
//! Subagents use a non-interactive gate: under full access their mutating tools
//! run; under require-approval they are limited to read tools unless
//! `allowed_tools` pre-grants specific tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use agent_core::{
    run_turn, ApprovalGate, AutoApprove, AutoDeny, CoreError, EventLog, LoopConfig, MandateGate,
    Message, Policy, Result, RiskLevel, Tool, ToolRegistry, ToolSpec,
};

use crate::auth::AuthStore;
use crate::bridge::{DeviceTools, Handles, Hub, Pending};
use crate::conn::{drive_turn, Out};
use crate::providers::ProviderTiers;
use crate::storage::Storage;

/// A subagent task's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentState {
    Spawned,
    Running,
    Done,
    Failed,
    Stopped,
}

impl SubagentState {
    fn as_str(self) -> &'static str {
        match self {
            SubagentState::Spawned => "spawned",
            SubagentState::Running => "running",
            SubagentState::Done => "done",
            SubagentState::Failed => "failed",
            SubagentState::Stopped => "stopped",
        }
    }
    fn is_terminal(self) -> bool {
        matches!(
            self,
            SubagentState::Done | SubagentState::Failed | SubagentState::Stopped
        )
    }
}

/// Spawn / fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Spawn,
    Fork,
}

/// Isolation for a subagent's workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Isolation {
    None,
    Worktree,
}

/// A parsed spawn request.
struct SpawnReq {
    mode: Mode,
    tier: String,
    prompt: String,
    allowed_tools: Vec<String>,
    isolation: Isolation,
    conversation: String,
}

/// One tracked subagent task.
struct TaskRecord {
    name: Option<String>,
    state: SubagentState,
    /// The subagent's running messages, kept for `send_subagent_message`.
    messages: Vec<Message>,
    output: Option<String>,
    tier: String,
    handle: Option<tokio::task::JoinHandle<()>>,
    worktree: Option<PathBuf>,
}

/// Shared subagent runtime. Holds the model tiers, the dependencies needed to
/// build a subagent's (orchestration-free) registry, the task registry, the
/// client `out` sink, and a turn lock that serializes proactive wake turns
/// against the connection's own user turns.
pub struct SubagentRuntime {
    providers: ProviderTiers,
    policy: Policy,
    max_concurrent: usize,
    storage: Arc<Storage>,
    workspace: PathBuf,
    device_id: String,
    hub: Hub,
    pending: Pending,
    handles: Handles,
    auth: Arc<AuthStore>,
    device_tools: DeviceTools,
    out: Out,
    tasks: Mutex<HashMap<String, TaskRecord>>,
    running: AtomicU64,
    seq: AtomicU64,
    active_conversation: Mutex<String>,
    /// Serializes all turns on this connection (user turns + wake turns) so a
    /// background completion never interleaves storage appends with a live turn.
    turn_lock: Mutex<()>,
}

/// Concurrency cap default; override with `FLEETY_SUBAGENT_MAX_CONCURRENT`.
const DEFAULT_MAX_CONCURRENT: usize = 4;

impl SubagentRuntime {
    /// Build a runtime for one connection.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        providers: ProviderTiers,
        policy: Policy,
        storage: Arc<Storage>,
        workspace: PathBuf,
        device_id: String,
        hub: Hub,
        pending: Pending,
        handles: Handles,
        auth: Arc<AuthStore>,
        device_tools: DeviceTools,
        out: Out,
    ) -> Arc<Self> {
        let max_concurrent = std::env::var("FLEETY_SUBAGENT_MAX_CONCURRENT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|n| n.max(1))
            .unwrap_or(DEFAULT_MAX_CONCURRENT);
        Arc::new(Self {
            providers,
            policy,
            max_concurrent,
            storage,
            workspace,
            device_id,
            hub,
            pending,
            handles,
            auth,
            device_tools,
            out,
            tasks: Mutex::new(HashMap::new()),
            running: AtomicU64::new(0),
            seq: AtomicU64::new(1),
            active_conversation: Mutex::new(String::new()),
            turn_lock: Mutex::new(()),
        })
    }

    /// Record which conversation the parent is currently driving, so a `fork`
    /// subagent inherits the right conversation. Set by `conn` before each turn.
    pub async fn set_active_conversation(&self, conversation: &str) {
        *self.active_conversation.lock().await = conversation.to_string();
    }

    /// Acquire the per-connection turn lock. `conn` holds this for the duration
    /// of every user turn so a background completion's wake turn cannot
    /// interleave storage appends with a live turn.
    pub async fn lock_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.turn_lock.lock().await
    }

    /// Build a subagent's tool registry: the full parent tool set MINUS the
    /// orchestration tools (which are only ever added at the top level), so the
    /// subagent cannot itself spawn subagents.
    fn build_registry(&self, workspace: &Path) -> ToolRegistry {
        crate::conn::build_full_registry(
            &self.storage,
            workspace,
            &self.device_id,
            &self.hub,
            &self.pending,
            &self.handles,
            &self.auth,
            &self.device_tools,
        )
    }

    /// The non-interactive gate for a subagent run. The tier never changes this.
    fn make_gate(&self, allowed_tools: &[String]) -> Box<dyn ApprovalGate + Send> {
        match self.policy {
            Policy::FullAccess => Box::new(AutoApprove),
            Policy::RequireApproval => {
                if allowed_tools.is_empty() {
                    Box::new(AutoDeny)
                } else {
                    Box::new(MandateGate::new(allowed_tools.iter().cloned()))
                }
            }
        }
    }

    /// Initial messages for a run: spawn → fresh system + briefing; fork →
    /// the conversation history plus the briefing as a new user message.
    async fn initial_messages(&self, mode: Mode, conversation: &str, prompt: &str) -> Vec<Message> {
        let system = self.storage.system_prompt().unwrap_or_default();
        match mode {
            Mode::Spawn => vec![Message::system(system), Message::user(prompt)],
            Mode::Fork => {
                // Mirror a real turn: system preamble, then the inherited
                // conversation, then the new briefing.
                let mut msgs = vec![Message::system(system)];
                msgs.extend(
                    self.storage
                        .load(&self.device_id, conversation)
                        .unwrap_or_default(),
                );
                msgs.push(Message::user(prompt));
                msgs
            }
        }
    }

    /// Run one nested agent loop over `messages` and return the terminal state,
    /// output, and the (possibly extended) messages. Never panics: a subagent
    /// error becomes a `Failed` state with the error summary as output.
    async fn run_loop(
        &self,
        mut messages: Vec<Message>,
        tier: &str,
        allowed_tools: &[String],
        workspace: &Path,
    ) -> (SubagentState, String, Vec<Message>) {
        let registry = self.build_registry(workspace);
        let provider = self.providers.resolve(tier);
        let mut gate = self.make_gate(allowed_tools);
        let mut events = EventLog::new();
        let cfg = LoopConfig::default();
        let outcome = run_turn(
            provider.as_ref(),
            &registry,
            &mut messages,
            &mut events,
            &cfg,
            self.policy,
            gate.as_mut(),
        )
        .await;
        // Audit: every subagent action is recorded to the parent device's log.
        for ev in events.events() {
            let _ = self.storage.append_history(&self.device_id, ev);
        }
        match outcome {
            Ok(o) => (SubagentState::Done, o.output, messages),
            Err(e) => (
                SubagentState::Failed,
                format!("subagent failed: {e}"),
                messages,
            ),
        }
    }

    /// Reserve a fresh task id.
    fn next_id(&self) -> String {
        format!("sub-{}", self.seq.fetch_add(1, Ordering::SeqCst))
    }
}

/// Resolve the workspace for a run, creating a git worktree when isolation is
/// `Worktree`. Returns the workspace path and an optional worktree dir to remove
/// after the run.
fn resolve_workspace(
    base: &Path,
    isolation: Isolation,
    task_id: &str,
) -> Result<(PathBuf, Option<PathBuf>)> {
    match isolation {
        Isolation::None => Ok((base.to_path_buf(), None)),
        Isolation::Worktree => {
            if !base.join(".git").exists() {
                return Err(CoreError::Message(format!(
                    "isolation=\"worktree\" needs a git repository at the workspace ({}); \
                     use isolation=\"none\" or run in a git repo",
                    base.display()
                )));
            }
            let dir = base.join(".fleety").join("worktrees").join(task_id);
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(base)
                .arg("worktree")
                .arg("add")
                .arg("--detach")
                .arg(&dir)
                .output()
                .map_err(|e| {
                    CoreError::Message(format!("git worktree add failed to spawn: {e}"))
                })?;
            if !out.status.success() {
                return Err(CoreError::Message(format!(
                    "git worktree add failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
            Ok((dir.clone(), Some(dir)))
        }
    }
}

/// Remove a worktree created for a subagent (best effort; only when unchanged).
fn cleanup_worktree(base: &Path, dir: &Path) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .arg("worktree")
        .arg("remove")
        .arg(dir)
        .output();
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

fn opt_str<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn parse_allowed(args: &Value) -> Vec<String> {
    args.get("allowed_tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Register the orchestration tools on a top-level registry. NEVER call this
/// when building a subagent's registry — that is what caps nesting at one level.
pub fn register(registry: &mut ToolRegistry, runtime: Arc<SubagentRuntime>) {
    registry.register(Box::new(SpawnSubagent(Arc::clone(&runtime))));
    registry.register(Box::new(SendSubagentMessage(Arc::clone(&runtime))));
    registry.register(Box::new(StopSubagent(Arc::clone(&runtime))));
    registry.register(Box::new(SubagentStatus(runtime)));
}

struct SpawnSubagent(Arc<SubagentRuntime>);

#[async_trait]
impl Tool for SpawnSubagent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn_subagent".to_string(),
            description:
                "Delegate a task to a subagent (a nested agent with the same tools as you \
                MINUS the ability to spawn its own subagents). `mode`: \"spawn\" (fresh context + \
                this prompt) or \"fork\" (inherits the current conversation). `model`: \"main\" or \
                \"cheap\" (the economy model, if configured). `run_in_background`: true returns a \
                task_id immediately and reports back when done; false awaits and returns the \
                output. `isolation`: \"none\" or \"worktree\" (a dedicated git worktree). \
                `allowed_tools`: optional whitelist (matters only under require-approval)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "The task to delegate." },
                    "mode": { "type": "string", "enum": ["spawn", "fork"], "description": "Default \"spawn\"." },
                    "model": { "type": "string", "enum": ["main", "cheap"], "description": "Default \"main\"." },
                    "run_in_background": { "type": "boolean", "description": "Default false." },
                    "isolation": { "type": "string", "enum": ["none", "worktree"], "description": "Default \"none\"." },
                    "allowed_tools": { "type": "array", "items": { "type": "string" } },
                    "name": { "type": "string", "description": "Short task name (1-2 words)." }
                },
                "required": ["prompt"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rt = &self.0;
        let prompt = require_str(&args, "prompt")?.to_string();
        let mode = match opt_str(&args, "mode", "spawn") {
            "fork" => Mode::Fork,
            "spawn" => Mode::Spawn,
            other => return Err(CoreError::Message(format!("unknown mode '{other}'"))),
        };
        let tier = match opt_str(&args, "model", "main") {
            t @ ("main" | "cheap") => t.to_string(),
            other => return Err(CoreError::Message(format!("unknown model tier '{other}'"))),
        };
        let isolation = match opt_str(&args, "isolation", "none") {
            "worktree" => Isolation::Worktree,
            "none" => Isolation::None,
            other => return Err(CoreError::Message(format!("unknown isolation '{other}'"))),
        };
        let allowed_tools = parse_allowed(&args);
        let name = args.get("name").and_then(Value::as_str).map(str::to_string);
        let background = args
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let conversation = rt.active_conversation.lock().await.clone();

        // Validate worktree early so the error is returned synchronously.
        if isolation == Isolation::Worktree && !rt.workspace.join(".git").exists() {
            return Err(CoreError::Message(
                "isolation=\"worktree\" needs a git repository at the workspace".to_string(),
            ));
        }

        let task_id = rt.next_id();
        let req = SpawnReq {
            mode,
            tier: tier.clone(),
            prompt,
            allowed_tools,
            isolation,
            conversation: conversation.clone(),
        };

        if background {
            // Concurrency cap (background only — foreground is bounded by the
            // caller awaiting it).
            if rt.running.load(Ordering::SeqCst) as usize >= rt.max_concurrent {
                return Err(CoreError::Message(format!(
                    "too many background subagents running (cap {}); wait for one to finish or \
                     stop one with stop_subagent",
                    rt.max_concurrent
                )));
            }
            rt.tasks.lock().await.insert(
                task_id.clone(),
                TaskRecord {
                    name,
                    state: SubagentState::Spawned,
                    messages: Vec::new(),
                    output: None,
                    tier,
                    handle: None,
                    worktree: None,
                },
            );
            rt.running.fetch_add(1, Ordering::SeqCst);
            let rt2 = Arc::clone(rt);
            let id2 = task_id.clone();
            let handle = tokio::spawn(async move {
                run_background(rt2, id2, req).await;
            });
            if let Some(rec) = rt.tasks.lock().await.get_mut(&task_id) {
                rec.handle = Some(handle);
                rec.state = SubagentState::Running;
            }
            Ok(json!({ "task_id": task_id, "state": "running" }))
        } else {
            rt.tasks.lock().await.insert(
                task_id.clone(),
                TaskRecord {
                    name,
                    state: SubagentState::Running,
                    messages: Vec::new(),
                    output: None,
                    tier,
                    handle: None,
                    worktree: None,
                },
            );
            let (state, output) = run_once(rt, &task_id, req).await;
            Ok(json!({ "task_id": task_id, "state": state.as_str(), "output": output }))
        }
    }
}

/// Run a subagent to completion (foreground or the body of a background task),
/// updating the task record. Returns the terminal state and output.
async fn run_once(
    rt: &Arc<SubagentRuntime>,
    task_id: &str,
    req: SpawnReq,
) -> (SubagentState, String) {
    let (workspace, worktree) = match resolve_workspace(&rt.workspace, req.isolation, task_id) {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("{e}");
            if let Some(rec) = rt.tasks.lock().await.get_mut(task_id) {
                rec.state = SubagentState::Failed;
                rec.output = Some(msg.clone());
            }
            return (SubagentState::Failed, msg);
        }
    };
    let messages = rt
        .initial_messages(req.mode, &req.conversation, &req.prompt)
        .await;
    let (state, output, final_messages) = rt
        .run_loop(messages, &req.tier, &req.allowed_tools, &workspace)
        .await;
    if let Some(dir) = &worktree {
        cleanup_worktree(&rt.workspace, dir);
    }
    if let Some(rec) = rt.tasks.lock().await.get_mut(task_id) {
        // A stop request may have already moved it to Stopped; don't overwrite.
        if rec.state != SubagentState::Stopped {
            rec.state = state;
            rec.output = Some(output.clone());
            rec.messages = final_messages;
            rec.worktree = worktree;
        }
    }
    (state, output)
}

/// Background task body: run the subagent, then proactively drive a coordinator
/// turn seeded with the result (mirroring Claude Code's auto re-invocation).
async fn run_background(rt: Arc<SubagentRuntime>, task_id: String, req: SpawnReq) {
    let conversation = req.conversation.clone();
    let (state, output) = run_once(&rt, &task_id, req).await;
    rt.running.fetch_sub(1, Ordering::SeqCst);
    // A deliberately stopped task is not announced (the user asked to cancel it).
    if state != SubagentState::Stopped {
        drive_wake(&rt, conversation, task_id, state, output).await;
    }
}

/// Proactively resume the parent: under the turn lock, seed a coordinator turn
/// with the finished subagent's result and stream it to the user. The wake
/// happens exactly once per background task (this runs once at completion).
async fn drive_wake(
    rt: &Arc<SubagentRuntime>,
    conversation: String,
    task_id: String,
    state: SubagentState,
    output: String,
) {
    let _guard = rt.turn_lock.lock().await;
    rt.set_active_conversation(&conversation).await;
    let summary = truncate(&output, 4000);
    let seed = format!(
        "[subagent {task_id} {}] The background subagent you spawned has finished. \
         Its result:\n\n{summary}\n\nSynthesize this for the user or take any follow-up action.",
        state.as_str()
    );
    // A real parent turn: full tool set PLUS orchestration, so it may spawn again.
    let mut tools = rt.build_registry(&rt.workspace);
    register(&mut tools, Arc::clone(rt));
    let provider = rt.providers.main();
    let mut gate = AutoApprove;
    if let Err(e) = drive_turn(
        &rt.out,
        &rt.storage,
        provider.as_ref(),
        &tools,
        rt.policy,
        &rt.device_id,
        &conversation,
        Message::user(seed),
        &mut gate,
    )
    .await
    {
        tracing::warn!(task_id, error = %format!("{e}"), "subagent wake turn failed");
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…[truncated]", &s[..end])
    }
}

struct SendSubagentMessage(Arc<SubagentRuntime>);

#[async_trait]
impl Tool for SendSubagentMessage {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_subagent_message".to_string(),
            description:
                "Continue an existing (finished, not still-running) subagent with another \
                prompt, preserving its context. Returns the subagent's new output."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" },
                    "prompt": { "type": "string" }
                },
                "required": ["task_id", "prompt"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rt = &self.0;
        let task_id = require_str(&args, "task_id")?.to_string();
        let prompt = require_str(&args, "prompt")?.to_string();
        let (mut messages, tier) = {
            let tasks = rt.tasks.lock().await;
            let rec = tasks
                .get(&task_id)
                .ok_or_else(|| CoreError::Message(format!("unknown subagent task '{task_id}'")))?;
            if rec.state == SubagentState::Running {
                return Err(CoreError::Message(format!(
                    "subagent '{task_id}' is still running; wait for it to finish before sending"
                )));
            }
            (rec.messages.clone(), rec.tier.clone())
        };
        if messages.is_empty() {
            messages.push(Message::system(
                rt.storage.system_prompt().unwrap_or_default(),
            ));
        }
        messages.push(Message::user(prompt));
        if let Some(rec) = rt.tasks.lock().await.get_mut(&task_id) {
            rec.state = SubagentState::Running;
        }
        let (state, output, final_messages) =
            rt.run_loop(messages, &tier, &[], &rt.workspace).await;
        if let Some(rec) = rt.tasks.lock().await.get_mut(&task_id) {
            rec.state = state;
            rec.output = Some(output.clone());
            rec.messages = final_messages;
        }
        Ok(json!({ "task_id": task_id, "state": state.as_str(), "output": output }))
    }
}

struct StopSubagent(Arc<SubagentRuntime>);

#[async_trait]
impl Tool for StopSubagent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "stop_subagent".to_string(),
            description: "Stop a subagent. A background subagent's task is aborted; its state \
                becomes \"stopped\"."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "task_id": { "type": "string" } },
                "required": ["task_id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rt = &self.0;
        let task_id = require_str(&args, "task_id")?.to_string();
        let mut tasks = rt.tasks.lock().await;
        let rec = tasks
            .get_mut(&task_id)
            .ok_or_else(|| CoreError::Message(format!("unknown subagent task '{task_id}'")))?;
        if let Some(handle) = rec.handle.take() {
            handle.abort();
            rt.running.fetch_sub(1, Ordering::SeqCst);
        }
        rec.state = SubagentState::Stopped;
        Ok(json!({ "task_id": task_id, "state": "stopped" }))
    }
}

struct SubagentStatus(Arc<SubagentRuntime>);

#[async_trait]
impl Tool for SubagentStatus {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent_status".to_string(),
            description: "Report a subagent's current state and, when finished, its output."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "task_id": { "type": "string" } },
                "required": ["task_id"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rt = &self.0;
        let task_id = require_str(&args, "task_id")?.to_string();
        let tasks = rt.tasks.lock().await;
        let rec = tasks
            .get(&task_id)
            .ok_or_else(|| CoreError::Message(format!("unknown subagent task '{task_id}'")))?;
        Ok(json!({
            "task_id": task_id,
            "state": rec.state.as_str(),
            "name": rec.name,
            "output": rec.output,
            "terminal": rec.state.is_terminal(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MockProvider, ModelProvider, ModelResponse};
    use serial_test::serial;
    use std::sync::Mutex as StdMutex;

    fn mk_runtime(tiers: ProviderTiers) -> (Arc<SubagentRuntime>, PathBuf) {
        let home = std::env::temp_dir().join(format!("fleety-sub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        let auth = Arc::new(crate::auth::AuthStore::load(
            home.join("auth.json"),
            None,
            false,
        ));
        let (out, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Drop the receiver: foreground tests never stream; that is fine.
        drop(_rx);
        let rt = SubagentRuntime::new(
            tiers,
            Policy::FullAccess,
            storage,
            home.clone(),
            "test-dev".to_string(),
            crate::bridge::new_hub(),
            crate::bridge::new_pending(),
            crate::bridge::new_handles(),
            auth,
            crate::bridge::new_device_tools(),
            out,
        );
        (rt, home)
    }

    fn one_shot(text: &str) -> Arc<dyn ModelProvider> {
        Arc::new(MockProvider::new(vec![ModelResponse {
            message: Message::assistant(text),
        }]))
    }

    /// Records how many messages the provider was handed (to tell spawn from fork).
    struct Recording {
        seen: Arc<StdMutex<usize>>,
    }
    #[async_trait]
    impl ModelProvider for Recording {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolSpec],
        ) -> Result<ModelResponse> {
            *self.seen.lock().unwrap() = messages.len();
            Ok(ModelResponse {
                message: Message::assistant("ok"),
            })
        }
    }

    /// Blocks in `complete` until released, to hold `running` at the cap.
    struct Block {
        gate: Arc<tokio::sync::Notify>,
    }
    #[async_trait]
    impl ModelProvider for Block {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolSpec],
        ) -> Result<ModelResponse> {
            self.gate.notified().await;
            Ok(ModelResponse {
                message: Message::assistant("released"),
            })
        }
    }

    #[tokio::test]
    async fn subagent_registry_omits_orchestration_keeps_device_exec() {
        // "Capability inheritance with one-level nesting"
        let (rt, home) = mk_runtime(ProviderTiers::new(one_shot("x"), None));
        let sub = rt.build_registry(&rt.workspace);
        assert!(sub.contains("device_exec"), "subagent keeps device_exec");
        for t in [
            "spawn_subagent",
            "send_subagent_message",
            "stop_subagent",
            "subagent_status",
        ] {
            assert!(
                !sub.contains(t),
                "subagent must NOT have orchestration tool {t}"
            );
        }
        let mut top = crate::conn::build_full_registry(
            &rt.storage,
            &rt.workspace,
            &rt.device_id,
            &rt.hub,
            &rt.pending,
            &rt.handles,
            &rt.auth,
            &rt.device_tools,
        );
        register(&mut top, Arc::clone(&rt));
        assert!(
            top.contains("spawn_subagent"),
            "top level has orchestration"
        );
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn spawn_foreground_returns_subagent_output() {
        // "Spawn and fork subagents" (foreground returns output)
        let (rt, home) = mk_runtime(ProviderTiers::new(one_shot("sub-done"), None));
        let tool = SpawnSubagent(Arc::clone(&rt));
        let out = tool.call(json!({ "prompt": "do it" })).await.unwrap();
        assert_eq!(out["state"], "done");
        assert_eq!(out["output"], "sub-done");
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn model_tier_routes_to_selected_provider() {
        // "Model tier selection"
        let (rt, home) = mk_runtime(ProviderTiers::new(
            one_shot("MAIN"),
            Some(one_shot("CHEAP")),
        ));
        let tool = SpawnSubagent(Arc::clone(&rt));
        let c = tool
            .call(json!({ "prompt": "x", "model": "cheap" }))
            .await
            .unwrap();
        assert_eq!(c["output"], "CHEAP", "cheap tier runs the cheap provider");
        let m = tool
            .call(json!({ "prompt": "x", "model": "main" }))
            .await
            .unwrap();
        assert_eq!(m["output"], "MAIN", "main tier runs the main provider");
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn fork_inherits_conversation_spawn_does_not() {
        // "Spawn and fork subagents" (spawn clean vs fork inherits)
        let seen = Arc::new(StdMutex::new(0usize));
        let prov: Arc<dyn ModelProvider> = Arc::new(Recording { seen: seen.clone() });
        let (rt, home) = mk_runtime(ProviderTiers::new(prov, None));
        rt.storage
            .append(&rt.device_id, "conv1", &Message::user("earlier"))
            .unwrap();
        rt.storage
            .append(&rt.device_id, "conv1", &Message::assistant("reply"))
            .unwrap();
        rt.set_active_conversation("conv1").await;
        let tool = SpawnSubagent(Arc::clone(&rt));
        tool.call(json!({ "prompt": "new", "mode": "spawn" }))
            .await
            .unwrap();
        let spawn_len = *seen.lock().unwrap(); // system + user
        tool.call(json!({ "prompt": "new", "mode": "fork" }))
            .await
            .unwrap();
        let fork_len = *seen.lock().unwrap(); // system + earlier + reply + user
        assert_eq!(spawn_len, 2, "spawn starts clean");
        assert!(fork_len > spawn_len, "fork inherits the conversation");
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn unknown_task_errors_and_stop_marks_stopped() {
        // "Continue and stop subagents"
        let (rt, home) = mk_runtime(ProviderTiers::new(one_shot("x"), None));
        let status = SubagentStatus(Arc::clone(&rt));
        assert!(status.call(json!({ "task_id": "nope" })).await.is_err());
        let send = SendSubagentMessage(Arc::clone(&rt));
        assert!(send
            .call(json!({ "task_id": "nope", "prompt": "hi" }))
            .await
            .is_err());
        let stop = StopSubagent(Arc::clone(&rt));
        assert!(stop.call(json!({ "task_id": "nope" })).await.is_err());
        let spawn = SpawnSubagent(Arc::clone(&rt));
        let r = spawn.call(json!({ "prompt": "x" })).await.unwrap();
        let id = r["task_id"].as_str().unwrap().to_string();
        let s = stop.call(json!({ "task_id": id })).await.unwrap();
        assert_eq!(s["state"], "stopped");
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    #[serial]
    async fn concurrency_cap_refuses_excess_background() {
        // "Non-interactive gate and concurrency limit" (the cap half)
        std::env::set_var("FLEETY_SUBAGENT_MAX_CONCURRENT", "1");
        let gate = Arc::new(tokio::sync::Notify::new());
        let prov: Arc<dyn ModelProvider> = Arc::new(Block { gate: gate.clone() });
        let (rt, home) = mk_runtime(ProviderTiers::new(prov, None));
        std::env::remove_var("FLEETY_SUBAGENT_MAX_CONCURRENT");
        let spawn = SpawnSubagent(Arc::clone(&rt));
        let r1 = spawn
            .call(json!({ "prompt": "a", "run_in_background": true }))
            .await
            .unwrap();
        assert_eq!(r1["state"], "running");
        let r2 = spawn
            .call(json!({ "prompt": "b", "run_in_background": true }))
            .await;
        assert!(r2.is_err(), "second background spawn exceeds cap=1");
        gate.notify_waiters();
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn worktree_isolation_requires_git() {
        // "Isolation mode" (non-git workspace errors)
        let (rt, home) = mk_runtime(ProviderTiers::new(one_shot("x"), None));
        let spawn = SpawnSubagent(Arc::clone(&rt));
        let r = spawn
            .call(json!({ "prompt": "x", "isolation": "worktree" }))
            .await;
        assert!(r.is_err(), "worktree in a non-git workspace must error");
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    #[serial]
    async fn background_completion_drives_a_coordinator_wake_turn() {
        // "Background completion notification": a finished background subagent
        // proactively drives a coordinator turn that streams the synthesis to
        // the client sink — exactly once (this background task runs once).
        std::env::remove_var("FLEETY_SUBAGENT_MAX_CONCURRENT");
        let home = std::env::temp_dir().join(format!("fleety-sub-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        let auth = Arc::new(crate::auth::AuthStore::load(home.join("auth.json"), None, false));
        let (out, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // One main provider, scripted: first the subagent's own reply, then the
        // coordinator synthesis produced on the wake turn.
        let main: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse {
                message: Message::assistant("sub-result"),
            },
            ModelResponse {
                message: Message::assistant("coordinator-ack"),
            },
        ]));
        let rt = SubagentRuntime::new(
            ProviderTiers::new(main, None),
            Policy::FullAccess,
            storage,
            home.clone(),
            "dev".to_string(),
            crate::bridge::new_hub(),
            crate::bridge::new_pending(),
            crate::bridge::new_handles(),
            auth,
            crate::bridge::new_device_tools(),
            out,
        );
        rt.set_active_conversation("c1").await;
        let spawn = SpawnSubagent(Arc::clone(&rt));
        let r = spawn
            .call(json!({ "prompt": "go", "run_in_background": true }))
            .await
            .unwrap();
        assert_eq!(r["state"], "running");
        // The wake turn streams the coordinator synthesis to `out`; wait for it.
        let saw = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(frame) = rx.recv().await {
                if let tokio_tungstenite::tungstenite::Message::Text(t) = frame {
                    if t.contains("coordinator-ack") {
                        return true;
                    }
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(saw, "background completion drove a coordinator wake turn");
        let _ = std::fs::remove_dir_all(home);
    }
}
