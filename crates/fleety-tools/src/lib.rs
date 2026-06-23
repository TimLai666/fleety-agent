//! Shared workspace tools, root-relative so the same implementations run on the
//! server's workspace and on any device via `fleetyd` (no drift between them).
//!
//! `register_workspace(registry, root, backups_dir)` adds: `read_file`,
//! `list_dir`, `search_files` (ripgrep engine), `write_file` + `edit_file`
//! (backup + unified diff), `delete_file` / `move_file` / `make_dir`,
//! `rollback` (restore a backup), `run_command` (critical-command guard; can
//! `track` paths to diff what it changed), and `git_status`/`git_diff` (incl.
//! untracked)/`git_log`/`git_show`. All paths are confined to `root` with a
//! path-escape guard; mutations back up to `backups_dir`.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

mod insyra;
pub use insyra::register_insyra;

/// Register the workspace tools rooted at `root`; mutations back up to `backups_dir`.
pub fn register_workspace(registry: &mut ToolRegistry, root: &Path, backups_dir: &Path) {
    let r = || root.to_path_buf();
    registry.register(Box::new(ReadFile { root: r() }));
    registry.register(Box::new(ListDir { root: r() }));
    registry.register(Box::new(SearchFiles { root: r() }));
    registry.register(Box::new(WriteFile {
        root: r(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(EditFile {
        root: r(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(RunCommand { root: r() }));
    registry.register(Box::new(DeleteFile {
        root: r(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(MoveFile {
        root: r(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(MakeDir { root: r() }));
    registry.register(Box::new(Rollback {
        root: r(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(GitStatus { root: r() }));
    registry.register(Box::new(GitDiff { root: r() }));
    registry.register(Box::new(GitLog { root: r() }));
    registry.register(Box::new(GitShow { root: r() }));
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Resolve `rel` against `root`, refusing paths that escape it.
fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("root unavailable: {e}")))?;
    let resolved = canon_root
        .join(rel)
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("path '{rel}' not found: {e}")))?;
    if !resolved.starts_with(&canon_root) {
        return Err(CoreError::Message(format!(
            "path '{rel}' escapes the workspace; use a path inside it"
        )));
    }
    Ok(resolved)
}

/// Resolve a path for writing: parent must exist and stay within `root`, the
/// leaf may be new, and the leaf must not be a symlink.
fn resolve_for_write(root: &Path, rel: &str) -> Result<PathBuf> {
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("root unavailable: {e}")))?;
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
    let resolved = canon_parent.join(file_name);
    if resolved
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(CoreError::Message(format!(
            "refusing to write through symlink '{rel}'"
        )));
    }
    Ok(resolved)
}

/// Resolve a workspace-relative path lexically (no `..`, not absolute) without
/// requiring it to exist — for paths a tool may create or that aren't there yet.
fn resolve_lenient(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || Path::new(rel).is_absolute() || rel.split(['/', '\\']).any(|c| c == "..") {
        return Err(CoreError::Message(format!(
            "path '{rel}' must be a relative path inside the workspace (no '..')"
        )));
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| CoreError::Message(format!("root unavailable: {e}")))?;
    Ok(canon_root.join(rel))
}

/// Find the first file under `dir` (recursively); used to locate a backup's content.
fn first_file(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_file() {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = first_file(&path) {
                return Some(found);
            }
        }
    }
    None
}

/// Walk the backups store and return a summary entry per backup id:
/// `{ id, original_rel_path, ts_secs }`. Best-effort: an unreadable backup is
/// skipped, not fatal. The list is sorted newest-first by `ts_secs`.
pub fn list_backups(backups: &Path) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(backups) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(CoreError::Message(format!("read backups dir: {e}"))),
    };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(file) = first_file(&dir) else {
            continue;
        };
        let rel = file
            .strip_prefix(&dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let ts_secs = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(json!({
            "id": id,
            "original_rel_path": rel,
            "ts_secs": ts_secs,
        }));
    }
    out.sort_by(|a, b| {
        b.get("ts_secs")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .cmp(&a.get("ts_secs").and_then(Value::as_u64).unwrap_or(0))
    });
    Ok(out)
}

/// Restore a backup into `root`. Used by the `rollback` tool and by the
/// server's CLI-facing rollback handler. Returns `{ restored: <rel>,
/// backup_id: <id> }` on success. Validates the id (no path traversal) and
/// gracefully errors if the backup is missing or corrupt.
pub fn apply_backup(root: &Path, backups: &Path, backup_id: &str) -> Result<Value> {
    if backup_id.is_empty()
        || backup_id.contains('/')
        || backup_id.contains('\\')
        || backup_id.contains("..")
    {
        return Err(CoreError::Message(format!(
            "invalid backup id '{backup_id}'"
        )));
    }
    let backup_root = backups.join(backup_id);
    let backup_file = first_file(&backup_root)
        .ok_or_else(|| CoreError::Message(format!("no backup '{backup_id}'")))?;
    let rel = backup_file
        .strip_prefix(&backup_root)
        .map_err(|e| CoreError::Message(format!("corrupt backup '{backup_id}': {e}")))?
        .to_string_lossy()
        .replace('\\', "/");
    let dest = resolve_lenient(root, &rel)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot recreate '{rel}' parent: {e}")))?;
    }
    std::fs::copy(&backup_file, &dest)
        .map_err(|e| CoreError::Message(format!("restore '{rel}' failed: {e}")))?;
    Ok(json!({ "restored": rel, "backup_id": backup_id }))
}

/// Copy an existing file into the backups store and return a `{id, path}` handle.
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

/// A unified diff of a single file's change (works on any device, not just git).
fn unified_diff(old: &str, new: &str, path: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// Critical-command guard: refuses clearly irreversible commands. Conservative
/// (ordinary `rm -rf ./build` is allowed); whitespace is normalized first.
fn critical_reason(command: &str) -> Option<&'static str> {
    let norm = command
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(reason) = catastrophic_delete(&norm) {
        return Some(reason);
    }
    const PATTERNS: &[(&str, &str)] = &[
        ("mkfs", "formatting a filesystem"),
        ("dd if=", "raw disk read/write"),
        ("of=/dev/", "raw write to a block device"),
        ("> /dev/sd", "overwriting a block device"),
        ("> /dev/nvme", "overwriting a block device"),
        ("wipefs", "wiping filesystem signatures"),
        ("shred ", "secure-erasing data"),
        ("fdisk", "disk partitioning"),
        ("parted", "disk partitioning"),
        (":(){", "fork bomb"),
        ("shutdown", "shutting down the host"),
        ("reboot", "rebooting the host"),
        ("poweroff", "powering off the host"),
        ("init 0", "shutting down the host"),
        ("init 6", "rebooting the host"),
        ("format ", "formatting a disk"),
        ("del /f /s /q", "mass file deletion"),
        ("del /s", "recursive file deletion"),
        ("rd /s", "recursive directory deletion"),
        ("rmdir /s", "recursive directory deletion"),
        ("diskpart", "disk partitioning"),
        ("format-volume", "formatting a volume"),
        ("clear-disk", "wiping a disk"),
        ("cipher /w", "wiping free disk space"),
    ];
    PATTERNS
        .iter()
        .find(|(p, _)| norm.contains(p))
        .map(|(_, why)| *why)
}

fn catastrophic_delete(norm: &str) -> Option<&'static str> {
    let is_rm = norm.starts_with("rm ")
        || norm.contains("; rm ")
        || norm.contains("&& rm ")
        || norm.contains("sudo rm ");
    if !is_rm {
        return None;
    }
    let recursive = norm.contains(" -r") || norm.contains("--recursive");
    let force = norm.contains(" -f")
        || norm.contains(" -rf")
        || norm.contains(" -fr")
        || norm.contains("--force");
    if !(recursive && force) {
        return None;
    }
    let catastrophic = norm
        .split(' ')
        .any(|t| matches!(t, "/" | "/*" | "~" | "~/" | "~/*" | "$home" | "$home/*"))
        || norm.contains("--no-preserve-root");
    if catastrophic {
        Some("recursive forced delete of a root/home path")
    } else {
        None
    }
}

/// Search files under `base` (within `root`) for a regex via the ripgrep engine.
fn ripgrep_search(base: &Path, root: &Path, pattern: &str, max: usize) -> Result<Vec<Value>> {
    use grep::regex::RegexMatcher;
    use grep::searcher::sinks::UTF8;
    use grep::searcher::Searcher;
    use ignore::WalkBuilder;

    let matcher = RegexMatcher::new(pattern)
        .map_err(|e| CoreError::Message(format!("invalid search regex '{pattern}': {e}")))?;
    let mut out: Vec<Value> = Vec::new();
    let walker = WalkBuilder::new(base)
        .hidden(true)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".fleety-backups" | ".git"
            )
        })
        .build();
    for dent in walker {
        if out.len() >= max {
            break;
        }
        let dent = match dent {
            Ok(d) => d,
            Err(_) => continue,
        };
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = dent.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        let mut searcher = Searcher::new();
        let _ = searcher.search_path(
            &matcher,
            path,
            UTF8(|lnum, line| {
                if out.len() < max {
                    out.push(json!({ "file": rel, "line": lnum, "text": line.trim_end() }));
                }
                Ok(out.len() < max)
            }),
        );
    }
    Ok(out)
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

struct SearchFiles {
    root: PathBuf,
}

#[async_trait]
impl Tool for SearchFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_files".to_string(),
            description: "Search workspace file contents by regex (ripgrep engine: respects .gitignore, skips binaries); returns file/line/text matches.".to_string(),
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
        let matches = ripgrep_search(&base, &canon_root, query, max)?;
        let truncated = matches.len() >= max;
        Ok(json!({ "matches": matches, "truncated": truncated }))
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
            description: "Write a UTF-8 text file within the workspace (its parent directory must exist). The previous content is backed up for rollback; the result includes a unified diff.".to_string(),
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

        let old = std::fs::read_to_string(&resolved).unwrap_or_default();
        let mut backup = Value::Null;
        if resolved.exists() {
            backup = backup_existing(&self.backups, path, &resolved)?;
        }
        std::fs::write(&resolved, content)
            .map_err(|e| CoreError::Message(format!("cannot write '{path}': {e}")))?;
        Ok(json!({
            "path": path,
            "bytes_written": content.len(),
            "backup": backup,
            "diff": unified_diff(&old, content, path),
        }))
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
            description: "Replace an exact, unique substring in a workspace file (precise edit). The prior content is backed up; the result includes a unified diff.".to_string(),
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
        Ok(json!({
            "path": path,
            "replaced": 1,
            "backup": backup,
            "diff": unified_diff(&content, &updated, path),
        }))
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
            description: "Run a shell command in the workspace and capture stdout/stderr/exit code. Clearly destructive commands are refused. Pass `track` (paths) to get a unified diff of what those files looked like before vs after — useful when a command (sed, a build, etc.) changes files.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string", "description": "workspace-relative working dir (default workspace root)" },
                    "track": { "type": "array", "items": { "type": "string" }, "description": "workspace-relative paths to diff (before vs after the command)" }
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

        // Snapshot tracked paths so we can diff what the command changed.
        let tracked: Vec<String> = args
            .get("track")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut before: Vec<(String, PathBuf, String)> = Vec::new();
        for rel in &tracked {
            let path = resolve_lenient(&self.root, rel)?;
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            before.push((rel.clone(), path, content));
        }

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

        let mut result = json!({
            "exit_code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        });
        if !before.is_empty() {
            let diffs: Vec<Value> = before
                .iter()
                .map(|(rel, path, old)| {
                    let new = std::fs::read_to_string(path).unwrap_or_default();
                    json!({
                        "path": rel,
                        "changed": *old != new,
                        "diff": unified_diff(old, &new, rel),
                    })
                })
                .collect();
            result["diffs"] = json!(diffs);
        }
        Ok(result)
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
        Ok(json!({ "status": run_git(&self.root, &["status", "--porcelain"])? }))
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
            description: "Show the unstaged `git diff` for the workspace, plus any untracked new files (changes from any source — edits, run_command, external tools — show here).".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let diff = run_git(&self.root, &["diff"])?;
        let untracked: Vec<String> =
            run_git(&self.root, &["ls-files", "--others", "--exclude-standard"])
                .unwrap_or_default()
                .lines()
                .map(|l| l.to_string())
                .filter(|l| !l.is_empty())
                .collect();
        Ok(json!({ "diff": diff, "untracked": untracked }))
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
        Ok(json!({ "log": run_git(&self.root, &["log", "--oneline", "-n", &limit.to_string()])? }))
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
        Ok(json!({ "show": run_git(&self.root, &["show", "--stat", reference])? }))
    }
}

struct DeleteFile {
    root: PathBuf,
    backups: PathBuf,
}

#[async_trait]
impl Tool for DeleteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delete_file".to_string(),
            description: "Delete a file in the workspace. The content is backed up first, so the delete can be undone with rollback.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let resolved = resolve_in_root(&self.root, path)?;
        if resolved.is_dir() {
            return Err(CoreError::Message(format!(
                "'{path}' is a directory; use run_command for directory removal"
            )));
        }
        let backup = backup_existing(&self.backups, path, &resolved)?;
        std::fs::remove_file(&resolved)
            .map_err(|e| CoreError::Message(format!("cannot delete '{path}': {e}")))?;
        Ok(json!({ "path": path, "deleted": true, "backup": backup }))
    }
}

struct MoveFile {
    root: PathBuf,
    backups: PathBuf,
}

#[async_trait]
impl Tool for MoveFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "move_file".to_string(),
            description: "Move/rename a file within the workspace. If the destination exists it is backed up first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" }
                },
                "required": ["from", "to"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let from = require_str(&args, "from")?;
        let to = require_str(&args, "to")?;
        let src = resolve_in_root(&self.root, from)?;
        let dest = resolve_lenient(&self.root, to)?;
        let mut backup = Value::Null;
        if dest.exists() {
            backup = backup_existing(&self.backups, to, &dest)?;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create '{to}' parent: {e}")))?;
        }
        std::fs::rename(&src, &dest)
            .map_err(|e| CoreError::Message(format!("cannot move '{from}' -> '{to}': {e}")))?;
        Ok(json!({ "from": from, "to": to, "moved": true, "backup": backup }))
    }
}

struct MakeDir {
    root: PathBuf,
}

#[async_trait]
impl Tool for MakeDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "make_dir".to_string(),
            description: "Create a directory (and any missing parents) within the workspace."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let resolved = resolve_lenient(&self.root, path)?;
        std::fs::create_dir_all(&resolved)
            .map_err(|e| CoreError::Message(format!("cannot create dir '{path}': {e}")))?;
        Ok(json!({ "path": path, "created": true }))
    }
}

struct Rollback {
    root: PathBuf,
    backups: PathBuf,
}

#[async_trait]
impl Tool for Rollback {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rollback".to_string(),
            description: "Restore a file from a backup produced by write_file/edit_file/delete_file/move_file (pass the backup `id`).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "backup_id": { "type": "string" } },
                "required": ["backup_id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "backup_id")?;
        apply_backup(&self.root, &self.backups, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    #[tokio::test]
    async fn read_search_edit_with_diff_and_escape_guard() {
        let root = temp();
        let backups = root.join(".bak");
        std::fs::write(root.join("a.txt"), "hello world\nfoo bar\n").expect("write");
        let mut reg = ToolRegistry::new();
        register_workspace(&mut reg, &root, &backups);

        // read
        let r = reg
            .call("read_file", json!({ "path": "a.txt" }))
            .await
            .expect("read");
        assert!(r["content"]
            .as_str()
            .unwrap_or_default()
            .contains("foo bar"));

        // ripgrep search (regex)
        let s = reg
            .call("search_files", json!({ "query": "foo" }))
            .await
            .expect("search");
        assert_eq!(s["matches"][0]["line"], json!(2));

        // edit + unified diff
        let e = reg
            .call(
                "edit_file",
                json!({ "path": "a.txt", "old": "foo bar", "new": "FOO BAR" }),
            )
            .await
            .expect("edit");
        assert!(e["diff"].as_str().unwrap_or_default().contains("+FOO BAR"));
        assert!(e["backup"]["id"].is_string());

        // write a new file -> diff vs empty
        let w = reg
            .call("write_file", json!({ "path": "b.txt", "content": "new\n" }))
            .await
            .expect("write");
        assert!(w["diff"].as_str().unwrap_or_default().contains("+new"));

        // escape guard
        assert!(reg
            .call("read_file", json!({ "path": "../escape" }))
            .await
            .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn track_diff_delete_rollback_move_mkdir() {
        let root = temp();
        let backups = root.join(".bak");
        std::fs::write(root.join("t.txt"), "one\ntwo\n").expect("seed");
        let mut reg = ToolRegistry::new();
        register_workspace(&mut reg, &root, &backups);

        // run_command with track -> diff of what the command changed
        let shell_append = if cfg!(windows) {
            "echo three>> t.txt"
        } else {
            "printf 'three\\n' >> t.txt"
        };
        let ran = reg
            .call(
                "run_command",
                json!({ "command": shell_append, "track": ["t.txt"] }),
            )
            .await
            .expect("run");
        assert_eq!(ran["diffs"][0]["changed"], json!(true));
        assert!(ran["diffs"][0]["diff"]
            .as_str()
            .unwrap_or_default()
            .contains("+three"));

        // make_dir + delete_file (backed up) + rollback restores it
        reg.call("make_dir", json!({ "path": "sub/deep" }))
            .await
            .expect("mkdir");
        assert!(root.join("sub/deep").is_dir());

        std::fs::write(root.join("sub/d.txt"), "keepme").expect("seed2");
        let del = reg
            .call("delete_file", json!({ "path": "sub/d.txt" }))
            .await
            .expect("delete");
        assert!(!root.join("sub/d.txt").exists());
        let id = del["backup"]["id"].as_str().expect("backup id").to_string();
        reg.call("rollback", json!({ "backup_id": id }))
            .await
            .expect("rollback");
        assert_eq!(
            std::fs::read_to_string(root.join("sub/d.txt")).expect("read"),
            "keepme"
        );

        // move_file
        reg.call(
            "move_file",
            json!({ "from": "sub/d.txt", "to": "sub/e.txt" }),
        )
        .await
        .expect("move");
        assert!(!root.join("sub/d.txt").exists() && root.join("sub/e.txt").exists());

        // guards: no escaping, no absolute, no ".."
        assert!(reg
            .call("make_dir", json!({ "path": "../evil" }))
            .await
            .is_err());
        assert!(reg
            .call("rollback", json!({ "backup_id": "../x" }))
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn critical_guard_flags_irreversible_but_not_ordinary() {
        for c in [
            "rm -rf /",
            "sudo rm -rf ~",
            "mkfs.ext4 /dev/sda1",
            "shutdown -h now",
            "diskpart",
        ] {
            assert!(critical_reason(c).is_some(), "should refuse: {c}");
        }
        for c in [
            "rm -rf ./build",
            "rm -rf node_modules",
            "cargo build",
            "ls /",
        ] {
            assert!(critical_reason(c).is_none(), "should allow: {c}");
        }
    }
}
