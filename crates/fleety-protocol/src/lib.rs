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

/// A media attachment riding along with a user message: an image / audio /
/// video / file the multimodal model is supposed to see directly. We don't
/// pre-process attachments through a vision tool — they're handed to the
/// model alongside the user's text in one multimodal request.
///
/// Set exactly one of `bytes_b64` (raw bytes, base64-encoded) or `url`. The
/// `mime` field routes the attachment into the right slot on the provider side
/// (`image/*` → image part, `audio/*` → audio part, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireAttachment {
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A device-deixis attention hint carried on a voice-mode terminal reply: which
/// device to look at, what to look at there, and an optional url/path the
/// terminal can surface or open.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AttentionHint {
    pub device: String,
    pub look_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
    /// returns a token in `Welcome`). `local_tools_json` is the JSON-encoded
    /// list of `ToolSpec`s the on-device runtime can execute — fleetyd sends
    /// this so the agent (and `device_show`) knows what `device_exec` can
    /// invoke on each device. All three are optional — auth is enforced only
    /// when the server runs with `FLEETY_REQUIRE_AUTH`, and an interactive CLI
    /// has no local tools to advertise.
    Hello {
        device_id: String,
        protocol: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pairing_code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_tools_json: Option<String>,
    },
    /// A user turn. `conversation_id` continues an existing conversation, or
    /// `None` starts a new one. `attachments` carries multimodal media handed
    /// straight to the model (images, audio, etc.) — see [`WireAttachment`].
    UserMessage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
        text: String,
        #[serde(default)]
        origin: OriginContext,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<WireAttachment>,
        /// Whether this message was spoken (voice mode); when on, the terminal
        /// turn may carry a spoken reply. Defaults to false.
        #[serde(default)]
        voice: bool,
        /// An explicitly asserted acting user (shared devices today; a comms
        /// sender later). Additive and optional: absent → the server resolves
        /// the acting user from the device owner, else Guest. Identifies only;
        /// it does not by itself authorize access to another user's data.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        acting_user: Option<String>,
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
    /// List a device's audit log entries newer than `since` (unix seconds, or
    /// `None` for everything), capped at `limit`.
    AuditList {
        device_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Fetch one audit entry by its monotonic line number (0-indexed).
    AuditShow { device_id: String, index: u64 },
    /// List backups available to roll back, by device.
    RollbackList { device_id: String },
    /// Restore a file from a backup (records its own audit + a fresh backup of
    /// the pre-rollback state so the rollback itself is reversible).
    RollbackApply {
        device_id: String,
        backup_id: String,
    },
    /// Ask the server for its health snapshot (version, uptime, connected
    /// device count, etc.). Used by `fleety status`.
    ServerStatus,
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
    /// A streamed slice of the in-progress assistant reply (token-by-token
    /// display). The full reply still arrives as `Assistant` when the turn ends.
    AssistantDelta {
        conversation_id: String,
        chunk: String,
    },
    /// An assistant message for a conversation, with its event `seq`.
    Assistant {
        conversation_id: String,
        text: String,
        seq: u64,
        /// Spoken-version text for voice mode; present only on the terminal turn
        /// when voice is on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speech: Option<String>,
        /// Device-deixis attention hint (which device + what to look at) for
        /// voice mode; present only on the terminal turn when the agent points
        /// the user at something. See [`AttentionHint`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attention: Option<AttentionHint>,
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
    /// The active conversation rolled over: `old` was set aside (still searchable
    /// via recall) and `new` is now active. Front-ends should switch to `new`;
    /// those that ignore this still work (the server transparently redirects
    /// messages on `old` to its successor). Additive; older clients ignore it.
    ConversationRolled { old: String, new: String },
    /// Something went wrong (actionable).
    Error { error: WireError },
    /// Reply to `AuditList`: a JSON-encoded array of compact audit summaries.
    /// Each summary carries `index`, `kind` (e.g. "tool_call"/"tool_result"),
    /// `tool`, and `ts_secs` so the CLI can render a table without parsing the
    /// full event payload.
    AuditListResult {
        device_id: String,
        entries_json: String,
    },
    /// Reply to `AuditShow`: the full event JSON at `index`.
    AuditShowResult {
        device_id: String,
        index: u64,
        event_json: String,
    },
    /// Reply to `RollbackList`: JSON-encoded array of backup descriptors
    /// (`id`, `original_path`, `ts_secs`, `source_tool`).
    RollbackListResult {
        device_id: String,
        backups_json: String,
    },
    /// Reply to `RollbackApply`: `ok` is true if the backup was restored.
    RollbackResult {
        device_id: String,
        backup_id: String,
        ok: bool,
        message: String,
    },
    /// Reply to `ServerStatus`: a compact health snapshot. `extra_json` is
    /// reserved for future fields so adding one doesn't break the wire.
    ServerStatusResult {
        version: String,
        uptime_secs: u64,
        connected_devices: u32,
        device_ids_json: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra_json: Option<String>,
    },
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
    fn assistant_delta_roundtrips() {
        let msg = ServerMsg::AssistantDelta {
            conversation_id: "c1".into(),
            chunk: "hello".into(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert_eq!(msg, serde_json::from_str(&json).expect("deserialize"));
    }

    #[test]
    fn server_status_frames_roundtrip() {
        let req = ClientMsg::ServerStatus;
        let json = serde_json::to_string(&req).expect("ser");
        assert_eq!(req, serde_json::from_str(&json).expect("de"));

        let reply = ServerMsg::ServerStatusResult {
            version: "0.1.0".into(),
            uptime_secs: 1234,
            connected_devices: 2,
            device_ids_json: "[\"a\",\"b\"]".into(),
            extra_json: None,
        };
        let json = serde_json::to_string(&reply).expect("ser");
        assert_eq!(reply, serde_json::from_str(&json).expect("de"));
    }

    #[test]
    fn audit_and_rollback_frames_roundtrip() {
        let list = ClientMsg::AuditList {
            device_id: "dev".into(),
            since: Some(1700000000),
            limit: Some(50),
        };
        let json = serde_json::to_string(&list).expect("ser");
        assert_eq!(list, serde_json::from_str(&json).expect("de"));

        let res = ServerMsg::AuditListResult {
            device_id: "dev".into(),
            entries_json: "[]".into(),
        };
        let json = serde_json::to_string(&res).expect("ser");
        assert_eq!(res, serde_json::from_str(&json).expect("de"));

        let apply = ClientMsg::RollbackApply {
            device_id: "dev".into(),
            backup_id: "abc".into(),
        };
        let json = serde_json::to_string(&apply).expect("ser");
        assert_eq!(apply, serde_json::from_str(&json).expect("de"));

        let rb = ServerMsg::RollbackResult {
            device_id: "dev".into(),
            backup_id: "abc".into(),
            ok: true,
            message: "restored".into(),
        };
        let json = serde_json::to_string(&rb).expect("ser");
        assert_eq!(rb, serde_json::from_str(&json).expect("de"));
    }

    #[test]
    fn user_message_voice_roundtrips() {
        let msg = ClientMsg::UserMessage {
            conversation_id: None,
            text: "hi".into(),
            origin: Default::default(),
            attachments: vec![],
            voice: true,
            acting_user: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert_eq!(msg, serde_json::from_str(&json).expect("deserialize"));

        let no_voice = ClientMsg::UserMessage {
            conversation_id: None,
            text: "hi".into(),
            origin: Default::default(),
            attachments: vec![],
            voice: false,
            acting_user: None,
        };
        let json = serde_json::to_string(&no_voice).expect("serialize");
        assert_eq!(no_voice, serde_json::from_str(&json).expect("deserialize"));
    }

    #[test]
    fn conversation_rolled_roundtrips() {
        let msg = ServerMsg::ConversationRolled {
            old: "c-old".into(),
            new: "c-new".into(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert_eq!(msg, serde_json::from_str(&json).expect("deserialize"));
        // Additive: an older stream without this variant still parses other
        // ServerMsg variants, and the version is unchanged.
        assert_eq!(PROTOCOL_VERSION, 0);
    }

    #[test]
    fn user_message_acting_user_roundtrips_and_is_backward_compatible() {
        // With an asserted acting user → round-trips.
        let asserted = ClientMsg::UserMessage {
            conversation_id: None,
            text: "hi".into(),
            origin: Default::default(),
            attachments: vec![],
            voice: false,
            acting_user: Some("bob".into()),
        };
        let json = serde_json::to_string(&asserted).expect("serialize");
        assert!(json.contains("acting_user"));
        assert_eq!(asserted, serde_json::from_str(&json).expect("deserialize"));

        // When absent, the field is omitted from the wire (skip_serializing_if),
        // so the on-wire shape matches an older client; it parses back to None.
        let absent = ClientMsg::UserMessage {
            conversation_id: None,
            text: "hi".into(),
            origin: Default::default(),
            attachments: vec![],
            voice: false,
            acting_user: None,
        };
        let json = serde_json::to_string(&absent).expect("serialize");
        assert!(!json.contains("acting_user"), "omitted when None");
        match serde_json::from_str::<ClientMsg>(&json).expect("parses") {
            ClientMsg::UserMessage { acting_user, .. } => assert_eq!(acting_user, None),
            _ => panic!("expected UserMessage"),
        }
        // Protocol version unchanged (additive field).
        assert_eq!(PROTOCOL_VERSION, 0);
    }

    #[test]
    fn assistant_speech_roundtrips() {
        let msg = ServerMsg::Assistant {
            conversation_id: "c1".into(),
            text: "t".into(),
            seq: 1,
            speech: Some("spoken".into()),
            attention: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert_eq!(msg, serde_json::from_str(&json).expect("deserialize"));

        let no_speech = ServerMsg::Assistant {
            conversation_id: "c1".into(),
            text: "t".into(),
            seq: 1,
            speech: None,
            attention: None,
        };
        let json = serde_json::to_string(&no_speech).expect("serialize");
        assert_eq!(no_speech, serde_json::from_str(&json).expect("deserialize"));
    }

    #[test]
    fn user_message_without_voice_field_defaults_false() {
        let json = r#"{"type":"user_message","text":"hi"}"#;
        let msg: ClientMsg = serde_json::from_str(json).expect("deserialize");
        match msg {
            ClientMsg::UserMessage { voice, .. } => assert!(!voice),
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn assistant_attention_roundtrips() {
        let msg = ServerMsg::Assistant {
            conversation_id: "c1".into(),
            text: "look there".into(),
            seq: 2,
            speech: None,
            attention: Some(AttentionHint {
                device: "lab-pi-a".into(),
                look_at: "the dashboard".into(),
                url: Some("http://pi-a/grafana".into()),
            }),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert_eq!(msg, serde_json::from_str(&json).expect("deserialize"));
    }

    #[test]
    fn assistant_without_attention_field_is_none() {
        // Backward compatibility: an old frame without `attention` deserializes.
        let json = r#"{"type":"assistant","conversation_id":"c1","text":"t","seq":1}"#;
        let msg: ServerMsg = serde_json::from_str(json).expect("deserialize");
        match msg {
            ServerMsg::Assistant { attention, .. } => assert_eq!(attention, None),
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn assistant_without_speech_field_is_none() {
        let json = r#"{"type":"assistant","conversation_id":"c1","text":"t","seq":1}"#;
        let msg: ServerMsg = serde_json::from_str(json).expect("deserialize");
        match msg {
            ServerMsg::Assistant { speech, .. } => assert_eq!(speech, None),
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn assistant_omits_speech_when_none() {
        let msg = ServerMsg::Assistant {
            conversation_id: "c1".into(),
            text: "t".into(),
            seq: 1,
            speech: None,
            attention: None,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(!json.contains("speech"));
        assert!(!json.contains("attention"));
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
