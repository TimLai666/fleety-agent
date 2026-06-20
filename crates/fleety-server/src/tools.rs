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

/// Build the read-only workspace tool registry rooted at `workspace`.
pub fn build_registry(workspace: &Path) -> ToolRegistry {
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
    registry
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_and_list_within_root() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root);
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
        let registry = build_registry(&root);
        let result = registry
            .call("read_file", json!({ "path": "../../../../etc/passwd" }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn missing_arg_is_actionable() {
        let root = std::env::current_dir().expect("cwd");
        let registry = build_registry(&root);
        let err = registry
            .call("read_file", json!({}))
            .await
            .expect_err("should require path");
        assert!(err.report().message.contains("path"));
    }
}
