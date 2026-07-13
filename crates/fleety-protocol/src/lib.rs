//! fleety-protocol — wire types shared across the Fleety client runtime and server.
//!
//! Pure data: this crate carries no logic and depends only on `serde`, so it can
//! act as the contract between components (and, later, across languages).
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use serde::{Deserialize, Serialize};

/// Bumped when the wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

/// The structured-config protocol version the server advertises in `Welcome`
/// (`0` = only the legacy `ConfigExec`). A client compares this to decide
/// which remote-config surfaces it can use. `1` adds `ConfigSnapshot`/
/// `ConfigApply`; `2` adds the credential frames (`CredentialPut`/`Status`/
/// `Delete`); `3` makes those credential frames per-provider (they carry a
/// `provider`, and a `codex-oauth` frame without one is rejected); `4` adds
/// provider-specific model discovery. Additive — an older server omits it and
/// the client sees `0`.
pub const CONFIG_PROTOCOL_VERSION: u32 = 4;

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
    /// The originating device's home directory, so the server can locate the
    /// origin's user-global instruction files (`~/.claude`, `~/.agents`) even
    /// when the origin is another device than the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

/// Which host a remote `ConfigExec` operates on. `Local` is handled by the CLI
/// without a connection (so it never travels the wire); `Device` is reserved for
/// a follow-up change (the server rejects it as unsupported for now).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigTarget {
    Server,
    Local,
    Device(String),
}

/// When a config change takes effect (reported back so the user isn't misled
/// into thinking a change applied immediately).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Re-read on the next client connection (provider pool / model selection).
    NextConnection,
    /// Needs a server restart (flat settings seeded into env at boot).
    Restart,
}

/// A single setting in a structured config snapshot (design §8): enough for a
/// client to render and edit it without parsing rendered text. A secret's
/// `value` is empty and only `is_set` is meaningful.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigEntry {
    pub key: String,
    pub scope: String,
    #[serde(default)]
    pub value: String,
    pub default: String,
    pub description: String,
    pub secret: bool,
    pub is_set: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// How one config change applies (secret tri-state, design §8): `Keep` leaves a
/// setting untouched (so a masked secret is never rewritten), `Set` writes the
/// change's `value`, `Clear` reverts it to env/default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOp {
    Keep,
    Set,
    Clear,
}

/// One change in a `ConfigApply` — sparse, only touched keys are sent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigChange {
    pub key: String,
    pub op: ChangeOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
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
        /// The machine's hostname — a display label (the identity is `device_id`).
        /// Additive + optional: old clients send `None`. Used for the device
        /// record and the one-time hostname→machine-id migration lookup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hostname: Option<String>,
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
    /// Cancel the connection's in-flight turn without submitting a new message
    /// (no triage). The conversation id is informational — one connection has a
    /// single in-flight turn today; carrying it keeps the wire shape stable if
    /// conversations ever run concurrently.
    CancelTurn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conversation_id: Option<String>,
    },
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
    /// List the connecting device's recent conversations so a user can find the
    /// id `Resume` needs. Scoped by the server to the acting user resolved for
    /// the device (its owner, else the device's own unattributed conversations);
    /// `limit` caps how many are returned (server-clamped). Reply:
    /// `ConversationListResult`. Additive + optional: an older server ignores it.
    ConversationList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
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
    /// Manage configuration over the connection. `args` is the full config
    /// argument vector (e.g. `["set","FLEETY_MODEL","gpt-5"]`, `["list"]`,
    /// `["provider","add",…]`), fed to the same parser the local CLI uses.
    /// `target` selects the host: `Server` runs on the connected server;
    /// `Device` is a follow-up (rejected for now). Reply: `ConfigResult`.
    ConfigExec {
        target: ConfigTarget,
        args: Vec<String>,
    },
    /// A periodic co-location report from a device that has presence tracking
    /// enabled, used to infer which site the device is currently at. `fingerprint`
    /// is a hash of the current LAN's stable attributes (default-gateway MAC +
    /// subnet); `subnet` is the CIDR; `peers` are mDNS-discovered Fleety device
    /// ids on the same segment. Additive and optional — an absent `fingerprint`
    /// means it could not be determined. There is no reply frame; the server
    /// updates the device's site and presence timeline silently.
    Colocation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        peers: Vec<String>,
    },
    /// Pull the target host's settings as a structured snapshot (reply:
    /// `ConfigSnapshotResult`) — the structured counterpart to `ConfigExec list`.
    ConfigSnapshot { target: ConfigTarget },
    /// Apply a sparse set of config changes under optimistic locking:
    /// `base_revision` is the revision of the snapshot the edit started from; the
    /// server rejects the apply as a conflict if it no longer matches (no lost
    /// update). `providers_json`, when present, additionally writes back the full
    /// structured provider config (the same shape `ConfigSnapshotResult` returns)
    /// under the same revision lock — part of the config protocol 2 capability
    /// set; older servers ignore unknown fields, so clients must gate on the
    /// advertised version before sending it. Reply: `ConfigResult`.
    ConfigApply {
        target: ConfigTarget,
        base_revision: String,
        changes: Vec<ConfigChange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        providers_json: Option<String>,
    },
    /// Store a credential in the connected server's credential store. `kind`
    /// discriminates the credential (first kind: `codex-oauth`, whose
    /// `payload_json` is the serde shape of the OAuth `Tokens`). Requires an
    /// authenticated connection; accepted writes are audited (never with token
    /// values). Reply: `CredentialResult`. Advertised by `config_protocol >= 2`;
    /// the per-provider `provider` key requires `config_protocol >= 3`.
    /// `provider` names the provider the credential belongs to (required for
    /// `codex-oauth`; a `None` from an older client is rejected server-side).
    CredentialPut {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        payload_json: String,
    },
    /// Query whether the server holds a credential of `kind` for `provider`. Reply:
    /// `CredentialStatusResult` — presence and expiry only, never token values.
    CredentialStatus {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    /// Ask the server to discover model IDs for a configured provider. The
    /// server owns any API key or OAuth credential used by the request. Reply:
    /// `ProviderModelListResult`; advertised by `config_protocol >= 4`.
    ProviderModelList { provider: String },
    /// Remove the server-side credential of `kind` for `provider`. Requires an
    /// authenticated connection; audited. Reply: `CredentialResult`.
    CredentialDelete {
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    /// Ask the connected server to mint a short-lived pairing code (for enrolling
    /// another device). Only reaches the server past Hello auth / loopback trust.
    /// Reply: `PairingCode`.
    MintPairingCode,
    /// Any frame this build does not recognize. `#[serde(other)]` routes an
    /// unknown `type` here instead of failing to parse — so a newer peer's
    /// additive frame is answered with an `unsupported` error rather than
    /// dropping the connection (design §8, M4).
    #[serde(other)]
    Unknown,
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
        /// The server's own version (`agent_core::VERSION`). A device's daemon
        /// converges to it (forward-only) so a fleet tracks the server it
        /// connects to. Additive; `""` when an older server omits it.
        #[serde(default)]
        server_version: String,
        /// Whether the active model accepts audio input. The voice client uses
        /// this to decide whether to send compressed audio or transcribe locally.
        /// Additive; `false` when an older server omits it (→ local STT).
        #[serde(default)]
        audio_input: bool,
        /// The structured-config protocol the server supports (see
        /// [`CONFIG_PROTOCOL_VERSION`]). Additive; `0` when an older server omits
        /// it, so the client falls back to the legacy `ConfigExec`.
        #[serde(default)]
        config_protocol: u32,
        /// The server's persistent identity fingerprint (also advertised over
        /// mDNS): clients pin it at pairing and use it to re-find this exact
        /// server after an address change. Additive; absent on older servers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_fingerprint: Option<String>,
        /// True when the server accepted this connection on same-host loopback
        /// trust (no token needed). The CLI uses it to skip pairing prompts for
        /// a local server. Additive; `false` on older servers.
        #[serde(default)]
        loopback_trusted: bool,
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
    /// Reply to `ConversationList`: a JSON-encoded array of conversation
    /// summaries. Each carries `conversation_id`, `last_ts_secs` (unix seconds
    /// of last activity), `events` (event count), and `preview` (a one-line clip
    /// of the first user message). Most-recent-first; empty array when the acting
    /// user has none.
    ConversationListResult { conversations_json: String },
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
    /// Reply to `ConfigExec`: `output` is the command's rendered text (list
    /// table, confirmation, …). `effect` says when a successful change takes
    /// effect; `error` carries an actionable message on failure (errors-as-
    /// messages — the server never crashes on a bad config request).
    ConfigResult {
        ok: bool,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effect: Option<Effect>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
    /// Reply to `ConfigSnapshot`: the target's settings as structured entries
    /// plus the structured provider/model config, tagged with a `revision` for
    /// optimistic-locked `ConfigApply`.
    ConfigSnapshotResult {
        revision: String,
        entries: Vec<ConfigEntry>,
        /// JSON-encoded structured provider/model config (providers.toml shape).
        providers_json: String,
    },
    /// Reply to `CredentialPut` / `CredentialDelete`.
    CredentialResult {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
    /// Reply to `CredentialStatus`: presence and expiry only — token values
    /// never cross this frame. `detail` is a non-secret label (e.g. an account
    /// hint); `error` is set when the query itself was refused.
    CredentialStatusResult {
        present: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at_secs: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
    /// Reply to `ProviderModelList`: model IDs only, never credential values.
    ProviderModelListResult {
        provider: String,
        model_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
    /// Reply to `MintPairingCode`: `code` is the minted short-lived pairing code
    /// on success; `error` explains why not (e.g. authentication is disabled).
    PairingCode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<WireError>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_hostname_is_optional_and_additive() {
        // With a hostname → round-trips.
        let h = ClientMsg::Hello {
            device_id: "machine-1".into(),
            protocol: PROTOCOL_VERSION,
            token: None,
            pairing_code: None,
            local_tools_json: None,
            hostname: Some("laptop".into()),
        };
        let json = serde_json::to_string(&h).expect("ser");
        assert!(json.contains("\"hostname\":\"laptop\""));
        match serde_json::from_str::<ClientMsg>(&json).expect("de") {
            ClientMsg::Hello { hostname, .. } => assert_eq!(hostname.as_deref(), Some("laptop")),
            _ => panic!("not hello"),
        }
        // An old client's frame (no hostname field) still parses → None.
        let old = r#"{"type":"hello","device_id":"d","protocol":0}"#;
        match serde_json::from_str::<ClientMsg>(old).expect("de old") {
            ClientMsg::Hello {
                hostname,
                device_id,
                ..
            } => {
                assert_eq!(device_id, "d");
                assert!(hostname.is_none());
            }
            _ => panic!("not hello"),
        }
    }

    #[test]
    fn welcome_server_version_is_additive() {
        // New server emits it.
        let w = ServerMsg::Welcome {
            session_id: "s".into(),
            conversation_id: "c".into(),
            protocol: PROTOCOL_VERSION,
            server_version: "0.3.0".into(),
            audio_input: true,
            config_protocol: CONFIG_PROTOCOL_VERSION,
            server_fingerprint: Some("srv-fp-1".into()),
            loopback_trusted: false,
            token: None,
        };
        let json = serde_json::to_string(&w).expect("ser");
        assert!(json.contains("\"server_version\":\"0.3.0\""));
        assert!(json.contains("\"audio_input\":true"));
        assert!(json.contains("\"config_protocol\":4"));
        assert!(json.contains("\"server_fingerprint\":\"srv-fp-1\""));
        // An old server's frame (no server_version / audio_input / config_protocol)
        // still parses → defaults ("" / false / 0 → legacy ConfigExec + local STT).
        let old = r#"{"type":"welcome","session_id":"s","conversation_id":"c","protocol":0}"#;
        match serde_json::from_str::<ServerMsg>(old).expect("de old") {
            ServerMsg::Welcome {
                server_version,
                audio_input,
                config_protocol,
                ..
            } => {
                assert_eq!(server_version, "");
                assert!(!audio_input);
                assert_eq!(config_protocol, 0);
            }
            _ => panic!("not welcome"),
        }
    }

    #[test]
    fn structured_config_frames_roundtrip_and_tolerate_unknown() {
        // ConfigSnapshot / ConfigApply / ConfigSnapshotResult round-trip.
        let snap = ClientMsg::ConfigSnapshot {
            target: ConfigTarget::Server,
        };
        let back: ClientMsg =
            serde_json::from_str(&serde_json::to_string(&snap).expect("ser")).expect("de");
        assert_eq!(back, snap);

        let apply = ClientMsg::ConfigApply {
            target: ConfigTarget::Server,
            base_revision: "rev-1".into(),
            changes: vec![
                ConfigChange {
                    key: "FLEETY_POLICY".into(),
                    op: ChangeOp::Set,
                    value: Some("require_approval".into()),
                },
                ConfigChange {
                    key: "FLEETY_TOKEN".into(),
                    op: ChangeOp::Keep,
                    value: None,
                },
            ],
            providers_json: None,
        };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(&serde_json::to_string(&apply).expect("ser"))
                .expect("de"),
            apply
        );
        // Key-only applies omit providers_json entirely on the wire, and an old
        // client's frame (predating the field) parses to None.
        match &apply {
            ClientMsg::ConfigApply { providers_json, .. } => assert!(providers_json.is_none()),
            _ => unreachable!(),
        }
        let apply_json = serde_json::to_string(&apply).expect("ser");
        assert!(!apply_json.contains("providers_json"));
        let old = r#"{"type":"config_apply","target":"server","base_revision":"r","changes":[]}"#;
        match serde_json::from_str::<ClientMsg>(old).expect("de old") {
            ClientMsg::ConfigApply { providers_json, .. } => assert!(providers_json.is_none()),
            _ => panic!("not a config apply"),
        }

        // A full provider write-back rides the same frame (config protocol 2).
        let apply_providers = ClientMsg::ConfigApply {
            target: ConfigTarget::Server,
            base_revision: "rev-1".into(),
            changes: vec![],
            providers_json: Some(r#"{"providers":{}}"#.into()),
        };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(
                &serde_json::to_string(&apply_providers).expect("ser")
            )
            .expect("de"),
            apply_providers
        );

        let result = ServerMsg::ConfigSnapshotResult {
            revision: "rev-1".into(),
            entries: vec![ConfigEntry {
                key: "FLEETY_TOKEN".into(),
                scope: "server".into(),
                value: String::new(), // a secret carries no value
                default: String::new(),
                description: "bootstrap admin token".into(),
                secret: true,
                is_set: true,
                effect: Some(Effect::Restart),
                choices: vec![],
            }],
            providers_json: "{}".into(),
        };
        let rt: ServerMsg =
            serde_json::from_str(&serde_json::to_string(&result).expect("ser")).expect("de");
        assert_eq!(rt, result);

        // An unknown client frame type deserializes to Unknown (does not error),
        // so the server can reply "unsupported" instead of dropping the link.
        let unknown = r#"{"type":"some_future_frame","field":1}"#;
        assert_eq!(
            serde_json::from_str::<ClientMsg>(unknown).expect("unknown parses"),
            ClientMsg::Unknown
        );
    }

    #[test]
    fn credential_frames_roundtrip_and_status_never_carries_tokens() {
        // Put / status / delete round-trip, carrying the per-provider key.
        let put = ClientMsg::CredentialPut {
            kind: "codex-oauth".into(),
            provider: Some("tingzhen-codex".into()),
            payload_json: r#"{"access_token":"a","refresh_token":"r","expires_at_secs":1}"#.into(),
        };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(&serde_json::to_string(&put).expect("ser"))
                .expect("de"),
            put
        );
        let status = ClientMsg::CredentialStatus {
            kind: "codex-oauth".into(),
            provider: Some("tingzhen-codex".into()),
        };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(&serde_json::to_string(&status).expect("ser"))
                .expect("de"),
            status
        );
        let del = ClientMsg::CredentialDelete {
            kind: "codex-oauth".into(),
            provider: Some("tingzhen-codex".into()),
        };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(&serde_json::to_string(&del).expect("ser"))
                .expect("de"),
            del
        );

        // An older client's frame omits `provider` entirely → it parses as `None`
        // (the server rejects a codex-oauth `None`, but the wire stays parseable).
        let legacy = r#"{"type":"credential_put","kind":"codex-oauth","payload_json":"{}"}"#;
        match serde_json::from_str::<ClientMsg>(legacy).expect("de legacy") {
            ClientMsg::CredentialPut { provider, .. } => assert!(provider.is_none()),
            _ => panic!("not a credential put"),
        }

        let ok = ServerMsg::CredentialResult {
            ok: true,
            error: None,
        };
        let json = serde_json::to_string(&ok).expect("ser");
        assert!(!json.contains("error"), "None error is omitted: {json}");
        assert_eq!(serde_json::from_str::<ServerMsg>(&json).expect("de"), ok);

        // The status reply carries presence + expiry only — by shape it has no
        // field a token value could travel in.
        let st = ServerMsg::CredentialStatusResult {
            present: true,
            expires_at_secs: Some(123),
            detail: Some("account abc".into()),
            error: None,
        };
        let json = serde_json::to_string(&st).expect("ser");
        assert!(json.contains("\"present\":true"));
        assert!(json.contains("\"expires_at_secs\":123"));
        assert!(!json.contains("token"));
        assert_eq!(serde_json::from_str::<ServerMsg>(&json).expect("de"), st);

        // A frame from a newer peer with extra fields still parses (additive).
        let extra = r#"{"type":"credential_status_result","present":false,"future_field":1}"#;
        match serde_json::from_str::<ServerMsg>(extra).expect("de extra") {
            ServerMsg::CredentialStatusResult {
                present,
                expires_at_secs,
                ..
            } => {
                assert!(!present);
                assert!(expires_at_secs.is_none());
            }
            _ => panic!("not a credential status result"),
        }
    }

    #[test]
    fn provider_model_frames_roundtrip_without_credentials() {
        let request = ClientMsg::ProviderModelList {
            provider: "tingzhen-codex".into(),
        };
        assert_eq!(
            serde_json::from_str::<ClientMsg>(&serde_json::to_string(&request).expect("ser"))
                .expect("de"),
            request
        );

        let result = ServerMsg::ProviderModelListResult {
            provider: "tingzhen-codex".into(),
            model_ids: vec!["gpt-5".into(), "gpt-5-mini".into()],
            error: None,
        };
        let json = serde_json::to_string(&result).expect("ser");
        assert!(json.contains("gpt-5"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
        assert_eq!(
            serde_json::from_str::<ServerMsg>(&json).expect("de"),
            result
        );
    }

    #[test]
    fn pairing_code_frames_roundtrip_and_tolerate_unknown() {
        let req = ClientMsg::MintPairingCode;
        assert_eq!(
            serde_json::from_str::<ClientMsg>(&serde_json::to_string(&req).expect("ser"))
                .expect("de"),
            req
        );
        let ok = ServerMsg::PairingCode {
            code: Some("abc123".into()),
            error: None,
        };
        let json = serde_json::to_string(&ok).expect("ser");
        assert!(json.contains("\"code\":\"abc123\""));
        assert!(!json.contains("error"));
        assert_eq!(serde_json::from_str::<ServerMsg>(&json).expect("de"), ok);
        // A future peer's extra field still parses (additive).
        let extra = r#"{"type":"pairing_code","code":"x","future":1}"#;
        match serde_json::from_str::<ServerMsg>(extra).expect("de extra") {
            ServerMsg::PairingCode { code, .. } => assert_eq!(code.as_deref(), Some("x")),
            _ => panic!("not a pairing code"),
        }
    }

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
    fn cancel_turn_roundtrips() {
        let cancel = ClientMsg::CancelTurn {
            conversation_id: Some("c1".into()),
        };
        let json = serde_json::to_string(&cancel).expect("serialize");
        assert_eq!(cancel, serde_json::from_str(&json).expect("deserialize"));
        // The conversation id is optional: omitted on the wire, defaults on read.
        let bare: ClientMsg = serde_json::from_str(r#"{"type":"cancel_turn"}"#).expect("bare");
        assert_eq!(
            bare,
            ClientMsg::CancelTurn {
                conversation_id: None
            }
        );
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
    fn config_frames_roundtrip() {
        // Request: a provider op targeting the server.
        let req = ClientMsg::ConfigExec {
            target: ConfigTarget::Server,
            args: vec!["set".into(), "FLEETY_MODEL".into(), "gpt-5".into()],
        };
        let json = serde_json::to_string(&req).expect("ser");
        assert_eq!(req, serde_json::from_str(&json).expect("de"));

        // Device target carries its id.
        let dev = ClientMsg::ConfigExec {
            target: ConfigTarget::Device("pi".into()),
            args: vec!["list".into()],
        };
        let json = serde_json::to_string(&dev).expect("ser");
        assert_eq!(dev, serde_json::from_str(&json).expect("de"));

        // Result: success with an effect, and a failure with an error.
        let ok = ServerMsg::ConfigResult {
            ok: true,
            output: "added provider 'codex-1'".into(),
            effect: Some(Effect::NextConnection),
            error: None,
        };
        let json = serde_json::to_string(&ok).expect("ser");
        assert_eq!(ok, serde_json::from_str(&json).expect("de"));

        let err = ServerMsg::ConfigResult {
            ok: false,
            output: String::new(),
            effect: None,
            error: Some(WireError {
                kind: "config".into(),
                message: "unknown setting".into(),
                remediation: None,
            }),
        };
        let json = serde_json::to_string(&err).expect("ser");
        assert_eq!(err, serde_json::from_str(&json).expect("de"));
    }

    #[test]
    fn colocation_frame_roundtrip() {
        // Full report with all fields.
        let full = ClientMsg::Colocation {
            fingerprint: Some("sha256:abcd".into()),
            subnet: Some("192.168.1.0/24".into()),
            peers: vec!["pi".into(), "desk".into()],
        };
        let json = serde_json::to_string(&full).expect("ser");
        assert_eq!(full, serde_json::from_str(&json).expect("de"));

        // Absent fingerprint (could not be determined) still round-trips, and a
        // minimal wire form (only the tag) deserializes to all-empty fields.
        let bare: ClientMsg = serde_json::from_str(r#"{"type":"colocation"}"#).expect("de bare");
        assert_eq!(
            bare,
            ClientMsg::Colocation {
                fingerprint: None,
                subnet: None,
                peers: vec![],
            }
        );

        // Adding this additive variant does not disturb existing frames.
        let resume = ClientMsg::Resume {
            conversation_id: "c1".into(),
            after_seq: 7,
        };
        let json = serde_json::to_string(&resume).expect("ser");
        assert_eq!(resume, serde_json::from_str(&json).expect("de"));
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
    fn conversation_list_frames_roundtrip() {
        // Request with an explicit limit → round-trips and carries the field.
        let with_limit = ClientMsg::ConversationList { limit: Some(10) };
        let json = serde_json::to_string(&with_limit).expect("ser");
        assert!(json.contains("\"limit\":10"));
        assert_eq!(with_limit, serde_json::from_str(&json).expect("de"));

        // Request with no limit → the field is omitted on the wire (matching an
        // older client's shape) and parses back to None.
        let no_limit = ClientMsg::ConversationList { limit: None };
        let json = serde_json::to_string(&no_limit).expect("ser");
        assert!(!json.contains("limit"), "limit omitted when None");
        assert_eq!(no_limit, serde_json::from_str(&json).expect("de"));

        // An old frame that lacks the limit field entirely still parses → None.
        let bare: ClientMsg =
            serde_json::from_str(r#"{"type":"conversation_list"}"#).expect("de bare");
        assert_eq!(bare, ClientMsg::ConversationList { limit: None });

        // The reply round-trips.
        let reply = ServerMsg::ConversationListResult {
            conversations_json:
                r#"[{"conversation_id":"c1","last_ts_secs":5,"events":3,"preview":"hi"}]"#.into(),
        };
        let json = serde_json::to_string(&reply).expect("ser");
        assert_eq!(reply, serde_json::from_str(&json).expect("de"));

        // Additive: the new variant doesn't disturb an existing frame's shape.
        assert_eq!(PROTOCOL_VERSION, 1);
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
        assert_eq!(PROTOCOL_VERSION, 1);
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
        assert_eq!(PROTOCOL_VERSION, 1);
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
