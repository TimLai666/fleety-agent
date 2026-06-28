//! Conversation persistence: one JSONL file of [`Message`]s per conversation,
//! under the Agent home, separate from any workspace (spec: workspace = dirty
//! work, durable state lives in the Fleety store).

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::{CoreError, Event, EventLog, Message, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line in `history.jsonl`. `ts_secs` rides alongside the event so audit
/// listings can show "5m ago" without a separate timestamp store. `flatten`
/// keeps the on-disk shape the same as before (just adds a `ts_secs` field),
/// so old lines still parse — they just deserialize with `ts_secs == 0`.
#[derive(Serialize, Deserialize)]
struct AuditRecord {
    #[serde(default)]
    ts_secs: u64,
    #[serde(flatten)]
    event: Event,
}

const DEFAULT_ME: &str = "Your name is Fleety. You are a cross-device, full-access agent that helps the user operate their devices. You act autonomously, keep an audit trail, and can roll back; you confirm only genuinely irreversible actions.\n\nYou are deeply curious about the world. Whenever something is worth keeping — an elegant architecture, a sharp working principle, an interesting idea or viewpoint, a thought that sparks mid-task, a logical thread worth chasing (and more — this list isn't exhaustive) — you distil it into your knowledge wiki with the wiki_* tools. Follow the wiki's rules (one concept per note, frontmatter, [[wikilinks]], dedup, classify), never a messy logbook; and you keep tending and reorganising old notes — merging, sharpening, correcting — instead of writing once and forgetting. At the first sign of an anomaly, an unexpected surprise, or a knowledge point / logic / corner worth digging into, you investigate, trace it to its source, and record what you find. You never pretend you didn't notice.";
const DEFAULT_USER: &str = "(Unknown so far. Record what you learn about the user here.)";
const DEFAULT_TODO: &str = "(No current to-dos.)";
/// The USER block for a Guest (unidentified) turn — neutral, no personal data,
/// and never another user's profile.
const GUEST_PROFILE: &str =
    "(No identified user for this turn — a guest. Do not record personal data here.)";

// The static behavioural prompt, embedded at build time. Reconciled to the
// actual tool surface (docs/tools.md); see `system_prompt`.
const PROTOCOL_MD: &str = include_str!("../../../prompts/protocol.md");
const RULES_MD: &str = include_str!("../../../prompts/rules.md");
const MEMORY_MD: &str = include_str!("../../../prompts/memory.md");
const POLICY_MD: &str = include_str!("../../../prompts/policy.md");

/// Categorise one serialized event into `(kind, tool)` for the audit summary.
/// Events are internally tagged on `event` (snake_case variant name). A
/// `tool_result` whose `result.denied == true` is surfaced as `tool_denied`
/// instead — that's how `agent_core::run_turn` records an approval denial
/// (it feeds the model a denial result rather than running the tool). The
/// audit listing wants to call that out clearly, not bury it under a generic
/// "tool_result" row.
fn summarise_event(value: &Value) -> (String, Option<String>) {
    let raw_kind = value
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("other");
    if raw_kind == "tool_result" {
        if let Some(result) = value.get("result") {
            if result
                .get("denied")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let tool = result.get("tool").and_then(Value::as_str).map(String::from);
                return ("tool_denied".to_string(), tool);
            }
        }
    }
    let kind = raw_kind.to_string();
    let tool = match kind.as_str() {
        "tool_call" => value.get("name").and_then(Value::as_str).map(String::from),
        "tool_result" => value.get("id").and_then(Value::as_str).map(String::from),
        _ => None,
    };
    (kind, tool)
}

/// A persisted conversation event with its monotonic sequence number.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub seq: u64,
    pub message: Message,
}

/// Read all stored events from a conversation file (empty if it does not exist).
fn read_events(path: &Path) -> Result<Vec<StoredEvent>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(CoreError::Message(format!(
                "cannot read {}: {e}",
                path.display()
            )))
        }
    };
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| CoreError::Message(format!("read line failed: {e}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)
            .map_err(|e| CoreError::Message(format!("corrupt conversation line: {e}")))?;
        let seq = value
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| CoreError::Message("conversation line missing 'seq'".to_string()))?;
        let message: Message =
            serde_json::from_value(value.get("message").cloned().unwrap_or(Value::Null))
                .map_err(|e| CoreError::Message(format!("corrupt conversation message: {e}")))?;
        events.push(StoredEvent { seq, message });
    }
    Ok(events)
}

/// Reject id components that could escape the store via path traversal.
fn validate_id(kind: &str, id: &str) -> Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains('\0')
        || id.contains(':')
    {
        return Err(CoreError::Message(format!(
            "invalid {kind} '{id}': must not be empty or contain path separators, ':' or '..'"
        )));
    }
    Ok(())
}

/// Filesystem-backed conversation store rooted at the Agent home.
pub struct Storage {
    home: PathBuf,
    /// Serializes the read-count-then-write critical section in `append` so
    /// concurrent appends can't assign the same `seq` (TOCTOU).
    append_lock: std::sync::Mutex<()>,
}

impl Storage {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            append_lock: std::sync::Mutex::new(()),
        }
    }

    fn conversation_path(&self, device_id: &str, conversation_id: &str) -> PathBuf {
        // User-primary when the conversation has a registered owner; otherwise
        // the legacy device path (unattributed / pre-identity conversations).
        if let Some(owner) = self.conversation_owner(conversation_id) {
            return self
                .home
                .join("fleet")
                .join("users")
                .join(owner)
                .join("conversations")
                .join(format!("{conversation_id}.jsonl"));
        }
        self.home
            .join("fleet")
            .join("devices")
            .join(device_id)
            .join("conversations")
            .join(format!("{conversation_id}.jsonl"))
    }

    fn conversations_index_path(&self) -> PathBuf {
        self.home.join("fleet").join("conversations.json")
    }

    /// The cross-user grants store (`fleet/grants.json`).
    pub fn grants(&self) -> crate::privacy::Grants {
        crate::privacy::Grants::load(&self.home.join("fleet").join("grants.json"))
    }

    /// The acting user's configured IANA timezone (`users/<id>/timezone`), if any.
    /// `None` for a Guest or when unset (caller falls back to `FLEETY_TZ`/UTC).
    pub fn user_timezone(&self, acting: &crate::identity::ActingUser) -> Option<String> {
        let id = acting.user_id()?;
        let text = fs::read_to_string(self.users_dir().join(id).join("timezone")).ok()?;
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    /// Set a user's IANA timezone. Config API consumed by the settings surface.
    #[allow(dead_code)]
    pub fn set_user_timezone(&self, user_id: &str, tz: &str) -> Result<()> {
        validate_id("user_id", user_id)?;
        let dir = self.users_dir().join(user_id);
        fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Message(format!("cannot create user dir: {e}")))?;
        fs::write(dir.join("timezone"), tz)
            .map_err(|e| CoreError::Message(format!("write timezone: {e}")))?;
        Ok(())
    }

    /// The owning user of a conversation (from the conversation→owner index), or
    /// `None` if it has no registered owner (legacy / unattributed / guest).
    pub fn conversation_owner(&self, conversation_id: &str) -> Option<String> {
        let text = fs::read_to_string(self.conversations_index_path()).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        v.get(conversation_id)
            .and_then(Value::as_str)
            .map(String::from)
    }

    /// Record the owner of a conversation (idempotent — set once, on first use).
    /// A Guest leaves it unowned (unattributed); only a real user becomes owner.
    pub fn register_conversation_owner(
        &self,
        conversation_id: &str,
        acting: &crate::identity::ActingUser,
    ) -> Result<()> {
        let Some(owner) = acting.user_id() else {
            return Ok(());
        };
        validate_id("conversation_id", conversation_id)?;
        validate_id("user_id", owner)?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| CoreError::Message("storage index lock poisoned".to_string()))?;
        let path = self.conversations_index_path();
        let mut map: serde_json::Map<String, Value> = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if !map.contains_key(conversation_id) {
            map.insert(
                conversation_id.to_string(),
                Value::String(owner.to_string()),
            );
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Message(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            let text = serde_json::to_string_pretty(&Value::Object(map))
                .map_err(|e| CoreError::Message(format!("serialize index: {e}")))?;
            fs::write(&path, text)
                .map_err(|e| CoreError::Message(format!("write conversation index: {e}")))?;
        }
        Ok(())
    }

    /// The privacy decision for an acting user reaching a conversation: an
    /// unowned conversation is open (it will be claimed on first append); an
    /// owned one is gated by the [`crate::privacy`] guard. Keyed to the acting
    /// user — the data-layer boundary the turn path must consult.
    pub fn conversation_access(
        &self,
        acting: &crate::identity::ActingUser,
        conversation_id: &str,
        grants: &crate::privacy::Grants,
    ) -> crate::privacy::Decision {
        match self.conversation_owner(conversation_id) {
            None => crate::privacy::Decision::Allow,
            Some(owner) => crate::privacy::can_access(acting, &owner, "conversation", grants),
        }
    }

    /// One-time, idempotent, lossless migration of legacy per-device
    /// conversations to the user-primary layout: for each device that has an
    /// owner, move its conversations under that user (registering the owner in
    /// the index and stamping each event with the device it happened on).
    /// Conversations on a device with no owner are left in place (the
    /// unattributed bucket). Verify-before-delete: the source is removed only
    /// after the destination is written and matches, so a crash never loses
    /// data; an already-migrated conversation is skipped. Returns how many moved.
    pub fn migrate_conversations(&self) -> Result<usize> {
        let devices_dir = self.devices_dir();
        let device_entries = match fs::read_dir(&devices_dir) {
            Ok(e) => e,
            Err(_) => return Ok(0),
        };
        let mut moved = 0usize;
        for dev in device_entries.flatten() {
            if !dev.path().is_dir() {
                continue;
            }
            let device_id = dev.file_name().to_string_lossy().into_owned();
            let (owner, _users, _shared) = self.device_ownership(&device_id).unwrap_or_default();
            let Some(owner) = owner else { continue };
            let conv_dir = dev.path().join("conversations");
            let convs = match fs::read_dir(&conv_dir) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for c in convs.flatten() {
                let src = c.path();
                if src.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(conv_id) = src.file_stem().and_then(|s| s.to_str()).map(String::from)
                else {
                    continue;
                };
                // Idempotent: an already-owned conversation has been migrated.
                if self.conversation_owner(&conv_id).is_some() {
                    continue;
                }
                // Stamp device_id into each event and write to the user path.
                let content = match fs::read_to_string(&src) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let mut out_lines = Vec::new();
                let mut src_lines = 0usize;
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    src_lines += 1;
                    let mut v: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => {
                            out_lines.push(line.to_string());
                            continue;
                        }
                    };
                    if let Some(obj) = v.as_object_mut() {
                        obj.entry("device_id")
                            .or_insert_with(|| Value::String(device_id.clone()));
                    }
                    out_lines.push(v.to_string());
                }
                // Registering the owner makes conversation_path route user-primary.
                self.register_conversation_owner(
                    &conv_id,
                    &crate::identity::ActingUser::User(owner.clone()),
                )?;
                let dest = self.conversation_path(&device_id, &conv_id);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        CoreError::Message(format!("cannot create {}: {e}", parent.display()))
                    })?;
                }
                let body = if out_lines.is_empty() {
                    String::new()
                } else {
                    format!("{}\n", out_lines.join("\n"))
                };
                if fs::write(&dest, &body).is_err() {
                    continue;
                }
                // Verify-before-delete: dest must have the same event count.
                let dest_ok = fs::read_to_string(&dest)
                    .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count() == src_lines)
                    .unwrap_or(false);
                if dest_ok {
                    let _ = fs::remove_file(&src);
                    moved += 1;
                }
            }
        }
        Ok(moved)
    }

    /// Append a message to a conversation's event stream; returns its `seq`
    /// (monotonic per conversation, the basis for resume/replay).
    pub fn append(&self, device_id: &str, conversation_id: &str, message: &Message) -> Result<u64> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let path = self.conversation_path(device_id, conversation_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        // Atomically assign seq: hold the lock across the count-then-write.
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| CoreError::Message("storage append lock poisoned".to_string()))?;
        let seq = read_events(&path)?.len() as u64 + 1;
        // Record the device the event happened on (privacy model stores
        // conversations user-primary; the device is "where", kept per event).
        let record = serde_json::json!({ "seq": seq, "device_id": device_id, "message": message });
        let line = serde_json::to_string(&record)
            .map_err(|e| CoreError::Message(format!("serialize message failed: {e}")))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CoreError::Message(format!("cannot open {}: {e}", path.display())))?;
        writeln!(file, "{line}")
            .map_err(|e| CoreError::Message(format!("write to {} failed: {e}", path.display())))?;
        Ok(seq)
    }

    /// Load a conversation's messages (empty if it does not exist yet).
    pub fn load(&self, device_id: &str, conversation_id: &str) -> Result<Vec<Message>> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let events = read_events(&self.conversation_path(device_id, conversation_id))?;
        Ok(events.into_iter().map(|e| e.message).collect())
    }

    /// Load stored events with `seq` greater than `after_seq` (for resume/replay).
    pub fn load_after(
        &self,
        device_id: &str,
        conversation_id: &str,
        after_seq: u64,
    ) -> Result<Vec<StoredEvent>> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let events = read_events(&self.conversation_path(device_id, conversation_id))?;
        Ok(events.into_iter().filter(|e| e.seq > after_seq).collect())
    }

    /// The store for rollback backups, outside any workspace.
    pub fn backups_dir(&self) -> PathBuf {
        self.home.join("fleet").join("backups")
    }

    /// Directory holding agent-level core memory files (ME/USER/TODO/TOOLS).
    pub fn memory_dir(&self) -> PathBuf {
        self.home.join("fleet")
    }

    /// Path to a device's audit log.
    pub fn history_path(&self, device_id: &str) -> PathBuf {
        self.home
            .join("fleet")
            .join("devices")
            .join(device_id)
            .join("history.jsonl")
    }

    /// Directory holding all device records.
    pub fn devices_dir(&self) -> PathBuf {
        self.home.join("fleet").join("devices")
    }

    /// Directory holding site (場域 / location) records.
    pub fn sites_dir(&self) -> PathBuf {
        self.home.join("fleet").join("sites")
    }

    /// Directory holding the agent's schedules.
    pub fn schedules_dir(&self) -> PathBuf {
        self.home.join("fleet").join("schedules")
    }

    /// Built-in skills (shipped with the runtime); read-only, replaced on update.
    pub fn skills_builtin_dir(&self) -> PathBuf {
        self.home.join("skills").join("builtin")
    }

    /// User-installed skills; preserved across updates. Overrides built-ins by name.
    pub fn skills_installed_dir(&self) -> PathBuf {
        self.home.join("skills").join("installed")
    }

    /// Agent-authored skills the agent writes for itself from experience
    /// (Hermes-style). The agent owns these fully — it may create/edit/delete
    /// them without user consent. Kept separate from `builtin` (shipped) and
    /// `installed` (user-chosen). Shadowing order at load: installed > authored
    /// > builtin.
    pub fn skills_authored_dir(&self) -> PathBuf {
        self.home.join("skills").join("authored")
    }

    /// Path to the connection-auth store (tokens + pairing codes).
    pub fn auth_path(&self) -> PathBuf {
        self.home.join("auth.json")
    }

    /// Path to the user-installed MCP server config (JSON). Shadows built-in
    /// servers of the same name.
    pub fn mcp_installed_config_path(&self) -> PathBuf {
        self.home.join("mcp").join("installed.json")
    }

    /// Built-in MCP servers (shipped with the runtime); seeded by
    /// `builtin_mcp::seed` at server boot, read-only from the user's PoV.
    /// `mcp_remove` refuses to delete built-ins — to override one, `mcp_add` it
    /// at `installed`, which shadows the built-in by name.
    pub fn mcp_builtin_config_path(&self) -> PathBuf {
        self.home.join("mcp").join("builtin.json")
    }

    /// The knowledge wiki vault (Obsidian-style markdown), separate from workspaces.
    /// Persistent cookie jars used by `http_request` / `ws_call` / `sse_stream`
    /// when the agent passes a `cookie_jar: <name>` to keep a session-bound
    /// API logged in across calls. Each jar is a single JSON file under here.
    pub fn cookies_dir(&self) -> PathBuf {
        self.home.join("fleet").join("cookies")
    }

    pub fn wiki_dir(&self) -> PathBuf {
        self.home.join("wiki")
    }

    /// Local model cache (e.g. the EmbeddingGemma weights fastembed downloads
    /// for wiki semantic search). Override with `FLEETY_MODELS_DIR`.
    pub fn models_dir(&self) -> PathBuf {
        if let Ok(d) = std::env::var("FLEETY_MODELS_DIR") {
            if !d.is_empty() {
                return PathBuf::from(d);
            }
        }
        self.home.join("models")
    }

    /// Ensure a device is registered: create `devices/{id}/device.json` (with
    /// defaults) and an initial `NOTES.md` if missing, and stamp `last_seen`.
    /// v0 stores the record as JSON; the spec's device.yaml has the same fields.
    pub fn ensure_device(&self, device_id: &str, connector_type: &str) -> Result<()> {
        validate_id("device_id", device_id)?;
        let dir = self.devices_dir().join(device_id);
        fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Message(format!("cannot create device dir: {e}")))?;
        let record_path = dir.join("device.json");
        let mut record = match fs::read_to_string(&record_path) {
            Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        if !record.is_object() {
            record = serde_json::json!({
                "id": device_id,
                "status": "active",
                "mobility": "unknown",
                "site": "unknown",
                "connectors": [{ "type": connector_type, "scope": "local" }],
                "owner": Value::Null,
                "users": [],
                "shared": false,
            });
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        record["last_seen"] = serde_json::json!(now);
        let pretty = serde_json::to_string_pretty(&record)
            .map_err(|e| CoreError::Message(format!("serialize device record: {e}")))?;
        fs::write(&record_path, pretty)
            .map_err(|e| CoreError::Message(format!("write device record: {e}")))?;
        let notes = dir.join("NOTES.md");
        if !notes.exists() {
            fs::write(
                &notes,
                format!("# {device_id}\n\nAuto-registered device.\n"),
            )
            .map_err(|e| CoreError::Message(format!("write NOTES.md: {e}")))?;
        }
        Ok(())
    }

    fn core_file(&self, name: &str, default: &str) -> Result<String> {
        let path = self.home.join("fleet").join(name);
        match fs::read_to_string(&path) {
            Ok(content) => Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        CoreError::Message(format!("cannot create {}: {e}", parent.display()))
                    })?;
                }
                fs::write(&path, default).map_err(|e| {
                    CoreError::Message(format!("cannot write {}: {e}", path.display()))
                })?;
                Ok(default.to_string())
            }
            Err(e) => Err(CoreError::Message(format!(
                "cannot read {}: {e}",
                path.display()
            ))),
        }
    }

    /// Read agent-level core memory for the acting user: ME and TODO are
    /// agent-global, but the USER block is the **acting user's** profile (or a
    /// neutral placeholder for a Guest), so each turn carries the right person.
    pub fn core_memory_for(&self, acting: &crate::identity::ActingUser) -> Result<String> {
        let me = self.core_file("ME.md", DEFAULT_ME)?;
        let user = self.user_profile(acting)?;
        let todo = self.core_file("TODO.md", DEFAULT_TODO)?;
        Ok(format!(
            "You are operating with the following core memory.\n\n## ME (self)\n{me}\n\n## USER\n{user}\n\n## TODO\n{todo}"
        ))
    }

    /// The acting user resolved from a device's ownership alone (no assertion):
    /// the device owner, else Guest. Used by unattended/device-scoped prompt
    /// builds (recover, scheduler, subagent); the interactive turn additionally
    /// honors an asserted user (see conn).
    pub fn acting_for_device(&self, device_id: &str) -> crate::identity::ActingUser {
        match self.device_ownership(device_id) {
            Ok((owner, users, _shared)) => {
                crate::identity::resolve_acting_user(owner.as_deref(), &users, None)
            }
            Err(_) => crate::identity::ActingUser::Guest,
        }
    }

    /// The full system prompt for the acting user: the static behavioural docs
    /// (protocol → rules → memory → policy, embedded at build time) followed by
    /// the acting user's core memory. Kept at index 0 by the run loop and
    /// preserved across compaction. `FLEETY_SYSTEM_PROMPT=minimal` drops the
    /// static docs (core memory only) for token-lean / debugging runs.
    pub fn system_prompt_for(&self, acting: &crate::identity::ActingUser) -> Result<String> {
        let core = self.core_memory_for(acting)?;
        if std::env::var("FLEETY_SYSTEM_PROMPT").as_deref() == Ok("minimal") {
            return Ok(core);
        }
        Ok(format!(
            "{PROTOCOL_MD}\n\n---\n\n{RULES_MD}\n\n---\n\n{MEMORY_MD}\n\n---\n\n{POLICY_MD}\n\n---\n\n# Core Memory\n\n{core}"
        ))
    }

    /// Directory holding per-user state: `fleet/users/`.
    fn users_dir(&self) -> PathBuf {
        self.home.join("fleet").join("users")
    }

    /// The acting user's `USER.md` profile, creating a default if missing. For a
    /// [`ActingUser::Guest`] this is a neutral placeholder with no personal data
    /// (so a guest turn never carries another person's profile).
    pub fn user_profile(&self, acting: &crate::identity::ActingUser) -> Result<String> {
        match acting.user_id() {
            Some(id) => {
                validate_id("user_id", id)?;
                let path = self.users_dir().join(id).join("USER.md");
                match fs::read_to_string(&path) {
                    Ok(content) => Ok(content),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if let Some(parent) = path.parent() {
                            fs::create_dir_all(parent).map_err(|e| {
                                CoreError::Message(format!(
                                    "cannot create {}: {e}",
                                    parent.display()
                                ))
                            })?;
                        }
                        fs::write(&path, DEFAULT_USER).map_err(|e| {
                            CoreError::Message(format!("cannot write {}: {e}", path.display()))
                        })?;
                        Ok(DEFAULT_USER.to_string())
                    }
                    Err(e) => Err(CoreError::Message(format!(
                        "cannot read {}: {e}",
                        path.display()
                    ))),
                }
            }
            None => Ok(GUEST_PROFILE.to_string()),
        }
    }

    /// Write a user's `USER.md` profile. Part of the identity store API consumed
    /// by the per-user memory write path (privacy-isolation) and tested here.
    #[allow(dead_code)]
    pub fn write_user_profile(&self, user_id: &str, content: &str) -> Result<()> {
        validate_id("user_id", user_id)?;
        let dir = self.users_dir().join(user_id);
        fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Message(format!("cannot create user dir: {e}")))?;
        fs::write(dir.join("USER.md"), content)
            .map_err(|e| CoreError::Message(format!("write USER.md: {e}")))?;
        Ok(())
    }

    /// List known user ids (the users index = the subdirectories of `users/`).
    /// Identity store API consumed by interactive-config / admin surfaces.
    #[allow(dead_code)]
    pub fn list_users(&self) -> Result<Vec<String>> {
        let dir = self.users_dir();
        let mut out = Vec::new();
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            out.push(name.to_string());
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CoreError::Message(format!("read users dir: {e}"))),
        }
        out.sort();
        Ok(out)
    }

    /// Read a device's ownership: `(owner, authorized users, shared)`. Missing
    /// fields default to `(None, [], false)` so legacy device records still load.
    pub fn device_ownership(&self, device_id: &str) -> Result<(Option<String>, Vec<String>, bool)> {
        validate_id("device_id", device_id)?;
        let path = self.devices_dir().join(device_id).join("device.json");
        let record: Value = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        let owner = record
            .get("owner")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let users = record
            .get("users")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let shared = record
            .get("shared")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok((owner, users, shared))
    }

    /// Set a device's ownership fields, preserving the rest of the record.
    /// Identity store API consumed by device configuration (interactive-config).
    #[allow(dead_code)]
    pub fn set_device_ownership(
        &self,
        device_id: &str,
        owner: Option<&str>,
        users: &[String],
        shared: bool,
    ) -> Result<()> {
        validate_id("device_id", device_id)?;
        let path = self.devices_dir().join(device_id).join("device.json");
        let mut record: Value = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        if !record.is_object() {
            record = serde_json::json!({ "id": device_id });
        }
        record["owner"] = match owner {
            Some(o) => Value::String(o.to_string()),
            None => Value::Null,
        };
        record["users"] = Value::Array(users.iter().map(|u| Value::String(u.clone())).collect());
        record["shared"] = Value::Bool(shared);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create device dir: {e}")))?;
        }
        let pretty = serde_json::to_string_pretty(&record)
            .map_err(|e| CoreError::Message(format!("serialize device record: {e}")))?;
        fs::write(&path, pretty)
            .map_err(|e| CoreError::Message(format!("write device record: {e}")))?;
        Ok(())
    }

    /// A compact summary of one audit log line — what the CLI shows in
    /// `fleety audit list` so the user can browse without parsing the full
    /// event payload.
    pub fn list_audit(
        &self,
        device_id: &str,
        since_secs: Option<u64>,
        limit: Option<u32>,
    ) -> Result<Vec<serde_json::Value>> {
        validate_id("device_id", device_id)?;
        let path = self.history_path(device_id);
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(CoreError::Message(format!("read audit: {e}"))),
        };
        let mut all: Vec<serde_json::Value> = Vec::new();
        for (idx, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| CoreError::Message(format!("read audit line: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let (kind, tool) = summarise_event(&value);
            // `ts_secs` is on new lines (this is the field we wrote). Old lines
            // (pre-timestamp) have no such field and surface as 0 — the CLI
            // renders that as "—" so a mixed history doesn't lie.
            let ts_secs = value.get("ts_secs").and_then(Value::as_u64).unwrap_or(0);
            // Drop pre-`since` lines but keep ts_secs == 0 (unknown time) so
            // a fresh `--since` doesn't silently swallow the whole pre-feature
            // history.
            if let Some(since) = since_secs {
                if ts_secs != 0 && ts_secs < since {
                    continue;
                }
            }
            let summary = serde_json::json!({
                "index": idx as u64,
                "kind": kind,
                "tool": tool,
                "ts_secs": ts_secs,
            });
            all.push(summary);
        }
        if let Some(limit) = limit {
            let limit = limit as usize;
            if all.len() > limit {
                let start = all.len() - limit;
                all = all.split_off(start);
            }
        }
        Ok(all)
    }

    /// Read one audit entry by line index (0-based, matches what `list_audit`
    /// returns). Returns the full event JSON so the caller (CLI) can render
    /// the entire payload.
    pub fn read_audit(&self, device_id: &str, index: u64) -> Result<serde_json::Value> {
        validate_id("device_id", device_id)?;
        let path = self.history_path(device_id);
        let file = File::open(&path).map_err(|e| CoreError::Message(format!("open audit: {e}")))?;
        for (idx, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| CoreError::Message(format!("read audit line: {e}")))?;
            if idx as u64 != index {
                continue;
            }
            let value: Value = serde_json::from_str(&line)
                .map_err(|e| CoreError::Message(format!("corrupt audit line: {e}")))?;
            return Ok(value);
        }
        Err(CoreError::Message(format!(
            "no audit entry at index {index}"
        )))
    }

    /// Append an event to a device's audit log (`history.jsonl`). Each line
    /// carries `ts_secs` (unix seconds, when the event was recorded) alongside
    /// the event's own fields — serde `flatten` keeps the existing per-event
    /// shape so old readers (including `HistoryList` and any lines written
    /// before this change) keep working: `ts_secs` is just one more field to
    /// ignore, and old lines without it parse with `ts_secs == 0`.
    pub fn append_history(&self, device_id: &str, event: &Event) -> Result<()> {
        validate_id("device_id", device_id)?;
        let path = self.history_path(device_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let record = AuditRecord {
            ts_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            event: event.clone(),
        };
        let line = serde_json::to_string(&record)
            .map_err(|e| CoreError::Message(format!("serialize audit event failed: {e}")))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CoreError::Message(format!("cannot open {}: {e}", path.display())))?;
        writeln!(file, "{line}")
            .map_err(|e| CoreError::Message(format!("write audit failed: {e}")))?;
        Ok(())
    }

    /// Path to a conversation's in-flight turn journal (durable record of the
    /// current turn's events, removed once the turn completes).
    fn journal_path(&self, device_id: &str, conversation_id: &str) -> PathBuf {
        self.home
            .join("fleet")
            .join("devices")
            .join(device_id)
            .join("conversations")
            .join(format!("{conversation_id}.journal.jsonl"))
    }

    /// Begin a turn journal: (re)create the file with the starting user message.
    pub fn journal_begin(
        &self,
        device_id: &str,
        conversation_id: &str,
        user: &Message,
    ) -> Result<()> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let path = self.journal_path(device_id, conversation_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let record = serde_json::json!({ "kind": "start", "message": user });
        let line = serde_json::to_string(&record)
            .map_err(|e| CoreError::Message(format!("serialize journal start: {e}")))?;
        // Truncating create: a fresh journal per turn.
        fs::write(&path, format!("{line}\n"))
            .map_err(|e| CoreError::Message(format!("write {} failed: {e}", path.display())))?;
        Ok(())
    }

    /// Append one loop event to the current turn journal (called as it happens).
    pub fn journal_event(
        &self,
        device_id: &str,
        conversation_id: &str,
        event: &Event,
    ) -> Result<()> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let path = self.journal_path(device_id, conversation_id);
        let record = serde_json::json!({ "kind": "event", "event": event });
        let line = serde_json::to_string(&record)
            .map_err(|e| CoreError::Message(format!("serialize journal event: {e}")))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CoreError::Message(format!("cannot open {}: {e}", path.display())))?;
        writeln!(file, "{line}")
            .map_err(|e| CoreError::Message(format!("write journal failed: {e}")))?;
        Ok(())
    }

    /// Finish a turn: remove its journal (the result now lives in the stream).
    pub fn journal_end(&self, device_id: &str, conversation_id: &str) -> Result<()> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let path = self.journal_path(device_id, conversation_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::Message(format!(
                "cannot remove {}: {e}",
                path.display()
            ))),
        }
    }

    /// Read the journaled loop events for an interrupted turn (empty if none).
    pub fn journal_events(&self, device_id: &str, conversation_id: &str) -> Result<Vec<Event>> {
        validate_id("device_id", device_id)?;
        validate_id("conversation_id", conversation_id)?;
        let path = self.journal_path(device_id, conversation_id);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(CoreError::Message(format!(
                    "cannot read {}: {e}",
                    path.display()
                )))
            }
        };
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| CoreError::Message(format!("read journal line: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&line)
                .map_err(|e| CoreError::Message(format!("corrupt journal line: {e}")))?;
            if value.get("kind").and_then(Value::as_str) != Some("event") {
                continue;
            }
            let event: Event =
                serde_json::from_value(value.get("event").cloned().unwrap_or(Value::Null))
                    .map_err(|e| CoreError::Message(format!("corrupt journal event: {e}")))?;
            events.push(event);
        }
        Ok(events)
    }

    /// An [`EventLog`] that journals each event to a conversation's turn journal
    /// the instant it happens, so a crash mid-turn is recoverable. Requires an
    /// `Arc<Storage>` so the sink can outlive the call.
    pub fn journaling_log(self: &Arc<Self>, device_id: &str, conversation_id: &str) -> EventLog {
        let storage = Arc::clone(self);
        let device = device_id.to_string();
        let conv = conversation_id.to_string();
        EventLog::with_sink(Box::new(move |event: &Event| {
            if let Err(e) = storage.journal_event(&device, &conv, event) {
                tracing::warn!(report = ?e.report(), "could not journal turn event");
            }
        }))
    }

    /// List `(device_id, conversation_id)` pairs that have an unfinished turn
    /// journal — used to recover interrupted turns.
    pub fn list_incomplete_turns(&self) -> Result<Vec<(String, String)>> {
        let mut out = Vec::new();
        let devices = self.devices_dir();
        let device_entries = match fs::read_dir(&devices) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(CoreError::Message(format!("cannot list devices: {e}"))),
        };
        for device in device_entries.flatten() {
            let device_id = device.file_name().to_string_lossy().to_string();
            let convs = device.path().join("conversations");
            let Ok(conv_entries) = fs::read_dir(&convs) else {
                continue;
            };
            for entry in conv_entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(conv) = name.strip_suffix(".journal.jsonl") {
                    out.push((device_id.clone(), conv.to_string()));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::Storage;
    use agent_core::Message;
    use std::path::PathBuf;

    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-storage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    #[test]
    fn seq_increments_and_load_after_filters() {
        let home = temp_home();
        let storage = Storage::new(home.clone());

        let s1 = storage
            .append("dev", "conv", &Message::user("one"))
            .expect("a1");
        let s2 = storage
            .append("dev", "conv", &Message::assistant("two"))
            .expect("a2");
        let s3 = storage
            .append("dev", "conv", &Message::user("three"))
            .expect("a3");
        assert_eq!((s1, s2, s3), (1, 2, 3));

        let all = storage.load("dev", "conv").expect("load");
        assert_eq!(all.len(), 3);

        let after = storage.load_after("dev", "conv", 1).expect("after");
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].seq, 2);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn user_profile_per_user_and_guest() {
        use crate::identity::ActingUser;
        let home = temp_home();
        let storage = Storage::new(home.clone());
        // Missing → created with the default; round-trips after a write.
        let alice = ActingUser::User("alice".to_string());
        assert!(storage
            .user_profile(&alice)
            .expect("p")
            .contains("Unknown so far"));
        storage
            .write_user_profile("alice", "Alice likes tea.")
            .expect("w");
        assert_eq!(
            storage.user_profile(&alice).expect("p2"),
            "Alice likes tea."
        );
        // A second user is independent.
        let bob = ActingUser::User("bob".to_string());
        storage
            .write_user_profile("bob", "Bob likes coffee.")
            .expect("wb");
        assert_eq!(storage.user_profile(&bob).expect("pb"), "Bob likes coffee.");
        assert_eq!(
            storage.user_profile(&alice).expect("pa"),
            "Alice likes tea."
        );
        // Guest → neutral placeholder, no personal data.
        let guest = storage.user_profile(&ActingUser::Guest).expect("pg");
        assert!(guest.contains("guest"));
        assert!(!guest.contains("tea") && !guest.contains("coffee"));
        // Index lists the known users.
        let mut users = storage.list_users().expect("list");
        users.sort();
        assert_eq!(users, vec!["alice".to_string(), "bob".to_string()]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn conversations_are_user_primary_and_access_guarded() {
        use crate::identity::ActingUser;
        use crate::privacy::{Decision, Grants};
        let home = temp_home();
        let storage = Storage::new(home.clone());
        let alice = ActingUser::User("alice".into());
        storage.register_conversation_owner("c1", &alice).unwrap();
        storage
            .append("laptop", "c1", &Message::user("hi"))
            .unwrap();
        // Stored under users/alice, not devices/laptop.
        let user_path = home
            .join("fleet")
            .join("users")
            .join("alice")
            .join("conversations")
            .join("c1.jsonl");
        let dev_path = home
            .join("fleet")
            .join("devices")
            .join("laptop")
            .join("conversations")
            .join("c1.jsonl");
        assert!(user_path.exists(), "conversation is user-primary");
        assert!(!dev_path.exists(), "not at the device path");
        // load resolves via the index regardless of the device passed.
        assert_eq!(storage.load("laptop", "c1").unwrap().len(), 1);
        // Guard: owner Allow; other user / guest Deny; an unowned conv is open.
        let grants = Grants::default();
        assert_eq!(
            storage.conversation_access(&alice, "c1", &grants),
            Decision::Allow
        );
        assert_eq!(
            storage.conversation_access(&ActingUser::User("bob".into()), "c1", &grants),
            Decision::Deny
        );
        assert_eq!(
            storage.conversation_access(&ActingUser::Guest, "c1", &grants),
            Decision::Deny
        );
        assert_eq!(
            storage.conversation_access(&alice, "unowned", &grants),
            Decision::Allow
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn migrate_moves_owned_conversations_losslessly_and_idempotently() {
        let home = temp_home();
        let storage = Storage::new(home.clone());
        // Legacy: a conversation under the device, before any owner is set.
        storage
            .append("laptop", "leg", &Message::user("one"))
            .unwrap();
        storage
            .append("laptop", "leg", &Message::assistant("two"))
            .unwrap();
        let dev_path = home
            .join("fleet")
            .join("devices")
            .join("laptop")
            .join("conversations")
            .join("leg.jsonl");
        let user_path = home
            .join("fleet")
            .join("users")
            .join("alice")
            .join("conversations")
            .join("leg.jsonl");
        assert!(dev_path.exists() && !user_path.exists());
        // Give the device an owner, then migrate.
        storage
            .set_device_ownership("laptop", Some("alice"), &[], false)
            .unwrap();
        assert_eq!(storage.migrate_conversations().unwrap(), 1);
        assert!(
            user_path.exists() && !dev_path.exists(),
            "moved to the owner"
        );
        assert_eq!(storage.load("laptop", "leg").unwrap().len(), 2, "lossless");
        assert_eq!(storage.conversation_owner("leg").as_deref(), Some("alice"));
        assert!(
            std::fs::read_to_string(&user_path)
                .unwrap()
                .contains("\"device_id\":\"laptop\""),
            "device recorded per event"
        );
        // Idempotent re-run is a no-op.
        assert_eq!(storage.migrate_conversations().unwrap(), 0);
        // A device with no owner is left in place (unattributed).
        storage.append("kiosk", "k1", &Message::user("x")).unwrap();
        assert_eq!(storage.migrate_conversations().unwrap(), 0);
        assert!(home
            .join("fleet")
            .join("devices")
            .join("kiosk")
            .join("conversations")
            .join("k1.jsonl")
            .exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn core_memory_uses_the_acting_users_profile() {
        use crate::identity::ActingUser;
        let home = temp_home();
        let storage = Storage::new(home.clone());
        storage
            .write_user_profile("alice", "Alice's profile")
            .unwrap();
        let cm = storage
            .core_memory_for(&ActingUser::User("alice".into()))
            .expect("cm");
        assert!(cm.contains("Alice's profile"), "USER block is alice's");
        assert!(
            cm.contains("## ME") && cm.contains("## TODO"),
            "ME/TODO stay global"
        );
        // Guest → neutral USER, no personal data.
        let guest = storage.core_memory_for(&ActingUser::Guest).expect("g");
        assert!(guest.contains("guest"));
        assert!(!guest.contains("Alice's profile"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn device_ownership_defaults_and_roundtrip() {
        let home = temp_home();
        let storage = Storage::new(home.clone());
        // ensure_device creates a record; ownership defaults to (None, [], false).
        storage.ensure_device("laptop", "local").expect("ensure");
        assert_eq!(
            storage.device_ownership("laptop").expect("own"),
            (None, Vec::<String>::new(), false)
        );
        // Set and read back; ensure_device must not clobber it afterwards.
        storage
            .set_device_ownership("laptop", Some("alice"), &["alice".to_string()], false)
            .expect("set");
        storage.ensure_device("laptop", "local").expect("ensure2");
        assert_eq!(
            storage.device_ownership("laptop").expect("own2"),
            (Some("alice".to_string()), vec!["alice".to_string()], false)
        );
        // A legacy device.json without ownership fields → defaults.
        let legacy = storage.devices_dir().join("old").join("device.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"id":"old","status":"active"}"#).unwrap();
        assert_eq!(
            storage.device_ownership("old").expect("own3"),
            (None, Vec::<String>::new(), false)
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn load_missing_is_empty() {
        let home = temp_home();
        let storage = Storage::new(home.clone());
        assert!(storage.load("dev", "none").expect("load").is_empty());
        assert!(storage
            .load_after("dev", "none", 0)
            .expect("after")
            .is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn turn_journal_records_events_and_lists_incomplete() {
        use agent_core::{Event, Message};
        let home = temp_home();
        let storage = Storage::new(home.clone());

        storage
            .journal_begin("dev", "conv", &Message::user("hi"))
            .expect("begin");
        storage
            .journal_event(
                "dev",
                "conv",
                &Event::ToolResult {
                    id: "a".into(),
                    result: serde_json::json!({ "ok": true }),
                },
            )
            .expect("event");

        // The incomplete turn is discoverable and its events readable.
        let incomplete = storage.list_incomplete_turns().expect("list");
        assert_eq!(incomplete, vec![("dev".to_string(), "conv".to_string())]);
        let events = storage.journal_events("dev", "conv").expect("events");
        assert_eq!(events.len(), 1);

        // After ending, nothing remains.
        storage.journal_end("dev", "conv").expect("end");
        assert!(storage.list_incomplete_turns().expect("list2").is_empty());
        assert!(storage
            .journal_events("dev", "conv")
            .expect("ev2")
            .is_empty());
        storage
            .journal_end("dev", "conv")
            .expect("end is idempotent");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn audit_list_summarises_events() {
        use agent_core::{Event, ToolCall};
        let home = temp_home();
        let storage = Storage::new(home.clone());

        // Three events: tool call, tool result, assistant.
        storage
            .append_history(
                "dev",
                &Event::ToolCall(ToolCall {
                    id: "1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({}),
                }),
            )
            .expect("append1");
        storage
            .append_history(
                "dev",
                &Event::ToolResult {
                    id: "1".into(),
                    result: serde_json::json!({ "ok": true }),
                },
            )
            .expect("append2");
        storage
            .append_history(
                "dev",
                &Event::Assistant(agent_core::Message::assistant("done")),
            )
            .expect("append3");

        // list returns all three with kind+tool fields populated where relevant.
        let entries = storage.list_audit("dev", None, None).expect("list");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["kind"], serde_json::json!("tool_call"));
        assert_eq!(entries[0]["tool"], serde_json::json!("read_file"));
        assert_eq!(entries[2]["kind"], serde_json::json!("assistant"));
        // Every freshly appended line carries a real timestamp now.
        let ts0 = entries[0]["ts_secs"].as_u64().expect("ts0");
        assert!(ts0 > 0, "ts_secs must be populated on new lines");

        // limit returns the LAST N (most recent).
        let entries = storage.list_audit("dev", None, Some(2)).expect("limit");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["index"], serde_json::json!(1u64));
        assert_eq!(entries[1]["index"], serde_json::json!(2u64));

        // since-filter: a since in the future should drop everything; a since
        // in the deep past should keep everything.
        let future = ts0 + 10_000;
        assert!(storage
            .list_audit("dev", Some(future), None)
            .expect("future")
            .is_empty());
        assert_eq!(
            storage
                .list_audit("dev", Some(0), None)
                .expect("epoch")
                .len(),
            3
        );

        // show by index returns the full event (with ts_secs alongside).
        let one = storage.read_audit("dev", 0).expect("show");
        assert_eq!(one["event"], serde_json::json!("tool_call"));
        assert!(one["ts_secs"].as_u64().unwrap_or(0) > 0);
        assert!(storage.read_audit("dev", 99).is_err());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn audit_list_surfaces_approval_denials() {
        // When `agent_core` records a denial, it pushes a synthetic
        // ToolResult event whose `result.denied == true` and `result.tool`
        // carries the tool name. The audit listing should call that out
        // ("tool_denied") rather than hide it behind a generic "tool_result".
        use agent_core::{Event, ToolCall};
        let home = temp_home();
        let storage = Storage::new(home.clone());

        storage
            .append_history(
                "dev",
                &Event::ToolCall(ToolCall {
                    id: "1".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({}),
                }),
            )
            .expect("call");
        storage
            .append_history(
                "dev",
                &Event::ToolResult {
                    id: "1".into(),
                    result: serde_json::json!({
                        "denied": true,
                        "tool": "write_file",
                        "reason": "user denied"
                    }),
                },
            )
            .expect("denial");

        let entries = storage.list_audit("dev", None, None).expect("list");
        assert_eq!(entries[1]["kind"], serde_json::json!("tool_denied"));
        assert_eq!(entries[1]["tool"], serde_json::json!("write_file"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn audit_list_treats_legacy_lines_as_timeless() {
        // Pre-timestamp lines (no `ts_secs`) still parse, but a `--since`
        // shouldn't silently drop them — we surface ts_secs == 0 instead.
        use std::io::Write;
        let home = temp_home();
        let storage = Storage::new(home.clone());

        let path = storage.history_path("dev");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&path).expect("touch");
        // Legacy line: bare Event JSON, no ts_secs.
        writeln!(
            f,
            r#"{{"event":"tool_call","id":"1","name":"read_file","arguments":{{}}}}"#
        )
        .expect("write legacy");
        drop(f);

        let entries = storage.list_audit("dev", None, None).expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["ts_secs"], serde_json::json!(0));
        // since-filter with ts == 0 is treated as "unknown time": kept, not
        // silently dropped.
        let entries = storage
            .list_audit("dev", Some(1_000_000_000), None)
            .expect("since");
        assert_eq!(entries.len(), 1);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn rejects_path_traversal_ids() {
        let home = temp_home();
        let storage = Storage::new(home.clone());
        assert!(storage.append("../evil", "c", &Message::user("x")).is_err());
        assert!(storage
            .append("dev", "../../evil", &Message::user("x"))
            .is_err());
        assert!(storage.append("a/b", "c", &Message::user("x")).is_err());
        assert!(storage.load("..", "c").is_err());
        assert!(storage.ensure_device("../x", "client_session").is_err());
        // A normal id still works.
        assert!(storage.append("dev", "conv", &Message::user("ok")).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }
}
