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
    /// First frame: identify the origin device. `token` authenticates an
    /// enrolled device; `pairing_code` enrolls a new one (the server mints and
    /// returns a token in `Welcome`). Both optional — auth is enforced only when
    /// the server runs with `FLEETY_REQUIRE_AUTH`.
    Hello {
        device_id: String,
        protocol: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pairing_code: Option<String>,
    },
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
    /// Result of an on-device tool the server dispatched (reply to `RunTool`).
    /// `result_json` is the JSON-encoded tool result.
    ToolResult {
        call_id: String,
        result_json: String,
    },
    /// Failure of an on-device tool the server dispatched (reply to `RunTool`).
    ToolError { call_id: String, error: WireError },
}

/// Frames sent server -> client over the WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Reply to `Hello`: the session and (initial) conversation. `token` is set
    /// only right after a successful pairing — the client should save it and
    /// authenticate with it on future connects.
    Welcome {
        session_id: String,
        conversation_id: String,
        protocol: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
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
    /// Ask the connected device's daemon to run a tool locally (on-device
    /// execution). `args_json` is the JSON-encoded arguments object.
    RunTool {
        call_id: String,
        tool: String,
        args_json: String,
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
    fn on_device_tool_frames_roundtrip() {
        let run = ServerMsg::RunTool {
            call_id: "c1".into(),
            tool: "run_command".into(),
            args_json: r#"{"command":"echo hi"}"#.into(),
        };
        let json = serde_json::to_string(&run).expect("serialize");
        assert_eq!(run, serde_json::from_str(&json).expect("deserialize"));

        let result = ClientMsg::ToolResult {
            call_id: "c1".into(),
            result_json: r#"{"ok":true}"#.into(),
        };
        let json = serde_json::to_string(&result).expect("serialize");
        assert_eq!(result, serde_json::from_str(&json).expect("deserialize"));
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
