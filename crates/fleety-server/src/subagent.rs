//! Fleety's [`SubagentHost`] — the I/O half of subagent delegation.
//!
//! The generic mechanism (task registry, state machine, concurrency cap,
//! spawn/fork/send/stop/status, the orchestration tools, and the `run_turn`
//! loop) lives in `agent_core::subagent`. This module just plugs Fleety's I/O
//! into it: providers via [`ProviderTiers`], the child tool registry via
//! `build_full_registry`, messages/audit via [`Storage`], git worktrees for
//! isolation, and a proactive wake turn via `conn::drive_turn`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_core::{
    register_orchestration, AutoApprove, CoreError, Message, ModelProvider, Policy, Result,
    SubagentHost, SubagentManager, SubagentMode, SubagentState, ToolRegistry,
};

use crate::auth::AuthStore;
use crate::bridge::{DeviceTools, Handles, Hub, Pending};
use crate::conn::{drive_turn, Out};
use crate::providers::ProviderTiers;
use crate::storage::Storage;

/// Default background-subagent concurrency cap; override with
/// `FLEETY_SUBAGENT_MAX_CONCURRENT`.
pub const DEFAULT_MAX_CONCURRENT: usize = 4;

/// Read the configured concurrency cap (floor 1 is applied by the manager).
pub fn max_concurrent_from_env() -> usize {
    std::env::var("FLEETY_SUBAGENT_MAX_CONCURRENT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_CONCURRENT)
}

/// Fleety's host implementation. Holds the per-connection state the host
/// callbacks need (providers, store, routing deps, the client sink, the turn
/// lock, and the active conversation), plus a weak back-reference to the manager
/// so a wake turn can carry the orchestration tools.
pub struct FleetyHost {
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
    active_conversation: Mutex<String>,
    /// Serializes user turns and wake turns on this connection.
    turn_lock: Mutex<()>,
    manager: OnceLock<Weak<SubagentManager>>,
}

impl FleetyHost {
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
        Arc::new(Self {
            providers,
            policy,
            storage,
            workspace,
            device_id,
            hub,
            pending,
            handles,
            auth,
            device_tools,
            out,
            active_conversation: Mutex::new(String::new()),
            turn_lock: Mutex::new(()),
            manager: OnceLock::new(),
        })
    }

    /// Wire the manager back-reference (set once, right after construction).
    pub fn set_manager(&self, manager: Weak<SubagentManager>) {
        let _ = self.manager.set(manager);
    }

    /// Record which conversation the parent is driving (for `fork` + wakes).
    pub async fn set_active_conversation(&self, conversation: &str) {
        *self.active_conversation.lock().await = conversation.to_string();
    }

    /// Hold the per-connection turn lock across a user turn so a background
    /// wake turn cannot interleave storage appends.
    pub async fn lock_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.turn_lock.lock().await
    }

    /// Build the full Fleety tool registry rooted at `workspace` (or the default
    /// workspace). This is the child registry — it has NO orchestration tools.
    fn registry_at(&self, workspace: Option<&str>) -> ToolRegistry {
        let overridden = workspace.map(PathBuf::from);
        let root = overridden.as_deref().unwrap_or(&self.workspace);
        crate::conn::build_full_registry(
            &self.storage,
            root,
            &self.device_id,
            &self.hub,
            &self.pending,
            &self.handles,
            &self.auth,
            &self.device_tools,
        )
    }
}

#[async_trait]
impl SubagentHost for FleetyHost {
    fn resolve_provider(&self, tier: &str) -> Arc<dyn ModelProvider> {
        self.providers.resolve(tier)
    }

    async fn capture_context(&self) -> String {
        self.active_conversation.lock().await.clone()
    }

    async fn child_registry(&self, workspace: Option<&str>) -> ToolRegistry {
        self.registry_at(workspace)
    }

    async fn initial_messages(
        &self,
        mode: SubagentMode,
        context: &str,
        prompt: &str,
    ) -> Vec<Message> {
        let system = self
            .storage
            .system_prompt_for(&self.storage.acting_for_device(&self.device_id))
            .unwrap_or_default();
        match mode {
            SubagentMode::Spawn => vec![Message::system(system), Message::user(prompt)],
            SubagentMode::Fork => {
                let mut msgs = vec![Message::system(system)];
                msgs.extend(
                    self.storage
                        .load(&self.device_id, context)
                        .unwrap_or_default(),
                );
                msgs.push(Message::user(prompt));
                msgs
            }
        }
    }

    async fn prepare_workspace(&self, isolation: &str, task_id: &str) -> Result<Option<String>> {
        match isolation {
            "worktree" => {
                let dir = create_worktree(&self.workspace, task_id)?;
                Ok(Some(dir.to_string_lossy().into_owned()))
            }
            // "none" and anything unrecognised share the parent workspace.
            _ => Ok(None),
        }
    }

    async fn cleanup_workspace(&self, workspace: Option<&str>) -> bool {
        match workspace {
            Some(ws) => cleanup_worktree(&self.workspace, Path::new(ws)),
            None => true,
        }
    }

    async fn record_events(&self, events: &[agent_core::Event]) {
        for ev in events {
            let _ = self.storage.append_history(&self.device_id, ev);
        }
    }

    async fn on_complete(
        &self,
        task_id: String,
        context: String,
        state: SubagentState,
        output: String,
    ) {
        let Some(manager) = self.manager.get().and_then(Weak::upgrade) else {
            return;
        };
        // Serialize against user turns, then drive a coordinator turn seeded with
        // the result — mirroring Claude Code's auto re-invocation.
        let _guard = self.turn_lock.lock().await;
        *self.active_conversation.lock().await = context.clone();
        let summary = truncate(&output, 4000);
        let seed = format!(
            "[subagent {task_id} {}] The background subagent you spawned has finished. \
             Its result:\n\n{summary}\n\nSynthesize this for the user or take any follow-up action.",
            state.as_str()
        );
        // A real parent turn: full tools PLUS orchestration, so it may spawn again.
        let mut tools = self.registry_at(None);
        register_orchestration(&mut tools, manager);
        let provider = self.providers.main();
        let mut gate = AutoApprove;
        if let Err(e) = drive_turn(
            &self.out,
            &self.storage,
            provider.as_ref(),
            &tools,
            self.policy,
            &self.device_id,
            &context,
            Message::user(seed),
            &mut gate,
            // A wake turn is single-shot: emit its reply normally.
            true,
            // Background wake turns are non-voice.
            false,
            &self.storage.acting_for_device(&self.device_id),
        )
        .await
        {
            tracing::warn!(task_id, error = %format!("{e}"), "subagent wake turn failed");
        }
    }
}

/// Create a detached git worktree for an isolated subagent run.
fn create_worktree(base: &Path, task_id: &str) -> Result<PathBuf> {
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
        .map_err(|e| CoreError::Message(format!("git worktree add failed to spawn: {e}")))?;
    if !out.status.success() {
        return Err(CoreError::Message(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(dir)
}

/// Remove a subagent's worktree only when it is clean, so its work is never
/// silently discarded; returns whether it was actually removed.
fn cleanup_worktree(base: &Path, dir: &Path) -> bool {
    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("status")
        .arg("--porcelain")
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(true);
    if dirty {
        return false;
    }
    let removed = std::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .arg("worktree")
        .arg("remove")
        .arg(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(base)
        .arg("worktree")
        .arg("prune")
        .output();
    removed
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

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MockProvider, ModelResponse, SpawnRequest};

    fn mk_host() -> (
        Arc<FleetyHost>,
        PathBuf,
        tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
        ProviderTiers,
    ) {
        let home = std::env::temp_dir().join(format!("fleety-host-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        let auth = Arc::new(AuthStore::load(home.join("auth.json"), None, false));
        let (out, rx) = tokio::sync::mpsc::unbounded_channel();
        let main: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![
            ModelResponse {
                message: Message::assistant("sub-result"),
            },
            ModelResponse {
                message: Message::assistant("coordinator-ack"),
            },
        ]));
        let tiers = ProviderTiers::new(main, None);
        let host = FleetyHost::new(
            tiers.clone(),
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
        (host, home, rx, tiers)
    }

    #[tokio::test]
    async fn child_registry_has_device_exec_not_orchestration() {
        // Fleety wiring of the one-level cap: real tools minus orchestration.
        let (host, home, _rx, _t) = mk_host();
        let child = host.child_registry(None).await;
        assert!(child.contains("device_exec"));
        assert!(!child.contains("spawn_subagent"));
        // Leaf cap also excludes the workflow tool, so a subagent can't nest one.
        assert!(!child.contains("run_workflow"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn background_completion_drives_a_coordinator_wake_turn() {
        // The Fleety-specific wake: a finished background subagent drives a
        // coordinator turn that streams the synthesis to the client sink.
        let (host, home, mut rx, _t) = mk_host();
        let manager = SubagentManager::new(
            Arc::clone(&host) as Arc<dyn SubagentHost>,
            Policy::FullAccess,
            4,
        );
        host.set_manager(Arc::downgrade(&manager));
        host.set_active_conversation("c1").await;
        let r = manager
            .spawn(SpawnRequest {
                mode: SubagentMode::Spawn,
                tier: "main".to_string(),
                prompt: "go".to_string(),
                allowed_tools: Vec::new(),
                isolation: "none".to_string(),
                name: None,
                background: true,
            })
            .await
            .unwrap();
        assert_eq!(r["state"], "running");
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
