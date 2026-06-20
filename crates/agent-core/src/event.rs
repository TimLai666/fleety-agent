//! In-memory event log for a turn.
//!
//! M0/M1 keeps events in memory; later milestones persist them as the
//! `conversations/{id}.jsonl` event stream that powers resume and compaction.

use serde_json::Value;

use crate::model::{Message, ToolCall};

/// One recorded step of the agent loop.
#[derive(Debug, Clone)]
pub enum Event {
    /// The assistant message returned by the provider.
    Assistant(Message),
    /// A tool call the assistant requested.
    ToolCall(ToolCall),
    /// The result fed back for a tool call (success value or an error report).
    ToolResult { id: String, result: Value },
}

/// An ordered log of [`Event`]s.
#[derive(Debug, Default)]
pub struct EventLog {
    events: Vec<Event>,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: Event) {
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
