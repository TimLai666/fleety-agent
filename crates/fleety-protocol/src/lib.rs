//! fleety-protocol — wire types shared across the Fleety client runtime and server.
//!
//! Pure data: this crate carries no logic and depends only on `serde`, so it can
//! act as the contract between components (and, later, across languages).
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use serde::{Deserialize, Serialize};

/// Bumped when the wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 0;

/// Wire form of an actionable error (mirrors `agent_core::ErrorReport`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireError {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remediation: Option<String>,
}

/// Origin context the CLI attaches so the agent knows where a message came from.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct OriginContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Frames sent client -> server over the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// First frame: identify the origin device.
    Hello { device_id: String, protocol: u32 },
    /// A user turn. `conversation_id` continues an existing conversation, or
    /// `None` starts a new one.
    UserMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        text: String,
        #[serde(default)]
        origin: OriginContext,
    },
    /// Reconnect to an existing conversation; the server replays events after
    /// `after_seq`.
    Resume {
        conversation_id: String,
        after_seq: u64,
    },
    /// Approve a pending tool call (reply to `ApprovalRequested`).
    Approve { approval_id: String },
    /// Deny a pending tool call (reply to `ApprovalRequested`).
    Deny { approval_id: String },
}

/// Frames sent server -> client over the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Reply to `Hello`: the session and (initial) conversation.
    Welcome {
        session_id: String,
        conversation_id: String,
        protocol: u32,
    },
    /// An assistant message for a conversation, with its event `seq`.
    Assistant {
        conversation_id: String,
        text: String,
        seq: u64,
    },
    /// A replayed past event (sent in response to `Resume`).
    Replay {
        conversation_id: String,
        seq: u64,
        role: String,
        content: String,
    },
    /// A tool call needs the user's approval before it runs.
    ApprovalRequested {
        approval_id: String,
        tool: String,
        summary: String,
        risk: String,
    },
    /// The turn is complete.
    Done { conversation_id: String },
    /// Something went wrong (actionable).
    Error { error: WireError },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_error_roundtrips() {
        let err = WireError {
            kind: "io".into(),
            message: "nope".into(),
            remediation: Some("retry".into()),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let back: WireError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, back);
    }

    #[test]
    fn remediation_omitted_when_none() {
        let err = WireError {
            kind: "x".into(),
            message: "y".into(),
            remediation: None,
        };
        let json = serde_json::to_string(&err).expect("serialize");
        assert!(!json.contains("remediation"));
    }
}
