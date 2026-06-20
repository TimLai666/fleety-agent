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

use agent_core::{CoreError, Result, Tool, ToolRegistry, ToolSpec};

/// Build the workspace tool registry rooted at `workspace`. Mutating tools back
/// up to `backups_dir` (outside the workspace) before changing files.
pub fn build_registry(workspace: &Path, backups_dir: &Path) -> ToolRegistry {
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
    registry.register(Box::new(WriteFile {
        root: workspace.to_path_buf(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(RunCommand {
        root: workspace.to_path_buf(),
    }));
    registry
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
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let diff = run_git(&self.root, &["diff"])?;
        Ok(json!({ "diff": diff }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_and_list_within_root() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root, &root);
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
        let registry = build_registry(&root, &root);
        let result = registry
            .call("read_file", json!({ "path": "../../../../etc/passwd" }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_arg_is_actionable() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root, &root);
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
        let registry = build_registry(&ws, &backups);

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
        let registry = build_registry(&ws, &ws);
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
        let registry = build_registry(&ws, &ws);
        let err = registry
            .call("run_command", json!({ "command": "rm -rf /" }))
            .await
            .expect_err("should refuse");
        assert!(err.report().message.contains("refused"));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
