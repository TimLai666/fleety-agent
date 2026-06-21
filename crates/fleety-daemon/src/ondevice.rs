//! On-device tools fleetyd runs locally when the server dispatches a `RunTool`.
//!
//! A focused set — read/list/write files and run commands on *this* device,
//! rooted at `FLEETY_DEVICE_ROOT` (default: the daemon's working dir) with the
//! same path-escape guard the server's workspace tools use.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// The device filesystem root the on-device tools operate within.
pub fn device_root() -> PathBuf {
    std::env::var("FLEETY_DEVICE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Build the on-device tool registry rooted at `root`.
pub fn build_local_registry(root: &Path) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFile {
        root: root.to_path_buf(),
    }));
    registry.register(Box::new(ListDir {
        root: root.to_path_buf(),
    }));
    registry.register(Box::new(WriteFile {
        root: root.to_path_buf(),
    }));
    registry.register(Box::new(RunCommand {
        root: root.to_path_buf(),
    }));
    registry
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Resolve an existing path inside `root`, refusing escapes.
fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("device root unavailable: {e}")))?;
    let target = canon_root.join(rel);
    let canon = target
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("path '{rel}' not found: {e}")))?;
    if !canon.starts_with(&canon_root) {
        return Err(CoreError::Message(format!(
            "path '{rel}' escapes the device root"
        )));
    }
    Ok(canon)
}

/// Resolve a path for writing (parent must exist and stay inside `root`).
fn resolve_for_write(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("device root unavailable: {e}")))?;
    let target = canon_root.join(rel);
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::Message(format!("invalid path '{rel}'")))?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("parent of '{rel}' not found: {e}")))?;
    if !canon_parent.starts_with(&canon_root) {
        return Err(CoreError::Message(format!(
            "path '{rel}' escapes the device root"
        )));
    }
    let file_name = target
        .file_name()
        .ok_or_else(|| CoreError::Message(format!("path '{rel}' has no file name")))?;
    Ok(canon_parent.join(file_name))
}

struct ReadFile {
    root: PathBuf,
}

#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a UTF-8 file on this device.".to_string(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rel = require_str(&args, "path")?;
        let path = resolve_in_root(&self.root, rel)?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| CoreError::Message(format!("cannot read '{rel}': {e}")))?;
        Ok(json!({ "path": rel, "content": content }))
    }
}

struct ListDir {
    root: PathBuf,
}

#[async_trait]
impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".to_string(),
            description: "List a directory on this device.".to_string(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let path = resolve_in_root(&self.root, rel)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path)
            .map_err(|e| CoreError::Message(format!("cannot list '{rel}': {e}")))?
            .flatten()
        {
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(json!({ "name": entry.file_name().to_string_lossy(), "dir": is_dir }));
        }
        Ok(json!({ "path": rel, "entries": entries }))
    }
}

struct WriteFile {
    root: PathBuf,
}

#[async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),
            description: "Write a UTF-8 file on this device (overwrites).".to_string(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let rel = require_str(&args, "path")?;
        let content = require_str(&args, "content")?;
        let path = resolve_for_write(&self.root, rel)?;
        std::fs::write(&path, content)
            .map_err(|e| CoreError::Message(format!("cannot write '{rel}': {e}")))?;
        Ok(json!({ "path": rel, "bytes_written": content.len() }))
    }
}

struct RunCommand {
    root: PathBuf,
}

#[async_trait]
impl Tool for RunCommand {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "run_command".to_string(),
            description: "Run a shell command on this device and capture its output.".to_string(),
            parameters: json!({ "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let command = require_str(&args, "command")?;
        let (program, flag) = if cfg!(target_os = "windows") {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let output = Command::new(program)
            .arg(flag)
            .arg(command)
            .current_dir(&self.root)
            .output()
            .map_err(|e| CoreError::Message(format!("cannot run command: {e}")))?;
        Ok(json!({
            "status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-ondev-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    // Avoid a uuid dep in the daemon test: a process+counter-based unique name.
    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!(
            "{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[tokio::test]
    async fn write_then_read_and_escape_guard() {
        let root = temp();
        let reg = build_local_registry(&root);
        reg.call(
            "write_file",
            json!({ "path": "note.txt", "content": "hi device" }),
        )
        .await
        .expect("write");
        let read = reg
            .call("read_file", json!({ "path": "note.txt" }))
            .await
            .expect("read");
        assert_eq!(read["content"], json!("hi device"));
        assert!(reg
            .call("read_file", json!({ "path": "../escape" }))
            .await
            .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn run_command_captures_output() {
        let root = temp();
        let reg = build_local_registry(&root);
        let out = reg
            .call("run_command", json!({ "command": "echo fleety" }))
            .await
            .expect("run");
        assert!(out["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("fleety"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
