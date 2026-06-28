//! Conversation lifecycle: agent-driven rollover.
//!
//! `rollover_conversation` lets the agent decide a task/topic is done and start a
//! fresh conversation. The tool only *records the intent* (and mints the new id);
//! the connection applies it after the turn — marking the old conversation ended
//! with the new as its successor (the old stays searchable via recall), emitting
//! `ConversationRolled`, and transparently redirecting later messages. Triggers
//! are agent-judged (this explicit tool, or a silent post-turn nudge); never a
//! hard length cut.

use std::sync::Arc;

use agent_core::{Result, RiskLevel, Tool, ToolSpec};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// A pending rollover requested during a turn, applied by the connection after.
#[derive(Debug, Clone)]
pub struct RolloverRequest {
    pub new_id: String,
    pub distill: bool,
    pub note: Option<String>,
}

/// Shared slot the rollover tool writes and the connection drains post-turn.
pub type RolloverState = Arc<Mutex<Option<RolloverRequest>>>;

pub fn new_state() -> RolloverState {
    Arc::new(Mutex::new(None))
}

/// Register the `rollover_conversation` tool against a connection's rollover slot.
pub fn register(tools: &mut agent_core::ToolRegistry, state: RolloverState) {
    tools.register(Box::new(RolloverConversation { state }));
}

struct RolloverConversation {
    state: RolloverState,
}

#[async_trait]
impl Tool for RolloverConversation {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rollover_conversation".to_string(),
            description:
                "Start a fresh conversation when the current task or topic is done. The current \
                 conversation is preserved (still searchable via conversation_search) and a new \
                 one becomes active. Distil anything durable into the right memory layer FIRST \
                 (see the distillation rules); do this quietly — never tell the user the system \
                 asked you to. Returns the new conversation id."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "distill": { "type": "boolean", "description": "Whether you have distilled durable takeaways (informational)." },
                    "note": { "type": "string", "description": "Optional short reason, for the audit log." }
                }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let new_id = uuid::Uuid::new_v4().to_string();
        let req = RolloverRequest {
            new_id: new_id.clone(),
            distill: args
                .get("distill")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            note: args.get("note").and_then(Value::as_str).map(str::to_string),
        };
        *self.state.lock().await = Some(req);
        Ok(json!({ "new_conversation_id": new_id, "status": "rollover scheduled after this turn" }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rollover_tool_records_request_and_returns_new_id() {
        let state = new_state();
        let mut reg = agent_core::ToolRegistry::new();
        register(&mut reg, Arc::clone(&state));
        let out = reg
            .call(
                "rollover_conversation",
                json!({ "note": "task done", "distill": true }),
            )
            .await
            .unwrap();
        let new_id = out["new_conversation_id"].as_str().unwrap().to_string();
        assert!(!new_id.is_empty());
        let req = state.lock().await.clone().expect("request recorded");
        assert_eq!(req.new_id, new_id);
        assert_eq!(req.note.as_deref(), Some("task done"));
        assert!(req.distill);
    }
}
