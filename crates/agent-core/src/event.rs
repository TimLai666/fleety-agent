//! Event log for a turn — the durable record of the agent loop.
//!
//! Each loop step (assistant message, tool call, tool result) is pushed here. An
//! optional **sink** lets a caller persist every event the instant it happens
//! (e.g. to a per-conversation turn journal), so a turn interrupted by a crash
//! or redeploy can be reconstructed and resumed rather than lost. The full
//! history also powers resume and compaction.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::model::{Message, ToolCall};

/// One recorded step of the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The assistant message returned by the provider.
    Assistant(Message),
    /// A tool call the assistant requested.
    ToolCall(ToolCall),
    /// The result fed back for a tool call (success value or an error report).
    ToolResult { id: String, result: Value },
}

/// A per-event persistence callback (e.g. append to a turn journal).
pub type EventSink = Box<dyn FnMut(&Event) + Send>;

/// An ordered log of [`Event`]s, with an optional per-event persistence sink.
#[derive(Default)]
pub struct EventLog {
    events: Vec<Event>,
    /// Called with each event as it is pushed, before it is stored — used to
    /// journal the in-flight turn durably.
    sink: Option<EventSink>,
}

impl std::fmt::Debug for EventLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLog")
            .field("events", &self.events)
            .field("sink", &self.sink.is_some())
            .finish()
    }
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// An event log that persists each event to `sink` as it is pushed.
    pub fn with_sink(sink: EventSink) -> Self {
        Self {
            events: Vec::new(),
            sink: Some(sink),
        }
    }

    pub fn push(&mut self, event: Event) {
        if let Some(sink) = self.sink.as_mut() {
            sink(&event);
        }
        self.events.push(event);
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The result fed back for a tool call that was interrupted by a restart: its
/// outcome is unknown, so the model must verify state before retrying. The loop
/// never blindly re-runs a side-effecting tool on resume (safe for full access).
pub fn interrupted_tool_result() -> Value {
    json!({
        "interrupted": true,
        "reason": "this tool call was interrupted by a restart before its result was recorded; whether it ran is unknown. Check the relevant state before deciding to retry — do not assume it did or did not take effect."
    })
}

/// Rebuild the in-turn messages from a journaled (possibly interrupted) turn so
/// the loop can continue after a restart. Tool results are budgeted to
/// `max_tool_result_chars` to match the live loop. Any tool call left without a
/// result (the one in flight at the crash) gets an `interrupted` result appended
/// — it is **not** re-executed.
pub fn reconstruct_messages(events: &[Event], max_tool_result_chars: usize) -> Vec<Message> {
    let satisfied: HashSet<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::ToolResult { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();

    let mut messages = Vec::new();
    for event in events {
        match event {
            Event::Assistant(msg) => messages.push(msg.clone()),
            Event::ToolResult { id, result } => {
                let fed = crate::agent::budget_text(&result.to_string(), max_tool_result_chars);
                messages.push(Message::tool_result(id.clone(), fed));
            }
            Event::ToolCall(_) => {} // carried inside the assistant message
        }
    }
    // Tool calls with no recorded result were interrupted: tell the model so it
    // verifies, rather than re-running the tool.
    let mut handled: HashSet<String> = HashSet::new();
    for event in events {
        if let Event::Assistant(msg) = event {
            for call in &msg.tool_calls {
                if !satisfied.contains(call.id.as_str()) && handled.insert(call.id.clone()) {
                    messages.push(Message::tool_result(
                        call.id.clone(),
                        interrupted_tool_result().to_string(),
                    ));
                }
            }
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_sees_every_pushed_event() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(0usize));
        let s = Arc::clone(&seen);
        let mut log = EventLog::with_sink(Box::new(move |_e| {
            *s.lock().expect("lock") += 1;
        }));
        log.push(Event::ToolResult {
            id: "a".into(),
            result: json!(1),
        });
        log.push(Event::ToolResult {
            id: "b".into(),
            result: json!(2),
        });
        assert_eq!(*seen.lock().expect("lock"), 2);
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn reconstruct_injects_interrupted_for_dangling_call() {
        // Assistant asked for tools a and b; only a got a result before the crash.
        let mut assistant = Message::assistant("");
        assistant.tool_calls = vec![
            ToolCall {
                id: "a".into(),
                name: "read_file".into(),
                arguments: json!({}),
            },
            ToolCall {
                id: "b".into(),
                name: "run_command".into(),
                arguments: json!({}),
            },
        ];
        let events = vec![
            Event::Assistant(assistant),
            Event::ToolCall(ToolCall {
                id: "a".into(),
                name: "read_file".into(),
                arguments: json!({}),
            }),
            Event::ToolResult {
                id: "a".into(),
                result: json!({ "ok": true }),
            },
            Event::ToolCall(ToolCall {
                id: "b".into(),
                name: "run_command".into(),
                arguments: json!({}),
            }),
            // crash: no ToolResult for b
        ];
        let messages = reconstruct_messages(&events, 8000);
        // assistant + result(a) + interrupted(b)
        assert_eq!(messages.len(), 3);
        let last = messages.last().expect("last");
        let content = last.content.clone().unwrap_or_default();
        assert!(content.contains("interrupted"));
        // b is the run_command (side-effecting) call — must NOT be re-run, only flagged.
        assert_eq!(last.tool_call_id.as_deref(), Some("b"));
    }
}
