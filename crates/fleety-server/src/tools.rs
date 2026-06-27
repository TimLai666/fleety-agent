//! Server-only agent tools: core memory (ME/USER/TODO/TOOLS), the per-device
//! audit history, and the device registry. The workspace file/search/edit/run/
//! git tools live in the shared `fleety-tools` crate (so they also run on any
//! device via `fleetyd`) and are wired in here by `build_registry`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Agent-level core memory files the agent may read/update.
const MEMORY_FILES: &[&str] = &["ME.md", "USER.md", "TODO.md", "TOOLS.md"];

/// Build the tool registry: shared workspace tools (from `fleety-tools`, rooted
/// at `workspace`, backing up to `backups_dir`) plus the server-only memory,
/// history, device, and schedule tools. `device_tools` is the live map of
/// per-device advertised specs (filled in at Hello time) so `device_show` can
/// surface them.
#[allow(clippy::too_many_arguments)]
pub fn build_registry(
    workspace: &Path,
    backups_dir: &Path,
    memory_dir: &Path,
    history_path: &Path,
    devices_dir: &Path,
    schedules_dir: &Path,
    device_tools: crate::bridge::DeviceTools,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    fleety_tools::register_workspace(&mut registry, workspace, backups_dir);
    fleety_tools::register_insyra(&mut registry, workspace);
    registry.register(Box::new(MemoryRead {
        dir: memory_dir.to_path_buf(),
    }));
    registry.register(Box::new(MemoryWrite {
        dir: memory_dir.to_path_buf(),
    }));
    registry.register(Box::new(MemoryEdit {
        dir: memory_dir.to_path_buf(),
    }));
    registry.register(Box::new(HistoryList {
        path: history_path.to_path_buf(),
    }));
    registry.register(Box::new(DeviceList {
        devices_dir: devices_dir.to_path_buf(),
    }));
    registry.register(Box::new(DeviceShow {
        devices_dir: devices_dir.to_path_buf(),
        device_tools,
    }));
    crate::schedules::register(&mut registry, schedules_dir);
    registry
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

fn memory_path(dir: &Path, file: &str) -> Result<PathBuf> {
    if !MEMORY_FILES.contains(&file) {
        return Err(CoreError::Message(format!(
            "memory file must be one of {MEMORY_FILES:?}, got '{file}'"
        )));
    }
    Ok(dir.join(file))
}

struct MemoryRead {
    dir: PathBuf,
}

#[async_trait]
impl Tool for MemoryRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_read".to_string(),
            description: "Read an agent core memory file (ME.md, USER.md, TODO.md, or TOOLS.md). \
                 Returns raw `content` plus a line-numbered `numbered` view and `line_count`; pass \
                 `start_line`/`end_line` for a slice. Use the line numbers for memory_edit's \
                 line-range mode."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "enum": ["ME.md", "USER.md", "TODO.md", "TOOLS.md"] },
                    "start_line": { "type": "integer", "description": "first line to return (1-based)" },
                    "end_line": { "type": "integer", "description": "last line (1-based, inclusive)" }
                },
                "required": ["file"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let file = require_str(&args, "file")?;
        let start_line = args
            .get("start_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let end_line = args
            .get("end_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let path = memory_path(&self.dir, file)?;
        let full = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(CoreError::Message(format!("cannot read {file}: {e}"))),
        };
        let (slice, start, end, total) = fleety_tools::slice_lines(&full, start_line, end_line);
        Ok(json!({
            "file": file,
            "content": slice,
            "numbered": fleety_tools::line_numbered(&slice, start.max(1)),
            "start_line": start,
            "end_line": end,
            "line_count": total,
        }))
    }
}

struct MemoryWrite {
    dir: PathBuf,
}

#[async_trait]
impl Tool for MemoryWrite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_write".to_string(),
            description: "Update an agent core memory file (ME.md/USER.md/TODO.md/TOOLS.md); mode 'replace' (default) or 'append'.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "enum": ["ME.md", "USER.md", "TODO.md", "TOOLS.md"] },
                    "content": { "type": "string" },
                    "mode": { "type": "string", "enum": ["replace", "append"] }
                },
                "required": ["file", "content"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let file = require_str(&args, "file")?;
        let content = require_str(&args, "content")?;
        let mode = args
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("replace");
        let path = memory_path(&self.dir, file)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create memory dir: {e}")))?;
        }
        match mode {
            "append" => {
                use std::io::Write;
                let mut handle = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(|e| CoreError::Message(format!("cannot open {file}: {e}")))?;
                writeln!(handle, "{content}")
                    .map_err(|e| CoreError::Message(format!("append {file} failed: {e}")))?;
            }
            "replace" => {
                std::fs::write(&path, content)
                    .map_err(|e| CoreError::Message(format!("write {file} failed: {e}")))?;
            }
            other => {
                return Err(CoreError::Message(format!(
                    "mode must be 'replace' or 'append', got '{other}'"
                )))
            }
        }
        Ok(json!({ "file": file, "mode": mode, "bytes": content.len() }))
    }
}

struct MemoryEdit {
    dir: PathBuf,
}

#[async_trait]
impl Tool for MemoryEdit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_edit".to_string(),
            description:
                "Edit a core memory file (ME.md/USER.md/TODO.md/TOOLS.md) precisely — the \
                 alternative to memory_write's full rewrite. Two modes: (1) substring — replace \
                 `old` with `new` (`old` unique unless replace_all:true); (2) line-range — replace \
                 lines `start_line`..`end_line` (1-based, from memory_read) with `new` (empty `new` \
                 deletes them). Returns an `applied` line-numbered view of the change."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "enum": ["ME.md", "USER.md", "TODO.md", "TOOLS.md"] },
                    "old": { "type": "string", "description": "substring mode: exact text to replace" },
                    "new": { "type": "string" },
                    "replace_all": { "type": "boolean", "description": "substring mode: replace every occurrence (default false)" },
                    "start_line": { "type": "integer", "description": "line-range mode: first line (1-based)" },
                    "end_line": { "type": "integer", "description": "line-range mode: last line (1-based, inclusive)" }
                },
                "required": ["file", "new"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let file = require_str(&args, "file")?;
        let new = require_str(&args, "new")?;
        let path = memory_path(&self.dir, file)?;
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::Message(format!(
                    "{file} is empty; nothing to edit — use memory_write to create it first"
                )))
            }
            Err(e) => return Err(CoreError::Message(format!("cannot read {file}: {e}"))),
        };

        let start_arg = args
            .get("start_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let (updated, replaced, change_start, change_len) = if let Some(start) = start_arg {
            // Line-range mode.
            let end = args
                .get("end_line")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(start);
            let (updated, inserted) = fleety_tools::replace_line_range(&content, start, end, new)?;
            (updated, 1usize, start, inserted)
        } else {
            // Substring mode.
            let old = require_str(&args, "old")?;
            if old.is_empty() {
                return Err(CoreError::Message(
                    "provide 'old' (substring mode) or 'start_line'/'end_line' (line-range mode)"
                        .to_string(),
                ));
            }
            let replace_all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let count = content.matches(old).count();
            if count == 0 {
                return Err(CoreError::Message(format!(
                    "the 'old' text was not found in {file}; read it with memory_read and copy the exact text"
                )));
            }
            if count > 1 && !replace_all {
                return Err(CoreError::Message(format!(
                    "the 'old' text appears {count} times in {file}; add surrounding context to make it unique, or set replace_all:true"
                )));
            }
            let pos = content.find(old).unwrap_or(0);
            let (updated, replaced) = if replace_all {
                (content.replace(old, new), count)
            } else {
                (content.replacen(old, new, 1), 1)
            };
            (
                updated,
                replaced,
                fleety_tools::line_of_offset(&content, pos),
                new.lines().count().max(1),
            )
        };

        std::fs::write(&path, &updated)
            .map_err(|e| CoreError::Message(format!("write {file} failed: {e}")))?;
        Ok(json!({
            "file": file,
            "replaced": replaced,
            "applied": fleety_tools::region_view(&updated, change_start, change_len, 3),
            "line_count": updated.lines().count(),
        }))
    }
}

struct HistoryList {
    path: PathBuf,
}

#[async_trait]
impl Tool for HistoryList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "history_list".to_string(),
            description:
                "List recent audit-log entries (tool calls, results, replies) for this device."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "max entries (default 20)" } }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(CoreError::Message(format!("cannot read history: {e}"))),
        };
        let mut entries: Vec<Value> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let total = entries.len();
        if entries.len() > limit {
            entries = entries.split_off(entries.len() - limit);
        }
        Ok(json!({ "total": total, "returned": entries.len(), "entries": entries }))
    }
}

struct DeviceList {
    devices_dir: PathBuf,
}

#[async_trait]
impl Tool for DeviceList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_list".to_string(),
            description: "List registered devices and their records.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let mut devices = Vec::new();
        match std::fs::read_dir(&self.devices_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry =
                        entry.map_err(|e| CoreError::Message(format!("dir entry error: {e}")))?;
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let record_path = entry.path().join("device.json");
                        if let Ok(text) = std::fs::read_to_string(&record_path) {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                devices.push(value);
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CoreError::Message(format!("cannot list devices: {e}"))),
        }
        Ok(json!({ "devices": devices }))
    }
}

struct DeviceShow {
    devices_dir: PathBuf,
    device_tools: crate::bridge::DeviceTools,
}

#[async_trait]
impl Tool for DeviceShow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_show".to_string(),
            description:
                "Show one device's record, NOTES, and the on-device tools it advertised when it \
                 last connected (so the agent knows what `device_exec` can call there)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "device": { "type": "string" } },
                "required": ["device"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        if device.contains('/') || device.contains('\\') || device.contains("..") {
            return Err(CoreError::Message(format!("invalid device id '{device}'")));
        }
        let dir = self.devices_dir.join(device);
        let record: Value = match std::fs::read_to_string(dir.join("device.json")) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| CoreError::Message(format!("corrupt device record: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::Message(format!(
                    "no such device '{device}'; use device_list to see registered devices"
                )))
            }
            Err(e) => {
                return Err(CoreError::Message(format!(
                    "cannot read device record: {e}"
                )))
            }
        };
        let notes = std::fs::read_to_string(dir.join("NOTES.md")).unwrap_or_default();
        // Pull the advertised tool list (if any) for this device — empty when
        // the device is offline or didn't advertise (e.g. an interactive CLI).
        let advertised_tools = self
            .device_tools
            .lock()
            .await
            .get(device)
            .cloned()
            .unwrap_or_default();
        Ok(json!({
            "record": record,
            "notes": notes,
            "advertised_tools": advertised_tools,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-srvtools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    // Workspace tools come from fleety-tools; these confirm the wiring works.
    #[tokio::test]
    async fn list_dir_and_escape_via_registry() {
        // Pin the confined sandbox for the escape assertion (default is whole-disk).
        std::env::set_var("FLEETY_FS_SCOPE", "workspace");
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(
            &root,
            &root,
            &root,
            &root,
            &root,
            &root,
            crate::bridge::new_device_tools(),
        );
        assert!(registry
            .call("list_dir", json!({ "path": "." }))
            .await
            .expect("list")
            .get("entries")
            .is_some());
        assert!(registry
            .call("read_file", json!({ "path": "../../../../etc/passwd" }))
            .await
            .is_err());
        assert!(registry
            .call("read_file", json!({}))
            .await
            .expect_err("needs path")
            .report()
            .message
            .contains("path"));
    }

    #[tokio::test]
    async fn write_edit_search_run_via_registry() {
        let ws = temp_dir();
        let backups = temp_dir();
        std::fs::write(ws.join("a.txt"), "hello\nfoo bar\n").expect("seed");
        let registry = build_registry(
            &ws,
            &backups,
            &ws,
            &ws.join("h.jsonl"),
            &ws,
            &ws,
            crate::bridge::new_device_tools(),
        );

        let edited = registry
            .call(
                "edit_file",
                json!({ "path": "a.txt", "old": "foo bar", "new": "FOO" }),
            )
            .await
            .expect("edit");
        assert!(edited["diff"].as_str().unwrap_or_default().contains("+FOO"));

        let found = registry
            .call("search_files", json!({ "query": "FOO" }))
            .await
            .expect("search");
        assert_eq!(found["matches"].as_array().map(Vec::len).unwrap_or(0), 1);

        let ran = registry
            .call("run_command", json!({ "command": "echo hi" }))
            .await
            .expect("run");
        assert_eq!(ran["exit_code"], json!(0));
        assert!(registry
            .call("run_command", json!({ "command": "rm -rf /" }))
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&backups);
    }

    #[tokio::test]
    async fn memory_write_then_read_and_reject_unknown() {
        let dir = temp_dir();
        let registry = build_registry(
            &dir,
            &dir,
            &dir,
            &dir,
            &dir,
            &dir,
            crate::bridge::new_device_tools(),
        );

        registry
            .call(
                "memory_write",
                json!({ "file": "USER.md", "content": "likes Rust" }),
            )
            .await
            .expect("write");
        let read = registry
            .call("memory_read", json!({ "file": "USER.md" }))
            .await
            .expect("read");
        assert_eq!(read["content"], json!("likes Rust"));

        assert!(registry
            .call(
                "memory_write",
                json!({ "file": "secrets.md", "content": "x" })
            )
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn memory_edit_fragment_and_guards() {
        let dir = temp_dir();
        let registry = build_registry(
            &dir,
            &dir,
            &dir,
            &dir,
            &dir,
            &dir,
            crate::bridge::new_device_tools(),
        );

        registry
            .call(
                "memory_write",
                json!({ "file": "TODO.md", "content": "- buy milk\n- buy milk\n- call Tim\n" }),
            )
            .await
            .expect("seed");

        // Ambiguous single-match edit is rejected (two "buy milk" lines).
        assert!(registry
            .call(
                "memory_edit",
                json!({ "file": "TODO.md", "old": "buy milk", "new": "buy oat milk" })
            )
            .await
            .is_err());

        // replace_all rewrites every occurrence.
        let all = registry
            .call(
                "memory_edit",
                json!({ "file": "TODO.md", "old": "buy milk", "new": "buy oat milk", "replace_all": true }),
            )
            .await
            .expect("replace_all");
        assert_eq!(all["replaced"], json!(2));

        // A unique fragment edits in place; the result carries the numbered region.
        let edited = registry
            .call(
                "memory_edit",
                json!({ "file": "TODO.md", "old": "call Tim", "new": "call Tim about release" }),
            )
            .await
            .expect("unique edit");
        assert!(edited["applied"]
            .as_str()
            .unwrap_or_default()
            .contains("call Tim about release"));
        let read = registry
            .call("memory_read", json!({ "file": "TODO.md" }))
            .await
            .expect("read");
        let body = read["content"].as_str().unwrap_or_default();
        assert!(body.contains("buy oat milk"));
        assert!(body.contains("call Tim about release"));
        assert!(read["numbered"].as_str().unwrap_or_default().contains('\t'));

        // Line-range mode: replace line 3 (the "call Tim" line).
        registry
            .call(
                "memory_edit",
                json!({ "file": "TODO.md", "start_line": 3, "end_line": 3, "new": "- done" }),
            )
            .await
            .expect("line edit");
        let after = registry
            .call(
                "memory_read",
                json!({ "file": "TODO.md", "start_line": 3, "end_line": 3 }),
            )
            .await
            .expect("read slice");
        assert_eq!(after["content"], json!("- done"));

        // Missing 'old' text errors; editing an empty file errors.
        assert!(registry
            .call(
                "memory_edit",
                json!({ "file": "TODO.md", "old": "nonexistent", "new": "x" })
            )
            .await
            .is_err());
        assert!(registry
            .call(
                "memory_edit",
                json!({ "file": "ME.md", "old": "anything", "new": "x" })
            )
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
