//! `set_effort` + difficulty-based auto-effort: how the main agent's reasoning
//! effort is chosen per turn.
//!
//! The agent can dial its effort up for hard reasoning and down for simple work.
//! The choice is held per connection in a `SessionEffort` slot; the connection
//! loop re-reads it before EVERY turn it drives — including each goal-continuation
//! turn of the current request — and selects an effort-variant provider, so a
//! mid-request change takes effect on the next continuation rather than being
//! deferred to the next user message. When the agent hasn't pinned a level and
//! `FLEETY_AUTO_EFFORT` is on, `assess_effort` classifies the message's difficulty
//! (economy tier) to pick a per-message baseline; a manual pin outranks it. The
//! auto pick is never written to the slot, so it never persists on its own.
//! Subagent effort is chosen by the spawning agent at spawn time, not here.

use std::sync::Arc;

use agent_core::model::Effort;
use agent_core::{
    CoreError, Message, ModelProvider, Result, RiskLevel, Tool, ToolRegistry, ToolSpec,
};
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

/// Whether difficulty-based auto-effort is enabled (`FLEETY_AUTO_EFFORT`,
/// default on). Only an explicit `off` / `0` / `false` disables it, so an unset
/// var keeps the on-by-default behavior the config registry advertises.
pub fn auto_effort_enabled() -> bool {
    match std::env::var("FLEETY_AUTO_EFFORT") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false"
        ),
        Err(_) => true,
    }
}

/// Map a difficulty classifier's free-text answer to an effort. Pure and
/// whole-word: `low` / `medium` / `high` win in that upward-biased order when
/// several appear; anything else (including a stray `below`) is `None`, so the
/// caller keeps the default rather than guessing.
pub fn parse_effort(model_text: &str) -> Option<Effort> {
    let t = model_text.to_ascii_lowercase();
    let has = |w: &str| {
        t.split(|c: char| !c.is_ascii_alphabetic())
            .any(|tok| tok == w)
    };
    if has("high") {
        Some(Effort::High)
    } else if has("medium") {
        Some(Effort::Medium)
    } else if has("low") {
        Some(Effort::Low)
    } else {
        None
    }
}

/// Classify a top-level user message's difficulty into a reasoning effort with
/// one lightweight model call. Mirrors `crate::triage`: an empty message, a
/// failed call, or an unparseable answer yields `None` (keep the default) — it
/// never errors and never blocks the turn.
pub async fn assess_effort(new_msg: &str, provider: &dyn ModelProvider) -> Option<Effort> {
    let msg = new_msg.trim();
    if msg.is_empty() {
        return None;
    }
    let prompt = format!(
        "Classify how much reasoning effort this request needs, weighing its \
         complexity, ambiguity, and how much multi-step reasoning it demands.\n\n\
         Request:\n\"{msg}\"\n\nAnswer with exactly one word:\n\
         - low    : simple, mechanical, or conversational\n\
         - medium : ordinary multi-step work\n\
         - high   : hard reasoning, tricky debugging, architecture, or deep analysis\n\
         One word only."
    );
    match provider.complete(&[Message::user(prompt)], &[]).await {
        Ok(resp) => parse_effort(resp.message.content.as_deref().unwrap_or("")),
        Err(_) => None,
    }
}

struct SetEffort {
    effort: SessionEffort,
}

#[async_trait]
impl Tool for SetEffort {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_effort".to_string(),
            description: "Set YOUR OWN reasoning effort: 'low', 'medium', 'high', or 'auto'. This \
                does NOT change the step you are on now — it applies from your NEXT turn onward, \
                INCLUDING the next goal-continuation turn of the current request, and persists \
                until you change it again. Raise it for hard reasoning, lower it for simple or \
                mechanical work. 'auto' clears your manual choice and hands control back to the \
                runtime's automatic, difficulty-based selection. (To set a subagent's effort, \
                pass it when you spawn the subagent — this tool only changes your own.)"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "level": {
                        "type": "string",
                        "enum": ["low", "medium", "high", "auto"],
                        "description": "Reasoning effort from your next turn onward; 'auto' returns to automatic selection."
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
        // `auto` clears the manual pin, handing control back to the runtime's
        // automatic difficulty-based selection (or the configured default).
        if level.trim().eq_ignore_ascii_case("auto") {
            *self.effort.lock().await = None;
            return Ok(json!({ "ok": true, "effort": "auto" }));
        }
        let e = Effort::parse(level).ok_or_else(|| {
            CoreError::Message(format!(
                "unknown effort '{level}'; use low, medium, high, or auto"
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
        // `auto` clears the manual pin (hands control back to auto-selection).
        let r = tool.call(json!({ "level": "auto" })).await.unwrap();
        assert_eq!(r["effort"], "auto");
        assert_eq!(*slot.lock().await, None);
    }

    #[test]
    fn parse_effort_table() {
        assert_eq!(parse_effort("high"), Some(Effort::High));
        assert_eq!(parse_effort("HIGH"), Some(Effort::High));
        assert_eq!(parse_effort("medium"), Some(Effort::Medium));
        assert_eq!(parse_effort("low"), Some(Effort::Low));
        assert_eq!(parse_effort("low effort please"), Some(Effort::Low));
        // Upward bias when several appear.
        assert_eq!(parse_effort("not low, this is high"), Some(Effort::High));
        // Whole-word: a stray substring must not match.
        assert_eq!(parse_effort("below the default"), None);
        // Unparseable / empty → None (keep the default).
        assert_eq!(parse_effort(""), None);
        assert_eq!(parse_effort("¯\\_(ツ)_/¯"), None);
    }
}
