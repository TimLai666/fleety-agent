//! Conversation persistence: one JSONL file of [`Message`]s per conversation,
//! under the Agent home, separate from any workspace (spec: workspace = dirty
//! work, durable state lives in the Fleety store).

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::{CoreError, Event, EventLog, Message, Result};
use serde_json::Value;

const DEFAULT_ME: &str = "Your name is Fleety. You are a cross-device, full-access agent that helps the user operate their devices. You act autonomously, keep an audit trail, and can roll back; you confirm only genuinely irreversible actions.";
const DEFAULT_USER: &str = "(Unknown so far. Record what you learn about the user here.)";
const DEFAULT_TODO: &str = "(No current to-dos.)";

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
        self.home
            .join("fleet")
            .join("devices")
            .join(device_id)
            .join("conversations")
            .join(format!("{conversation_id}.jsonl"))
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
        let record = serde_json::json!({ "seq": seq, "message": message });
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

    /// Path to the connection-auth store (tokens + pairing codes).
    pub fn auth_path(&self) -> PathBuf {
        self.home.join("auth.json")
    }

    /// Path to the user-installed MCP server config (JSON).
    pub fn mcp_config_path(&self) -> PathBuf {
        self.home.join("mcp").join("installed.json")
    }

    /// The knowledge wiki vault (Obsidian-style markdown), separate from workspaces.
    pub fn wiki_dir(&self) -> PathBuf {
        self.home.join("wiki")
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

    /// Read agent-level core memory (ME/USER/TODO), creating defaults if missing,
    /// as a single system-prompt block to inject each turn (`ME.md` defaults to Fleety).
    pub fn core_memory(&self) -> Result<String> {
        let me = self.core_file("ME.md", DEFAULT_ME)?;
        let user = self.core_file("USER.md", DEFAULT_USER)?;
        let todo = self.core_file("TODO.md", DEFAULT_TODO)?;
        Ok(format!(
            "You are operating with the following core memory.\n\n## ME (self)\n{me}\n\n## USER\n{user}\n\n## TODO\n{todo}"
        ))
    }

    /// Append an event to a device's audit log (`history.jsonl`).
    pub fn append_history(&self, device_id: &str, event: &Event) -> Result<()> {
        validate_id("device_id", device_id)?;
        let path = self.history_path(device_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let line = serde_json::to_string(event)
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
