//! Conversation persistence: one JSONL file of [`Message`]s per conversation,
//! under the Agent home, separate from any workspace (spec: workspace = dirty
//! work, durable state lives in the Fleety store).

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::{CoreError, Event, Message, Result};
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

/// Filesystem-backed conversation store rooted at the Agent home.
pub struct Storage {
    home: PathBuf,
}

impl Storage {
    pub fn new(home: PathBuf) -> Self {
        Self { home }
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
        let path = self.conversation_path(device_id, conversation_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
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

    /// Directory holding the agent's schedules.
    pub fn schedules_dir(&self) -> PathBuf {
        self.home.join("fleet").join("schedules")
    }

    /// Ensure a device is registered: create `devices/{id}/device.json` (with
    /// defaults) and an initial `NOTES.md` if missing, and stamp `last_seen`.
    /// v0 stores the record as JSON; the spec's device.yaml has the same fields.
    pub fn ensure_device(&self, device_id: &str, connector_type: &str) -> Result<()> {
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
}
