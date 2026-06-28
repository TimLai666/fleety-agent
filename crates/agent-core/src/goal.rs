//! Generic goal mechanism — the framework half.
//!
//! An agent records its own **goal** (derived from the user's request and
//! context) plus an optional self-managed checklist, then a host-side
//! drive-to-goal loop keeps re-engaging it until the agent signals a terminal
//! state. Two terminal signals exist: `complete_goal` (the goal is achieved) and
//! `ask_user` (a question only the user can answer). A turn that ends with an
//! active goal and no terminal signal is a *premature* stop — the host loop
//! injects a continuation nudge and runs another turn.
//!
//! This module is host-free: it owns only the [`GoalState`] and the five tools
//! that mutate it ([`register_goal_tools`]), exactly mirroring `subagent.rs`. The
//! loop, the emission/voice gating, and the safety cap belong to the embedding
//! host (Fleety wires them in `conn.rs`). The tools are registered ONLY on a
//! top-level registry, so a subagent's registry omits them — a subagent cannot
//! alter its parent's goal, the same one-level cap the orchestration tools use.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// One checklist step the agent set for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    pub done: bool,
}

/// How the current goal ended, if it has. `None` means the goal is still being
/// pursued (or no goal is set).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Terminal {
    /// No terminal signal yet.
    #[default]
    None,
    /// The agent declared the goal achieved, with an optional summary.
    Complete { summary: Option<String> },
    /// The agent must ask the user a question before it can continue.
    AskUser { question: String },
}

/// The shared goal state for one user message: the goal, its checklist, and the
/// terminal signal. The five tools mutate it; the host loop reads it after each
/// turn to decide whether to stop or continue.
#[derive(Debug, Default)]
pub struct GoalState {
    goal: Option<String>,
    steps: Vec<Step>,
    terminal: Terminal,
}

impl GoalState {
    /// A fresh, empty state (no goal, no steps, no terminal).
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a goal is set. The loop treats an active goal with no terminal
    /// signal as a premature stop and continues; with no active goal the turn
    /// ends normally (single-shot).
    pub fn is_active(&self) -> bool {
        self.goal.is_some()
    }

    /// The current terminal signal without consuming it.
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// Take the terminal signal, leaving `Terminal::None` behind.
    pub fn take_terminal(&mut self) -> Terminal {
        std::mem::take(&mut self.terminal)
    }

    /// The text of every not-yet-done checklist step.
    pub fn pending_steps(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|s| !s.done)
            .map(|s| s.text.as_str())
            .collect()
    }

    /// The continuation nudge injected as the next turn's user message when a
    /// goal is active but unmet: the goal, the pending steps, and the rule for
    /// when to stop.
    pub fn nudge_text(&self) -> String {
        let goal = self.goal.as_deref().unwrap_or("(no goal set)");
        let pending = self.pending_steps();
        let steps = if pending.is_empty() {
            "No checklist steps remain.".to_string()
        } else {
            let mut s = String::from("Remaining steps:");
            for step in pending {
                s.push_str("\n- ");
                s.push_str(step);
            }
            s
        };
        format!(
            "Keep working toward your goal — it is not complete yet.\nGoal: {goal}\n{steps}\n\n\
             Continue until the goal is fully done, then call complete_goal. Only call ask_user \
             if you genuinely cannot proceed without the user answering a question.",
        )
    }

    /// JSON snapshot of the goal and its checklist (the `goal_status` payload).
    fn status_json(&self) -> Value {
        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|s| json!({ "step": s.text, "done": s.done }))
            .collect();
        json!({
            "goal": self.goal,
            "active": self.is_active(),
            "steps": steps,
        })
    }
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CoreError::Message(format!("missing required non-empty string '{key}'")))
}

/// Register the five goal tools that drive a [`GoalState`]. Call this ONLY when
/// building a top-level registry — never for a subagent's own registry, which is
/// what keeps a subagent from altering its parent's goal (one-level cap, same as
/// [`crate::register_orchestration`]).
pub fn register_goal_tools(registry: &mut ToolRegistry, state: Arc<Mutex<GoalState>>) {
    registry.register(Box::new(SetGoal(Arc::clone(&state))));
    registry.register(Box::new(CompleteStep(Arc::clone(&state))));
    registry.register(Box::new(GoalStatus(Arc::clone(&state))));
    registry.register(Box::new(CompleteGoal(Arc::clone(&state))));
    registry.register(Box::new(AskUser(state)));
}

struct SetGoal(Arc<Mutex<GoalState>>);

#[async_trait]
impl Tool for SetGoal {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_goal".to_string(),
            description: "Record the goal you are pursuing (derived from the user's request and \
                context) plus an optional checklist of steps. The runtime keeps re-engaging you \
                until you call complete_goal or ask_user, so set a goal whenever a request needs \
                more than a single reply to finish. Call again to revise the goal or plan."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string", "description": "The goal to pursue to completion." },
                    "steps": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional checklist of steps toward the goal."
                    }
                },
                "required": ["goal"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let goal = require_str(&args, "goal")?.to_string();
        let steps: Vec<Step> = args
            .get("steps")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| Step {
                        text: s.to_string(),
                        done: false,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut state = self.0.lock().await;
        state.goal = Some(goal);
        state.steps = steps;
        // Revising the plan clears any stale terminal signal.
        state.terminal = Terminal::None;
        Ok(state.status_json())
    }
}

struct CompleteStep(Arc<Mutex<GoalState>>);

#[async_trait]
impl Tool for CompleteStep {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "complete_step".to_string(),
            description: "Mark one checklist step done (matched by its text). Returns the updated \
                checklist."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "step": { "type": "string", "description": "The text of the step to mark done." }
                },
                "required": ["step"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let step = require_str(&args, "step")?;
        let mut state = self.0.lock().await;
        match state.steps.iter_mut().find(|s| s.text == step) {
            Some(s) => {
                s.done = true;
                Ok(state.status_json())
            }
            None => {
                let existing: Vec<&str> = state.steps.iter().map(|s| s.text.as_str()).collect();
                Err(CoreError::Message(format!(
                    "no step matching '{step}'; current steps: {}",
                    if existing.is_empty() {
                        "(none)".to_string()
                    } else {
                        existing.join(", ")
                    }
                )))
            }
        }
    }
}

struct GoalStatus(Arc<Mutex<GoalState>>);

#[async_trait]
impl Tool for GoalStatus {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "goal_status".to_string(),
            description: "Report the current goal and which checklist steps are done or pending."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        Ok(self.0.lock().await.status_json())
    }
}

struct CompleteGoal(Arc<Mutex<GoalState>>);

#[async_trait]
impl Tool for CompleteGoal {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "complete_goal".to_string(),
            description: "Declare the goal achieved. This is a terminal signal: the runtime stops \
                re-engaging you and your reply (and, when voice is on, the spoken summary) goes to \
                the user. Call this only when the whole goal is genuinely done."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "Optional short summary of what was accomplished." }
                }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let summary = args
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut state = self.0.lock().await;
        state.terminal = Terminal::Complete {
            summary: summary.clone(),
        };
        Ok(json!({ "completed": true, "summary": summary }))
    }
}

struct AskUser(Arc<Mutex<GoalState>>);

#[async_trait]
impl Tool for AskUser {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ask_user".to_string(),
            description: "Ask the user a question you genuinely cannot proceed without. This is a \
                terminal signal: the runtime stops and your question (and, when voice is on, the \
                spoken question) goes to the user. Use it sparingly — only when you truly cannot \
                continue toward the goal on your own."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question the user must answer." }
                },
                "required": ["question"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let question = require_str(&args, "question")?.to_string();
        let mut state = self.0.lock().await;
        state.terminal = Terminal::AskUser {
            question: question.clone(),
        };
        Ok(json!({ "asked": true, "question": question }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh state behind a registry holding the five tools.
    fn setup() -> (Arc<Mutex<GoalState>>, ToolRegistry) {
        let state = Arc::new(Mutex::new(GoalState::new()));
        let mut registry = ToolRegistry::new();
        register_goal_tools(&mut registry, Arc::clone(&state));
        (state, registry)
    }

    #[tokio::test]
    async fn register_goal_tools_registers_the_five() {
        let (_state, registry) = setup();
        for name in [
            "set_goal",
            "complete_step",
            "goal_status",
            "complete_goal",
            "ask_user",
        ] {
            assert!(registry.contains(name), "missing tool {name}");
        }
    }

    #[tokio::test]
    async fn setting_a_goal_activates_the_mechanism() {
        // Spec scenario: setting a goal activates the mechanism.
        let (state, registry) = setup();
        let out = registry
            .call("set_goal", json!({ "goal": "ship the feature" }))
            .await
            .unwrap();
        assert_eq!(out["active"], true);
        let s = state.lock().await;
        assert!(s.is_active());
        assert_eq!(*s.terminal(), Terminal::None);
    }

    #[tokio::test]
    async fn empty_goal_is_rejected() {
        let (state, registry) = setup();
        assert!(registry
            .call("set_goal", json!({ "goal": "" }))
            .await
            .is_err());
        assert!(registry
            .call("set_goal", json!({ "goal": "   " }))
            .await
            .is_err());
        // A rejected set_goal leaves the mechanism inactive.
        assert!(!state.lock().await.is_active());
    }

    #[tokio::test]
    async fn complete_goal_sets_complete_terminal() {
        let (state, registry) = setup();
        registry
            .call("set_goal", json!({ "goal": "do it" }))
            .await
            .unwrap();
        registry
            .call("complete_goal", json!({ "summary": "done it" }))
            .await
            .unwrap();
        assert_eq!(
            *state.lock().await.terminal(),
            Terminal::Complete {
                summary: Some("done it".to_string())
            }
        );
    }

    #[tokio::test]
    async fn ask_user_sets_ask_terminal() {
        let (state, registry) = setup();
        registry
            .call("set_goal", json!({ "goal": "do it" }))
            .await
            .unwrap();
        registry
            .call("ask_user", json!({ "question": "which one?" }))
            .await
            .unwrap();
        assert_eq!(
            *state.lock().await.terminal(),
            Terminal::AskUser {
                question: "which one?".to_string()
            }
        );
        // ask_user with no question is rejected.
        assert!(registry.call("ask_user", json!({})).await.is_err());
    }

    #[tokio::test]
    async fn complete_step_ticks_and_shrinks_pending() {
        let (state, registry) = setup();
        registry
            .call("set_goal", json!({ "goal": "g", "steps": ["a", "b", "c"] }))
            .await
            .unwrap();
        assert_eq!(state.lock().await.pending_steps(), vec!["a", "b", "c"]);
        registry
            .call("complete_step", json!({ "step": "b" }))
            .await
            .unwrap();
        assert_eq!(state.lock().await.pending_steps(), vec!["a", "c"]);
    }

    #[tokio::test]
    async fn unknown_step_errors() {
        let (_state, registry) = setup();
        registry
            .call("set_goal", json!({ "goal": "g", "steps": ["a"] }))
            .await
            .unwrap();
        assert!(registry
            .call("complete_step", json!({ "step": "nope" }))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn take_terminal_consumes_and_resets() {
        let (state, registry) = setup();
        registry
            .call("set_goal", json!({ "goal": "g" }))
            .await
            .unwrap();
        registry.call("complete_goal", json!({})).await.unwrap();
        let mut s = state.lock().await;
        assert!(matches!(s.take_terminal(), Terminal::Complete { .. }));
        // Consumed: a second take is None.
        assert_eq!(s.take_terminal(), Terminal::None);
    }

    #[tokio::test]
    async fn revising_goal_clears_terminal() {
        let (state, registry) = setup();
        registry
            .call("set_goal", json!({ "goal": "g1" }))
            .await
            .unwrap();
        registry.call("complete_goal", json!({})).await.unwrap();
        // Revise the plan: terminal must reset so the loop keeps driving.
        registry
            .call("set_goal", json!({ "goal": "g2" }))
            .await
            .unwrap();
        assert_eq!(*state.lock().await.terminal(), Terminal::None);
    }

    #[tokio::test]
    async fn nudge_names_goal_and_pending_steps() {
        let (state, registry) = setup();
        registry
            .call(
                "set_goal",
                json!({ "goal": "build X", "steps": ["one", "two"] }),
            )
            .await
            .unwrap();
        registry
            .call("complete_step", json!({ "step": "one" }))
            .await
            .unwrap();
        let nudge = state.lock().await.nudge_text();
        assert!(nudge.contains("build X"));
        assert!(nudge.contains("two"));
        assert!(!nudge.contains("- one"), "done step should not be nudged");
    }
}
