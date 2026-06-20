//! Conversation persistence: one JSONL file of [`Message`]s per conversation,
//! under the Agent home, separate from any workspace (spec: workspace = dirty
//! work, durable state lives in the Fleety store).

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use agent_core::{CoreError, Event, Message, Result};

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

    /// Append one message to a conversation's event stream.
    pub fn append(&self, device_id: &str, conversation_id: &str, message: &Message) -> Result<()> {
        let path = self.conversation_path(device_id, conversation_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let line = serde_json::to_string(message)
            .map_err(|e| CoreError::Message(format!("serialize message failed: {e}")))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CoreError::Message(format!("cannot open {}: {e}", path.display())))?;
        writeln!(file, "{line}")
            .map_err(|e| CoreError::Message(format!("write to {} failed: {e}", path.display())))?;
        Ok(())
    }

    /// Load a conversation's messages (empty if it does not exist yet).
    pub fn load(&self, device_id: &str, conversation_id: &str) -> Result<Vec<Message>> {
        let path = self.conversation_path(device_id, conversation_id);
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
        let mut messages = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| CoreError::Message(format!("read line failed: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }
            let message: Message = serde_json::from_str(&line)
                .map_err(|e| CoreError::Message(format!("corrupt conversation line: {e}")))?;
            messages.push(message);
        }
        Ok(messages)
    }

    /// The store for rollback backups, outside any workspace.
    pub fn backups_dir(&self) -> PathBuf {
        self.home.join("fleet").join("backups")
    }

    /// Append an event to a device's audit log (`history.jsonl`).
    pub fn append_history(&self, device_id: &str, event: &Event) -> Result<()> {
        let path = self
            .home
            .join("fleet")
            .join("devices")
            .join(device_id)
            .join("history.jsonl");
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
