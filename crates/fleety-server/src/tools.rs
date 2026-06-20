//! Workspace tools the agent can call. M3a: read-only inspection
//! (`read_file`, `list_dir`, `git_status`, `git_diff`). Mutating tools
//! (write/patch/run) with audit + rollback + critical gate land next.
//!
//! v0 uses synchronous `std::fs` / `std::process` for simplicity; these run on
//! the server host within a fixed workspace root, with a path-escape guard.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Agent-level core memory files the agent may read/update.
const MEMORY_FILES: &[&str] = &["ME.md", "USER.md", "TODO.md", "TOOLS.md"];

/// Build the workspace tool registry rooted at `workspace`. Mutating tools back
/// up to `backups_dir` (outside the workspace) before changing files;
/// `memory_dir` holds the agent-level core memory files.
pub fn build_registry(
    workspace: &Path,
    backups_dir: &Path,
    memory_dir: &Path,
    history_path: &Path,
    devices_dir: &Path,
    schedules_dir: &Path,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFile {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(ListDir {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(GitStatus {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(GitDiff {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(GitLog {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(GitShow {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(WriteFile {
        root: workspace.to_path_buf(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(EditFile {
        root: workspace.to_path_buf(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(RunCommand {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(SearchFiles {
        root: workspace.to_path_buf(),
    }));
    registry.register(Box::new(MemoryRead {
        dir: memory_dir.to_path_buf(),
    }));
    registry.register(Box::new(MemoryWrite {
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
    }));
    crate::schedules::register(&mut registry, schedules_dir);
    registry
}

fn memory_path(dir: &Path, file: &str) -> Result<PathBuf> {
    if !MEMORY_FILES.contains(&file) {
        return Err(CoreError::Message(format!(
            "memory file must be one of {MEMORY_FILES:?}, got '{file}'"
        )));
    }
    Ok(dir.join(file))
}

/// Resolve a path for writing: the parent must exist and stay within the root,
/// but the target file itself may be new.
fn resolve_for_write(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("workspace root unavailable: {e}")))?;
    let target = canon_root.join(rel);
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::Message(format!("invalid path '{rel}'")))?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("parent directory of '{rel}' not found: {e}")))?;
    if !canon_parent.starts_with(&canon_root) {
        return Err(CoreError::Message(format!(
            "path '{rel}' escapes the workspace"
        )));
    }
    let file_name = target
        .file_name()
        .ok_or_else(|| CoreError::Message(format!("path '{rel}' has no file name")))?;
    Ok(canon_parent.join(file_name))
}

/// Copy an existing file into the backups store (outside the workspace) and
/// return a `{id, path}` handle for rollback.
fn backup_existing(backups: &Path, rel: &str, resolved: &Path) -> Result<Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let backup_path = backups.join(&id).join(rel);
    if let Some(parent) = backup_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create backup dir: {e}")))?;
    }
    std::fs::copy(resolved, &backup_path)
        .map_err(|e| CoreError::Message(format!("backup of '{rel}' failed: {e}")))?;
    Ok(json!({ "id": id, "path": backup_path.display().to_string() }))
}

/// v0 critical-command guard (deliberately permissive: only clearly
/// irreversible commands are refused). A real semantic classifier comes later.
fn critical_reason(command: &str) -> Option<&'static str> {
    let c = command.to_lowercase();
    const PATTERNS: &[(&str, &str)] = &[
        ("rm -rf /", "recursive delete of the filesystem root"),
        ("rm -rf /*", "recursive delete of the filesystem root"),
        ("mkfs", "formatting a filesystem"),
        ("dd if=", "raw disk write"),
        (":(){", "fork bomb"),
        ("shutdown", "shutting down the host"),
        ("reboot", "rebooting the host"),
        ("format ", "formatting a disk"),
        ("del /f /s /q", "mass file deletion"),
        ("rd /s", "recursive directory deletion"),
        ("diskpart", "disk partitioning"),
    ];
    PATTERNS
        .iter()
        .find(|(p, _)| c.contains(p))
        .map(|(_, why)| *why)
}

/// Resolve `rel` against `root`, refusing paths that escape the workspace.
fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("workspace root unavailable: {e}")))?;
    let resolved = canon_root
        .join(rel)
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("path '{rel}' not found: {e}")))?;
    if !resolved.starts_with(&canon_root) {
        return Err(CoreError::Message(format!(
            "path '{rel}' escapes the workspace; use a path inside the workspace"
        )));
    }
    Ok(resolved)
}

/// Recursively search files under `dir` for lines containing `query`, pushing
/// matches (relative to `root`) into `out`, bounded by `max`.
fn search_dir(dir: &Path, root: &Path, query: &str, max: usize, out: &mut Vec<Value>) {
    if out.len() >= max {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= max {
            return;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | "target" | "node_modules" | ".fleety-backups"
            ) {
                continue;
            }
            search_dir(&path, root, query, max, out);
        } else if let Ok(content) = std::fs::read_to_string(&path) {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    out.push(json!({ "file": rel, "line": index + 1, "text": line.trim() }));
                    if out.len() >= max {
                        return;
                    }
                }
            }
        }
    }
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

fn run_git(root: &Path, git_args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(git_args)
        .output()
        .map_err(|e| CoreError::Message(format!("cannot run git: {e}; is git installed?")))?;
    if !output.status.success() {
        return Err(CoreError::Message(format!(
            "git {}: {}",
            git_args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct ReadFile {
    root: PathBuf,
}

#[async_trait]
impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a UTF-8 text file within the workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "workspace-relative path" } },
                "required": ["path"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let resolved = resolve_in_root(&self.root, path)?;
        let content = std::fs::read_to_string(&resolved)
            .map_err(|e| CoreError::Message(format!("cannot read '{path}': {e}")))?;
        Ok(json!({ "path": path, "content": content }))
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
            description: "List entries of a directory within the workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "workspace-relative dir (default '.')" } }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let resolved = resolve_in_root(&self.root, path)?;
        let mut entries = Vec::new();
        let read = std::fs::read_dir(&resolved)
            .map_err(|e| CoreError::Message(format!("cannot list '{path}': {e}")))?;
        for entry in read {
            let entry = entry.map_err(|e| CoreError::Message(format!("dir entry error: {e}")))?;
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            entries.push(json!({ "name": entry.file_name().to_string_lossy(), "is_dir": is_dir }));
        }
        Ok(json!({ "path": path, "entries": entries }))
    }
}

struct GitStatus {
    root: PathBuf,
}

#[async_trait]
impl Tool for GitStatus {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_status".to_string(),
            description: "Show `git status --porcelain` for the workspace.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let status = run_git(&self.root, &["status", "--porcelain"])?;
        Ok(json!({ "status": status }))
    }
}

struct GitDiff {
    root: PathBuf,
}

#[async_trait]
impl Tool for GitDiff {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_diff".to_string(),
            description: "Show the unstaged `git diff` for the workspace.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let diff = run_git(&self.root, &["diff"])?;
        Ok(json!({ "diff": diff }))
    }
}

struct GitLog {
    root: PathBuf,
}

#[async_trait]
impl Tool for GitLog {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_log".to_string(),
            description: "Show recent commits (`git log --oneline`).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "max commits (default 20)" } }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20);
        let log = run_git(&self.root, &["log", "--oneline", "-n", &limit.to_string()])?;
        Ok(json!({ "log": log }))
    }
}

struct GitShow {
    root: PathBuf,
}

#[async_trait]
impl Tool for GitShow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "git_show".to_string(),
            description: "Show a commit or object (`git show <ref>`).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "ref": { "type": "string", "description": "commit/ref (default HEAD)" } }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let reference = args.get("ref").and_then(Value::as_str).unwrap_or("HEAD");
        let show = run_git(&self.root, &["show", "--stat", reference])?;
        Ok(json!({ "show": show }))
    }
}

struct WriteFile {
    root: PathBuf,
    backups: PathBuf,
}

#[async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),
            description: "Write a UTF-8 text file within the workspace (its parent directory must exist). The previous content, if any, is backed up for rollback.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "workspace-relative path" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let content = require_str(&args, "content")?;
        let resolved = resolve_for_write(&self.root, path)?;

        // Back up existing content (outside the workspace) before overwriting.
        let mut backup = Value::Null;
        if resolved.exists() {
            let id = uuid::Uuid::new_v4().to_string();
            let backup_path = self.backups.join(&id).join(path);
            if let Some(parent) = backup_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::Message(format!("cannot create backup dir: {e}")))?;
            }
            std::fs::copy(&resolved, &backup_path)
                .map_err(|e| CoreError::Message(format!("backup of '{path}' failed: {e}")))?;
            backup = json!({ "id": id, "path": backup_path.display().to_string() });
        }

        std::fs::write(&resolved, content)
            .map_err(|e| CoreError::Message(format!("cannot write '{path}': {e}")))?;
        Ok(json!({ "path": path, "bytes_written": content.len(), "backup": backup }))
    }
}

struct EditFile {
    root: PathBuf,
    backups: PathBuf,
}

#[async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".to_string(),
            description: "Replace an exact, unique substring in a workspace file (precise edit). The prior content is backed up for rollback.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string", "description": "exact text to replace (must be unique in the file)" },
                    "new": { "type": "string" }
                },
                "required": ["path", "old", "new"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let old = require_str(&args, "old")?;
        let new = require_str(&args, "new")?;
        let resolved = resolve_in_root(&self.root, path)?;
        let content = std::fs::read_to_string(&resolved)
            .map_err(|e| CoreError::Message(format!("cannot read '{path}': {e}")))?;
        let count = content.matches(old).count();
        if count == 0 {
            return Err(CoreError::Message(format!(
                "the 'old' text was not found in '{path}'; read the file and copy the exact text"
            )));
        }
        if count > 1 {
            return Err(CoreError::Message(format!(
                "the 'old' text appears {count} times in '{path}'; include more surrounding context to make it unique"
            )));
        }
        let backup = backup_existing(&self.backups, path, &resolved)?;
        let updated = content.replacen(old, new, 1);
        std::fs::write(&resolved, &updated)
            .map_err(|e| CoreError::Message(format!("cannot write '{path}': {e}")))?;
        Ok(json!({ "path": path, "replaced": 1, "backup": backup }))
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
            description: "Run a shell command in the workspace and capture stdout/stderr/exit code. Clearly destructive commands are refused.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string", "description": "workspace-relative working dir (default workspace root)" }
                },
                "required": ["command"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let command = require_str(&args, "command")?;
        if let Some(reason) = critical_reason(command) {
            return Err(CoreError::Message(format!(
                "refused critical command ({reason}): '{command}'. Irreversible actions need explicit user confirmation, which is not available here; do not retry this command."
            )));
        }
        let cwd = match args.get("cwd").and_then(Value::as_str) {
            Some(rel) => resolve_in_root(&self.root, rel)?,
            None => self.root.clone(),
        };
        let (shell, flag) = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let output = Command::new(shell)
            .arg(flag)
            .arg(command)
            .current_dir(&cwd)
            .output()
            .map_err(|e| CoreError::Message(format!("cannot run command: {e}")))?;
        Ok(json!({
            "exit_code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }
}

struct SearchFiles {
    root: PathBuf,
}

#[async_trait]
impl Tool for SearchFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_files".to_string(),
            description: "Search file contents in the workspace for a substring; returns file/line/text matches.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "path": { "type": "string", "description": "workspace-relative subdir (default whole workspace)" },
                    "max_results": { "type": "integer", "description": "default 100" }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let query = require_str(&args, "query")?;
        if query.is_empty() {
            return Err(CoreError::Message(
                "search query must not be empty".to_string(),
            ));
        }
        let canon_root = self
            .root
            .canonicalize()
            .map_err(|e| CoreError::Message(format!("workspace root unavailable: {e}")))?;
        let base = match args.get("path").and_then(Value::as_str) {
            Some(path) => resolve_in_root(&self.root, path)?,
            None => canon_root.clone(),
        };
        let max = args
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(100) as usize;
        let mut matches = Vec::new();
        search_dir(&base, &canon_root, query, max, &mut matches);
        let truncated = matches.len() >= max;
        Ok(json!({ "matches": matches, "truncated": truncated }))
    }
}

struct MemoryRead {
    dir: PathBuf,
}

#[async_trait]
impl Tool for MemoryRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_read".to_string(),
            description: "Read an agent core memory file (ME.md, USER.md, TODO.md, or TOOLS.md)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "file": { "type": "string", "enum": ["ME.md", "USER.md", "TODO.md", "TOOLS.md"] } },
                "required": ["file"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let file = require_str(&args, "file")?;
        let path = memory_path(&self.dir, file)?;
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(CoreError::Message(format!("cannot read {file}: {e}"))),
        };
        Ok(json!({ "file": file, "content": content }))
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
}

#[async_trait]
impl Tool for DeviceShow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_show".to_string(),
            description: "Show one device's record and NOTES.".to_string(),
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
        Ok(json!({ "record": record, "notes": notes }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_and_list_within_root() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root, &root, &root, &root, &root, &root);
        // Cargo.toml exists at the repo/workspace root in tests run from a crate dir's parent;
        // use list_dir on "." which always resolves.
        let listed = registry
            .call("list_dir", json!({ "path": "." }))
            .await
            .expect("list");
        assert!(listed.get("entries").is_some());
    }

    #[tokio::test]
    async fn path_escape_is_rejected() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root, &root, &root, &root, &root, &root);
        let result = registry
            .call("read_file", json!({ "path": "../../../../etc/passwd" }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_arg_is_actionable() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root, &root, &root, &root, &root, &root);
        let err = registry
            .call("read_file", json!({}))
            .await
            .expect_err("should require path");
        assert!(err.report().message.contains("path"));
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    #[tokio::test]
    async fn write_file_creates_and_backs_up() {
        let ws = temp_dir();
        let backups = temp_dir();
        let registry = build_registry(&ws, &backups, &ws, &ws, &ws, &ws);

        let created = registry
            .call("write_file", json!({ "path": "a.txt", "content": "one" }))
            .await
            .expect("write1");
        assert_eq!(created["backup"], Value::Null);
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).expect("read"),
            "one"
        );

        let overwritten = registry
            .call("write_file", json!({ "path": "a.txt", "content": "two" }))
            .await
            .expect("write2");
        assert!(overwritten["backup"]["id"].is_string());
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).expect("read"),
            "two"
        );

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&backups);
    }

    #[tokio::test]
    async fn run_command_captures_output() {
        let ws = temp_dir();
        let registry = build_registry(&ws, &ws, &ws, &ws, &ws, &ws);
        let result = registry
            .call("run_command", json!({ "command": "echo hello" }))
            .await
            .expect("run");
        assert_eq!(result["exit_code"], json!(0));
        assert!(result["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("hello"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn critical_command_refused() {
        let ws = temp_dir();
        let registry = build_registry(&ws, &ws, &ws, &ws, &ws, &ws);
        let err = registry
            .call("run_command", json!({ "command": "rm -rf /" }))
            .await
            .expect_err("should refuse");
        assert!(err.report().message.contains("refused"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn memory_write_then_read_and_reject_unknown() {
        let dir = temp_dir();
        let registry = build_registry(&dir, &dir, &dir, &dir, &dir, &dir);

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

        let bad = registry
            .call(
                "memory_write",
                json!({ "file": "secrets.md", "content": "x" }),
            )
            .await;
        assert!(bad.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn edit_file_replaces_unique_and_rejects_ambiguous() {
        let ws = temp_dir();
        let backups = temp_dir();
        std::fs::write(ws.join("c.txt"), "alpha\nbeta\nalpha\n").expect("write");
        let registry = build_registry(&ws, &backups, &ws, &ws.join("h.jsonl"), &ws, &ws);

        // "beta" is unique -> replaced
        let ok = registry
            .call(
                "edit_file",
                json!({ "path": "c.txt", "old": "beta", "new": "BETA" }),
            )
            .await
            .expect("edit");
        assert_eq!(ok["replaced"], json!(1));
        assert!(std::fs::read_to_string(ws.join("c.txt"))
            .expect("read")
            .contains("BETA"));

        // "alpha" appears twice -> rejected
        let ambiguous = registry
            .call(
                "edit_file",
                json!({ "path": "c.txt", "old": "alpha", "new": "A" }),
            )
            .await;
        assert!(ambiguous.is_err());

        let _ = std::fs::remove_dir_all(&ws);
        let _ = std::fs::remove_dir_all(&backups);
    }

    #[tokio::test]
    async fn search_files_finds_matches() {
        let ws = temp_dir();
        std::fs::write(ws.join("a.txt"), "hello world\nfoo bar\n").expect("write");
        let history = ws.join("h.jsonl");
        let registry = build_registry(&ws, &ws, &ws, &history, &ws, &ws);
        let result = registry
            .call("search_files", json!({ "query": "foo" }))
            .await
            .expect("search");
        let matches = result["matches"].as_array().expect("matches array");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["line"], json!(2));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn git_log_and_show_on_real_repo() {
        let ws = temp_dir();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&ws)
                .args(args)
                .output()
        };
        let _ = git(&["init"]);
        let _ = git(&["config", "user.email", "t@example.com"]);
        let _ = git(&["config", "user.name", "tester"]);
        std::fs::write(ws.join("f.txt"), "hi").expect("write");
        let _ = git(&["add", "."]);
        let committed = git(&["commit", "-m", "first commit"])
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !committed {
            // git unavailable in this environment; skip.
            let _ = std::fs::remove_dir_all(&ws);
            return;
        }
        let registry = build_registry(&ws, &ws, &ws, &ws.join("h.jsonl"), &ws, &ws);

        let log = registry.call("git_log", json!({})).await.expect("log");
        assert!(log["log"]
            .as_str()
            .unwrap_or_default()
            .contains("first commit"));
        let show = registry.call("git_show", json!({})).await.expect("show");
        assert!(show["show"]
            .as_str()
            .unwrap_or_default()
            .contains("first commit"));

        let _ = std::fs::remove_dir_all(&ws);
    }

    #[tokio::test]
    async fn device_list_and_show() {
        let devices = temp_dir();
        let pi = devices.join("pi");
        std::fs::create_dir_all(&pi).expect("mkdir");
        std::fs::write(pi.join("device.json"), r#"{"id":"pi","status":"active"}"#).expect("write");
        let history = devices.join("h.jsonl");
        let registry = build_registry(&devices, &devices, &devices, &history, &devices, &devices);

        let list = registry.call("device_list", json!({})).await.expect("list");
        assert_eq!(list["devices"].as_array().map(Vec::len).unwrap_or(0), 1);

        let show = registry
            .call("device_show", json!({ "device": "pi" }))
            .await
            .expect("show");
        assert_eq!(show["record"]["id"], json!("pi"));

        let escape = registry
            .call("device_show", json!({ "device": "../secret" }))
            .await;
        assert!(escape.is_err());

        let _ = std::fs::remove_dir_all(&devices);
    }
}
