//! `set_effort`: the main agent adjusts its own reasoning effort.
//!
//! The agent can dial its effort up for hard reasoning and down for simple work;
//! the choice is held per connection and applied to each subsequent turn's model
//! call (the connection loop reads it and selects an effort-variant provider).
//! Subagent effort is chosen by the spawning agent at spawn time, not here.

use std::sync::Arc;

use agent_core::model::Effort;
use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// The agent's current self-selected effort for this connection (None → use the
/// model's configured default). Shared between the `set_effort` tool (writes)
/// and the connection loop (reads, per turn).
pub type SessionEffort = Arc<Mutex<Option<Effort>>>;

/// A fresh session-effort slot (no override; the model's default applies).
pub fn new_session_effort() -> SessionEffort {
    Arc::new(Mutex::new(None))
}

/// Register the `set_effort` tool against this connection's effort slot.
pub fn register(tools: &mut ToolRegistry, effort: SessionEffort) {
    tools.register(Box::new(SetEffort { effort }));
}

struct SetEffort {
    effort: SessionEffort,
}

#[async_trait]
impl Tool for SetEffort {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_effort".to_string(),
            description: "Set YOUR OWN reasoning effort for your subsequent turns: 'low', \
                'medium', or 'high'. Raise it for hard reasoning, lower it for simple or \
                mechanical work; it applies until you change it again. (To set a subagent's \
                effort, pass it when you spawn the subagent — this tool only changes your own.)"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "level": {
                        "type": "string",
                        "enum": ["low", "medium", "high"],
                        "description": "Reasoning effort for your next turns."
                    }
                },
                "required": ["level"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let level = args
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let e = Effort::parse(level).ok_or_else(|| {
            CoreError::Message(format!(
                "unknown effort '{level}'; use low, medium, or high"
            ))
        })?;
        *self.effort.lock().await = Some(e);
        Ok(json!({ "ok": true, "effort": e.as_str() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_effort_updates_session_and_rejects_bad() {
        let slot = new_session_effort();
        let tool = SetEffort {
            effort: Arc::clone(&slot),
        };
        let r = tool.call(json!({ "level": "high" })).await.unwrap();
        assert_eq!(r["effort"], "high");
        assert_eq!(*slot.lock().await, Some(Effort::High));
        // Unknown level is rejected, leaving the prior value.
        assert!(tool.call(json!({ "level": "ludicrous" })).await.is_err());
        assert_eq!(*slot.lock().await, Some(Effort::High));
    }
}
