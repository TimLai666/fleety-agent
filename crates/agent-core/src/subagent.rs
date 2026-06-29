//! Generic subagent delegation — the framework half.
//!
//! A parent agent delegates a task to a **subagent**: a nested [`run_turn`] loop
//! with its own messages, a chosen model tier, and a tool registry equal to the
//! parent's MINUS the orchestration tools. Because a subagent has no
//! orchestration tools it cannot spawn further subagents — a one-level nesting
//! cap enforced by tool absence.
//!
//! Everything host-specific (which provider a tier maps to, how to build the
//! child tool registry, where the system prompt and history come from, isolated
//! workspaces, audit, and what "report back" means) is abstracted behind the
//! [`SubagentHost`] trait, so this module depends on no host crate. The
//! [`SubagentManager`] owns the generic mechanism: the task registry, lifecycle
//! states, concurrency cap, the spawn/fork/send/stop/status operations, and the
//! `run_turn` orchestration. [`register_orchestration`] adds the orchestration
//! tools that drive it (spawn / send / stop / status / list) — only ever at the
//! top level. Workers can be addressed by their `task_id` or the readable `name`
//! the lead gave them, which is the "agent team" usage pattern.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{
    run_turn, ApprovalGate, AutoApprove, AutoDeny, CoreError, Event, EventLog, LoopConfig,
    MandateGate, Message, ModelProvider, Policy, Result, RiskLevel, Tool, ToolRegistry, ToolSpec,
};

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
    /// Lowercase wire string for tool results.
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentState::Spawned => "spawned",
            SubagentState::Running => "running",
            SubagentState::Done => "done",
            SubagentState::Failed => "failed",
            SubagentState::Stopped => "stopped",
        }
    }
    /// Whether the task has finished (done/failed/stopped).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            SubagentState::Done | SubagentState::Failed | SubagentState::Stopped
        )
    }
}

/// Start fresh (`Spawn`) or inherit the parent conversation (`Fork`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentMode {
    Spawn,
    Fork,
}

/// A parsed request to spawn a subagent.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub mode: SubagentMode,
    /// Host-defined model tier (e.g. `"main"` / `"cheap"`). Resolved by the host.
    pub tier: String,
    pub prompt: String,
    pub allowed_tools: Vec<String>,
    /// Host-defined isolation token (e.g. `"none"` / `"worktree"`), opaque here.
    pub isolation: String,
    pub name: Option<String>,
    pub background: bool,
}

/// All host-specific I/O the manager needs. An embedder implements this; the
/// manager contains no host-specific code, so this module stays dependency-free.
#[async_trait]
pub trait SubagentHost: Send + Sync + 'static {
    /// Resolve a model tier name to a provider. Unknown tiers SHOULD map to a
    /// sensible default rather than error.
    fn resolve_provider(&self, tier: &str) -> Arc<dyn ModelProvider>;

    /// Snapshot an opaque context (e.g. the active conversation id) at spawn
    /// time, handed back to [`initial_messages`](Self::initial_messages) and
    /// [`on_complete`](Self::on_complete) so a background task reports to the
    /// right place even if the foreground moved on.
    async fn capture_context(&self) -> String;

    /// Build the subagent's tool registry. It MUST NOT include the orchestration
    /// tools (that is what caps nesting). `workspace` is an opaque token from
    /// [`prepare_workspace`](Self::prepare_workspace) when isolation is active.
    async fn child_registry(&self, workspace: Option<&str>) -> ToolRegistry;

    /// Build the subagent's initial messages: a fresh briefing for `Spawn`, or
    /// the inherited conversation plus the briefing for `Fork`.
    async fn initial_messages(
        &self,
        mode: SubagentMode,
        context: &str,
        prompt: &str,
    ) -> Vec<Message>;

    /// Prepare an isolated workspace for the run; return an opaque token or
    /// `None`. Errors (e.g. isolation unsupported here) surface synchronously.
    async fn prepare_workspace(&self, isolation: &str, task_id: &str) -> Result<Option<String>>;

    /// Tear down an isolated workspace; return whether it was actually removed
    /// (a host may keep one that still holds the subagent's changes).
    async fn cleanup_workspace(&self, workspace: Option<&str>) -> bool;

    /// Record the run's events (audit), tagged by the subagent's `task_id` so the
    /// host can attribute them to that run (e.g. a child conversation). Takes a
    /// slice (not the `EventLog`) so the future stays `Send`.
    async fn record_events(&self, task_id: &str, events: &[Event]);

    /// The id under which this run's record is stored (e.g. a child conversation
    /// id), surfaced in the spawn result so the caller can open it. `None` (the
    /// default) means the host keeps no separately-addressable record.
    fn record_id(&self, _task_id: &str) -> Option<String> {
        None
    }

    /// A background subagent finished: the host reports back (e.g. wakes a
    /// coordinator turn) using the captured `context`.
    async fn on_complete(
        &self,
        task_id: String,
        context: String,
        state: SubagentState,
        output: String,
    );
}

struct TaskRecord {
    name: Option<String>,
    state: SubagentState,
    messages: Vec<Message>,
    output: Option<String>,
    tier: String,
    handle: Option<tokio::task::JoinHandle<()>>,
    workspace: Option<String>,
}

/// Owns the generic subagent lifecycle for one parent: task registry, states,
/// concurrency cap, and the `run_turn` orchestration. I/O goes through the host.
pub struct SubagentManager {
    host: Arc<dyn SubagentHost>,
    policy: Policy,
    max_concurrent: usize,
    tasks: Mutex<HashMap<String, TaskRecord>>,
    running: AtomicU64,
    seq: AtomicU64,
}

impl SubagentManager {
    /// Build a manager over a host. `max_concurrent` is clamped to a floor of 1.
    pub fn new(host: Arc<dyn SubagentHost>, policy: Policy, max_concurrent: usize) -> Arc<Self> {
        Arc::new(Self {
            host,
            policy,
            max_concurrent: max_concurrent.max(1),
            tasks: Mutex::new(HashMap::new()),
            running: AtomicU64::new(0),
            seq: AtomicU64::new(1),
        })
    }

    fn next_id(&self) -> String {
        format!("sub-{}", self.seq.fetch_add(1, Ordering::SeqCst))
    }

    /// The non-interactive gate for a subagent run (generic; the tier never
    /// affects it). Full access runs everything; require-approval limits to read
    /// tools unless `allowed_tools` pre-grants specific tools.
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

    /// Run one nested agent loop and return the terminal state, output, and the
    /// (possibly extended) messages. Never panics: an error becomes `Failed`.
    async fn run_loop(
        &self,
        task_id: &str,
        mut messages: Vec<Message>,
        tier: &str,
        allowed_tools: &[String],
        workspace: Option<&str>,
    ) -> (SubagentState, String, Vec<Message>) {
        let registry = self.host.child_registry(workspace).await;
        let provider = self.host.resolve_provider(tier);
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
        self.host.record_events(task_id, events.events()).await;
        match outcome {
            Ok(o) => (SubagentState::Done, o.output, messages),
            Err(e) => (
                SubagentState::Failed,
                format!("subagent failed: {e}"),
                messages,
            ),
        }
    }

    /// Spawn (foreground or background). Foreground awaits and returns the
    /// output; background returns a `task_id` and reports via `on_complete`.
    pub async fn spawn(self: &Arc<Self>, req: SpawnRequest) -> Result<Value> {
        let context = self.host.capture_context().await;
        let task_id = self.next_id();
        if req.background && self.running.load(Ordering::SeqCst) as usize >= self.max_concurrent {
            return Err(CoreError::Message(format!(
                "too many background subagents running (cap {}); wait for one to finish or stop one",
                self.max_concurrent
            )));
        }
        // Prepare the isolated workspace up front so isolation errors (e.g. not a
        // git repo) surface synchronously for both foreground and background.
        let workspace = self
            .host
            .prepare_workspace(&req.isolation, &task_id)
            .await?;
        self.tasks.lock().await.insert(
            task_id.clone(),
            TaskRecord {
                name: req.name.clone(),
                state: SubagentState::Running,
                messages: Vec::new(),
                output: None,
                tier: req.tier.clone(),
                handle: None,
                workspace: workspace.clone(),
            },
        );
        if req.background {
            self.running.fetch_add(1, Ordering::SeqCst);
            let me = Arc::clone(self);
            let id = task_id.clone();
            let handle = tokio::spawn(async move {
                let (state, output) = me.run_to_completion(&id, &req, &context, workspace).await;
                me.running.fetch_sub(1, Ordering::SeqCst);
                if state != SubagentState::Stopped {
                    me.host.on_complete(id, context, state, output).await;
                }
            });
            if let Some(rec) = self.tasks.lock().await.get_mut(&task_id) {
                rec.handle = Some(handle);
            }
            let mut out = json!({ "task_id": task_id, "state": "running" });
            if let Some(child) = self.host.record_id(&task_id) {
                out["child_conversation_id"] = json!(child);
            }
            Ok(out)
        } else {
            let (state, output) = self
                .run_to_completion(&task_id, &req, &context, workspace)
                .await;
            let mut out = json!({ "task_id": task_id, "state": state.as_str(), "output": output });
            if let Some(child) = self.host.record_id(&task_id) {
                out["child_conversation_id"] = json!(child);
            }
            Ok(out)
        }
    }

    /// Build messages, run the loop, clean up the workspace, update the record.
    async fn run_to_completion(
        &self,
        task_id: &str,
        req: &SpawnRequest,
        context: &str,
        workspace: Option<String>,
    ) -> (SubagentState, String) {
        let messages = self
            .host
            .initial_messages(req.mode, context, &req.prompt)
            .await;
        let (state, mut output, final_messages) = self
            .run_loop(
                task_id,
                messages,
                &req.tier,
                &req.allowed_tools,
                workspace.as_deref(),
            )
            .await;
        // Remove the workspace only if the host did; if it kept one (changes
        // present), tell the caller where so the work is not lost.
        let removed = self.host.cleanup_workspace(workspace.as_deref()).await;
        let kept = workspace.filter(|_| !removed);
        if let Some(ws) = &kept {
            output.push_str(&format!("\n\n[isolated workspace kept at {ws}]"));
        }
        if let Some(rec) = self.tasks.lock().await.get_mut(task_id) {
            // A stop request may have already moved it to Stopped; don't clobber.
            if rec.state != SubagentState::Stopped {
                rec.state = state;
                rec.output = Some(output.clone());
                rec.messages = final_messages;
                rec.workspace = kept;
            }
        }
        (state, output)
    }

    /// Resolve a task id OR a worker name to the canonical task id. Lets the lead
    /// agent address team members by the readable `name` it gave them.
    async fn resolve(&self, key: &str) -> Result<String> {
        let tasks = self.tasks.lock().await;
        if tasks.contains_key(key) {
            return Ok(key.to_string());
        }
        for (id, rec) in tasks.iter() {
            if rec.name.as_deref() == Some(key) {
                return Ok(id.clone());
            }
        }
        Err(CoreError::Message(format!(
            "unknown subagent '{key}' (not a task_id or name); use subagent_list to see the team"
        )))
    }

    /// Roster of every subagent this manager owns — the lead's "team".
    pub async fn list(&self) -> Value {
        let tasks = self.tasks.lock().await;
        let mut items: Vec<Value> = tasks
            .iter()
            .map(|(id, rec)| {
                json!({
                    "task_id": id,
                    "name": rec.name,
                    "state": rec.state.as_str(),
                    "terminal": rec.state.is_terminal(),
                })
            })
            .collect();
        items.sort_by(|a, b| a["task_id"].as_str().cmp(&b["task_id"].as_str()));
        json!({ "subagents": items })
    }

    /// Continue a finished (not running) subagent — addressed by task id or name —
    /// preserving its messages.
    pub async fn send(&self, key: &str, prompt: &str) -> Result<Value> {
        let task_id = self.resolve(key).await?;
        let (mut messages, tier) = {
            let tasks = self.tasks.lock().await;
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
        messages.push(Message::user(prompt));
        if let Some(rec) = self.tasks.lock().await.get_mut(&task_id) {
            rec.state = SubagentState::Running;
        }
        let (state, output, final_messages) =
            self.run_loop(&task_id, messages, &tier, &[], None).await;
        if let Some(rec) = self.tasks.lock().await.get_mut(&task_id) {
            rec.state = state;
            rec.output = Some(output.clone());
            rec.messages = final_messages;
        }
        Ok(json!({ "task_id": task_id, "state": state.as_str(), "output": output }))
    }

    /// Stop a subagent (by task id or name): abort a background task, mark stopped.
    pub async fn stop(&self, key: &str) -> Result<Value> {
        let task_id = self.resolve(key).await?;
        let mut tasks = self.tasks.lock().await;
        let rec = tasks
            .get_mut(&task_id)
            .ok_or_else(|| CoreError::Message(format!("unknown subagent task '{task_id}'")))?;
        if let Some(handle) = rec.handle.take() {
            handle.abort();
            self.running.fetch_sub(1, Ordering::SeqCst);
        }
        rec.state = SubagentState::Stopped;
        Ok(json!({ "task_id": task_id, "state": "stopped" }))
    }

    /// Report a subagent's state (by task id or name) and, when finished, output.
    pub async fn status(&self, key: &str) -> Result<Value> {
        let task_id = self.resolve(key).await?;
        let tasks = self.tasks.lock().await;
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

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

fn opt_str<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn parse_request(args: &Value) -> Result<SpawnRequest> {
    let prompt = require_str(args, "prompt")?.to_string();
    let mode = match opt_str(args, "mode", "spawn") {
        "fork" => SubagentMode::Fork,
        "spawn" => SubagentMode::Spawn,
        other => return Err(CoreError::Message(format!("unknown mode '{other}'"))),
    };
    let tier = opt_str(args, "model", "main").to_string();
    let isolation = opt_str(args, "isolation", "none").to_string();
    let allowed_tools = args
        .get("allowed_tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let name = args.get("name").and_then(Value::as_str).map(str::to_string);
    let background = args
        .get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(SpawnRequest {
        mode,
        tier,
        prompt,
        allowed_tools,
        isolation,
        name,
        background,
    })
}

/// Register the four orchestration tools that drive a [`SubagentManager`]. Call
/// this ONLY when building a top-level registry — never for a subagent's own
/// registry, which is what caps nesting at one level.
pub fn register_orchestration(registry: &mut ToolRegistry, manager: Arc<SubagentManager>) {
    registry.register(Box::new(SpawnSubagent(Arc::clone(&manager))));
    registry.register(Box::new(SendSubagentMessage(Arc::clone(&manager))));
    registry.register(Box::new(StopSubagent(Arc::clone(&manager))));
    registry.register(Box::new(SubagentStatus(Arc::clone(&manager))));
    registry.register(Box::new(SubagentList(manager)));
}

struct SubagentList(Arc<SubagentManager>);

#[async_trait]
impl Tool for SubagentList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent_list".to_string(),
            description: "List your team — every subagent you spawned with its task_id, name, and \
                state. Use the names/ids here to address a worker with send_subagent_message, \
                stop_subagent, or subagent_status."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        Ok(self.0.list().await)
    }
}

struct SpawnSubagent(Arc<SubagentManager>);

#[async_trait]
impl Tool for SpawnSubagent {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "spawn_subagent".to_string(),
            description: "Delegate a task to a subagent (a nested agent with the same tools as you \
                MINUS the ability to spawn its own subagents). `mode`: \"spawn\" (fresh context + \
                this prompt) or \"fork\" (inherits the current conversation). `model`: a model tier \
                (e.g. \"main\" or \"cheap\"). `run_in_background`: true returns a task_id immediately \
                and reports back when done; false awaits and returns the output. `isolation`: e.g. \
                \"none\" or \"worktree\". `allowed_tools`: optional whitelist."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "The task to delegate." },
                    "mode": { "type": "string", "enum": ["spawn", "fork"], "description": "Default \"spawn\"." },
                    "model": { "type": "string", "description": "Model tier, e.g. \"main\" (default) or \"cheap\"." },
                    "run_in_background": { "type": "boolean", "description": "Default false." },
                    "isolation": { "type": "string", "description": "e.g. \"none\" (default) or \"worktree\"." },
                    "allowed_tools": { "type": "array", "items": { "type": "string" } },
                    "name": { "type": "string", "description": "Short worker name (1-2 words); you can address it later by this name in send/stop/status/list." }
                },
                "required": ["prompt"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let req = parse_request(&args)?;
        self.0.spawn(req).await
    }
}

struct SendSubagentMessage(Arc<SubagentManager>);

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
                    "task_id": { "type": "string", "description": "The worker's task_id or name." },
                    "prompt": { "type": "string" }
                },
                "required": ["task_id", "prompt"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let task_id = require_str(&args, "task_id")?.to_string();
        let prompt = require_str(&args, "prompt")?.to_string();
        self.0.send(&task_id, &prompt).await
    }
}

struct StopSubagent(Arc<SubagentManager>);

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
                "properties": { "task_id": { "type": "string", "description": "The worker's task_id or name." } },
                "required": ["task_id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let task_id = require_str(&args, "task_id")?.to_string();
        self.0.stop(&task_id).await
    }
}

struct SubagentStatus(Arc<SubagentManager>);

#[async_trait]
impl Tool for SubagentStatus {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent_status".to_string(),
            description: "Report a subagent's current state and, when finished, its output."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "task_id": { "type": "string", "description": "The worker's task_id or name." } },
                "required": ["task_id"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let task_id = require_str(&args, "task_id")?.to_string();
        self.0.status(&task_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MockProvider, ModelResponse};
    use std::sync::Mutex as StdMutex;

    fn one_shot(text: &str) -> Arc<dyn ModelProvider> {
        Arc::new(MockProvider::new(vec![ModelResponse {
            message: Message::assistant(text),
        }]))
    }

    struct Ping;
    #[async_trait]
    impl Tool for Ping {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "ping".to_string(),
                description: "test tool".to_string(),
                parameters: json!({ "type": "object" }),
                risk: RiskLevel::Read,
            }
        }
        async fn call(&self, _args: Value) -> Result<Value> {
            Ok(json!("pong"))
        }
    }

    /// Completion log: (task_id, terminal state, output).
    type Completed = Arc<StdMutex<Vec<(String, SubagentState, String)>>>;

    /// A minimal in-memory host: no storage, no git, no network.
    struct MockHost {
        main: Arc<dyn ModelProvider>,
        cheap: Arc<dyn ModelProvider>,
        completed: Completed,
    }

    #[async_trait]
    impl SubagentHost for MockHost {
        fn resolve_provider(&self, tier: &str) -> Arc<dyn ModelProvider> {
            if tier == "cheap" {
                Arc::clone(&self.cheap)
            } else {
                Arc::clone(&self.main)
            }
        }
        async fn capture_context(&self) -> String {
            "ctx".to_string()
        }
        async fn child_registry(&self, _workspace: Option<&str>) -> ToolRegistry {
            let mut r = ToolRegistry::new();
            r.register(Box::new(Ping));
            r
        }
        async fn initial_messages(
            &self,
            mode: SubagentMode,
            _context: &str,
            prompt: &str,
        ) -> Vec<Message> {
            match mode {
                SubagentMode::Spawn => vec![Message::system("s"), Message::user(prompt)],
                SubagentMode::Fork => vec![
                    Message::system("s"),
                    Message::user("history"),
                    Message::user(prompt),
                ],
            }
        }
        async fn prepare_workspace(
            &self,
            isolation: &str,
            _task_id: &str,
        ) -> Result<Option<String>> {
            if isolation == "fail" {
                Err(CoreError::Message(
                    "isolation unsupported in test".to_string(),
                ))
            } else {
                Ok(None)
            }
        }
        async fn cleanup_workspace(&self, _workspace: Option<&str>) -> bool {
            true
        }
        async fn record_events(&self, _task_id: &str, _events: &[Event]) {}
        async fn on_complete(
            &self,
            task_id: String,
            _context: String,
            state: SubagentState,
            output: String,
        ) {
            self.completed
                .lock()
                .unwrap()
                .push((task_id, state, output));
        }
    }

    fn mk(
        main: Arc<dyn ModelProvider>,
        cheap: Arc<dyn ModelProvider>,
        cap: usize,
    ) -> (Arc<SubagentManager>, Arc<MockHost>, Completed) {
        let completed = Arc::new(StdMutex::new(Vec::new()));
        let host = Arc::new(MockHost {
            main,
            cheap,
            completed: Arc::clone(&completed),
        });
        let mgr = SubagentManager::new(
            Arc::clone(&host) as Arc<dyn SubagentHost>,
            Policy::FullAccess,
            cap,
        );
        (mgr, host, completed)
    }

    fn req(
        prompt: &str,
        mode: &str,
        tier: &str,
        background: bool,
        isolation: &str,
    ) -> SpawnRequest {
        SpawnRequest {
            mode: if mode == "fork" {
                SubagentMode::Fork
            } else {
                SubagentMode::Spawn
            },
            tier: tier.to_string(),
            prompt: prompt.to_string(),
            allowed_tools: Vec::new(),
            isolation: isolation.to_string(),
            name: None,
            background,
        }
    }

    #[tokio::test]
    async fn child_registry_omits_orchestration_top_level_has_it() {
        // "One-level nesting cap by construction"
        let (mgr, host, _) = mk(one_shot("x"), one_shot("y"), 4);
        let child = host.child_registry(None).await;
        assert!(child.contains("ping"));
        for t in [
            "spawn_subagent",
            "send_subagent_message",
            "stop_subagent",
            "subagent_status",
        ] {
            assert!(!child.contains(t), "child must not have {t}");
        }
        let mut top = host.child_registry(None).await;
        register_orchestration(&mut top, mgr);
        assert!(top.contains("spawn_subagent") && top.contains("ping"));
    }

    #[tokio::test]
    async fn spawn_foreground_returns_output() {
        // "Manager owns the lifecycle" (foreground)
        let (mgr, _h, _) = mk(one_shot("sub-done"), one_shot("y"), 4);
        let r = mgr
            .spawn(req("go", "spawn", "main", false, "none"))
            .await
            .unwrap();
        assert_eq!(r["state"], "done");
        assert_eq!(r["output"], "sub-done");
    }

    #[tokio::test]
    async fn tier_routes_to_selected_provider() {
        let (mgr, _h, _) = mk(one_shot("MAIN"), one_shot("CHEAP"), 4);
        let c = mgr
            .spawn(req("x", "spawn", "cheap", false, "none"))
            .await
            .unwrap();
        assert_eq!(c["output"], "CHEAP");
        let m = mgr
            .spawn(req("x", "spawn", "main", false, "none"))
            .await
            .unwrap();
        assert_eq!(m["output"], "MAIN");
    }

    #[tokio::test]
    async fn unknown_task_errors_and_stop_marks_stopped() {
        // "Manager owns the lifecycle" (stop / status / send)
        let (mgr, _h, _) = mk(one_shot("x"), one_shot("y"), 4);
        assert!(mgr.status("nope").await.is_err());
        assert!(mgr.send("nope", "hi").await.is_err());
        assert!(mgr.stop("nope").await.is_err());
        let r = mgr
            .spawn(req("x", "spawn", "main", false, "none"))
            .await
            .unwrap();
        let id = r["task_id"].as_str().unwrap().to_string();
        let s = mgr.stop(&id).await.unwrap();
        assert_eq!(s["state"], "stopped");
    }

    #[tokio::test]
    async fn background_reports_on_complete() {
        let (mgr, _h, completed) = mk(one_shot("bg-done"), one_shot("y"), 4);
        let r = mgr
            .spawn(req("x", "spawn", "main", true, "none"))
            .await
            .unwrap();
        assert_eq!(r["state"], "running");
        let saw = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some((_, state, output)) = completed.lock().unwrap().first().cloned() {
                    return (state, output);
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("on_complete fired");
        assert_eq!(saw.0, SubagentState::Done);
        assert_eq!(saw.1, "bg-done");
        assert_eq!(completed.lock().unwrap().len(), 1, "reported exactly once");
    }

    #[tokio::test]
    async fn addresses_workers_by_name_and_lists_team() {
        // Agent-team layer: name a worker, see the roster, address it by name.
        let main: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse {
                message: Message::assistant("v1"),
            },
            ModelResponse {
                message: Message::assistant("v2"),
            },
        ]));
        let (mgr, _h, _) = mk(main, one_shot("y"), 4);
        let spawned = mgr
            .spawn(SpawnRequest {
                mode: SubagentMode::Spawn,
                tier: "main".to_string(),
                prompt: "p".to_string(),
                allowed_tools: Vec::new(),
                isolation: "none".to_string(),
                name: Some("alice".to_string()),
                background: false,
            })
            .await
            .unwrap();
        assert_eq!(spawned["output"], "v1");
        let roster = mgr.list().await;
        let arr = roster["subagents"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "alice");
        // Address by name, not task_id.
        let cont = mgr.send("alice", "again").await.unwrap();
        assert_eq!(cont["output"], "v2");
        assert!(mgr.status("nobody").await.is_err());
    }

    #[tokio::test]
    async fn concurrency_cap_refuses_excess_background() {
        // "Manager owns the lifecycle" (concurrency cap)
        let gate = Arc::new(tokio::sync::Notify::new());

        struct Block {
            gate: Arc<tokio::sync::Notify>,
        }
        #[async_trait]
        impl ModelProvider for Block {
            async fn complete(
                &self,
                _m: &[Message],
                _t: &[ToolSpec],
            ) -> Result<crate::model::ModelResponse> {
                self.gate.notified().await;
                Ok(crate::model::ModelResponse {
                    message: Message::assistant("released"),
                })
            }
        }

        let block: Arc<dyn ModelProvider> = Arc::new(Block {
            gate: Arc::clone(&gate),
        });
        let (mgr, _h, _) = mk(block, one_shot("y"), 1);
        let r1 = mgr
            .spawn(req("a", "spawn", "main", true, "none"))
            .await
            .unwrap();
        assert_eq!(r1["state"], "running");
        let r2 = mgr.spawn(req("b", "spawn", "main", true, "none")).await;
        assert!(r2.is_err(), "second background exceeds cap=1");
        gate.notify_waiters();
    }
}
