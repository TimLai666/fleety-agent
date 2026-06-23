//! Built-in MCP servers shipped with the runtime. At server startup we seed a
//! `builtin.json` next to the user-installed `installed.json`; the MCP runtime
//! merges them (user-installed shadows built-in by name), so Fleety exposes a
//! curated set of MCP servers out of the box with no manual `mcp_add` from the
//! agent or the user.
//!
//! The `codebase-memory-mcp` binary is provisioned by `fleetyd install/update`
//! into the same directory as `fleetyd` (and typically `fleety-server`); we
//! resolve the absolute path at seed time so the call gives a clear error if
//! provisioning hasn't run yet.

use std::path::Path;
use std::time::Duration;

use agent_core::{CoreError, Result};
use serde_json::{json, Value};

use crate::mcp::invoke_mcp;

/// `list_projects` is a near-instant SQLite read — keep the timeout tight.
const CBM_LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// `index_repository` on a large repo can run for minutes (the cbm README cites
/// ~3 min for the Linux kernel). Cap generously so we don't kill a live run.
const CBM_INDEX_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// The name of the prebuilt codebase-memory-mcp binary on disk.
fn cbm_binary_name() -> &'static str {
    if cfg!(windows) {
        "codebase-memory-mcp.exe"
    } else {
        "codebase-memory-mcp"
    }
}

/// Resolve the codebase-memory-mcp binary path. Honours `FLEETY_CBM_BIN`,
/// otherwise looks next to the current executable. Falls back to the bare
/// binary name (PATH lookup at spawn time) so the seed is always writable; if
/// the binary is missing, `mcp_call` returns an actionable spawn error.
fn resolve_cbm_binary() -> String {
    if let Ok(path) = std::env::var("FLEETY_CBM_BIN") {
        return path;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(cbm_binary_name());
            if candidate.is_file() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    cbm_binary_name().to_string()
}

/// One built-in MCP server: `(name, command, args)`.
fn builtin_servers() -> Vec<(String, String, Vec<String>)> {
    vec![(
        "codebase-memory".to_string(),
        resolve_cbm_binary(),
        Vec::new(),
    )]
}

/// Write the built-in MCP server list into `path` (overwriting), so an updated
/// binary always ships an updated set of built-ins. Best-effort: a write
/// failure must not prevent the server from starting.
pub fn seed(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create mcp dir: {e}")))?;
    }
    let arr: Vec<Value> = builtin_servers()
        .into_iter()
        .map(|(name, command, args)| json!({ "name": name, "command": command, "args": args }))
        .collect();
    let body = serde_json::to_string_pretty(&json!({ "servers": arr }))
        .map_err(|e| CoreError::Message(format!("serialize builtin mcp: {e}")))?;
    std::fs::write(path, body)
        .map_err(|e| CoreError::Message(format!("write builtin mcp: {e}")))?;
    Ok(())
}

/// Normalise a workspace path for comparison with `list_projects` records.
/// Canonicalise when we can (handles symlinks, mixed separators, trailing `.`);
/// fall back to the raw path so a non-existent workspace still produces a
/// stable string.
fn normalise_path(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical.to_string_lossy().to_string()
}

/// Determine whether `workspace` is already an indexed project in the local
/// codebase-memory store. Treats any error (binary missing, JSON shape change)
/// as "unknown" so the caller defaults to triggering an index — index is
/// idempotent, so over-indexing is at worst wasted CPU.
async fn workspace_already_indexed(cbm_binary: &str, workspace: &Path) -> Result<bool> {
    let result = invoke_mcp(
        "codebase-memory",
        cbm_binary,
        &[],
        "list_projects",
        &json!({}),
        CBM_LIST_TIMEOUT,
    )
    .await?;
    let target = normalise_path(workspace);
    // MCP `tools/call` results are wrapped in `{ content: [{ text: "..." }] }`.
    // The text payload is the tool's JSON. Be tolerant of either shape so a
    // future cbm version that returns structured `result` directly still works.
    let projects = extract_projects(&result);
    Ok(projects.iter().any(|p| {
        let raw = p
            .get("root_path")
            .or_else(|| p.get("path"))
            .or_else(|| p.get("repo_path"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if raw.is_empty() {
            return false;
        }
        normalise_path(Path::new(raw)) == target
    }))
}

/// Pull the project list out of an MCP `tools/call` result regardless of
/// whether cbm returned structured content or a `{content:[{text:JSON}]}`
/// envelope.
fn extract_projects(result: &Value) -> Vec<Value> {
    if let Some(arr) = result.get("projects").and_then(Value::as_array) {
        return arr.clone();
    }
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                if let Ok(inner) = serde_json::from_str::<Value>(text) {
                    if let Some(arr) = inner.get("projects").and_then(Value::as_array) {
                        return arr.clone();
                    }
                }
            }
        }
    }
    Vec::new()
}

/// Best-effort: ensure the configured workspace is indexed by the built-in
/// `codebase-memory` MCP server, so the agent's first structural query has data
/// to work with. Runs in the background at server startup. If cbm isn't yet
/// provisioned, the workspace doesn't exist, or anything else fails, we log a
/// warning and return — never crash the server.
pub async fn auto_index_workspace(workspace: &Path) {
    if !workspace.is_dir() {
        tracing::warn!(
            workspace = %workspace.display(),
            "skipping codebase-memory auto-index: workspace is not a directory"
        );
        return;
    }
    let binary = resolve_cbm_binary();
    match workspace_already_indexed(&binary, workspace).await {
        Ok(true) => {
            tracing::info!(
                workspace = %workspace.display(),
                "codebase-memory: workspace already indexed; skipping auto-index"
            );
            return;
        }
        Ok(false) => {}
        Err(e) => {
            // Most likely: cbm binary not provisioned yet, or the user's running
            // an older fleetyd. Log clearly and fall through — we'll attempt
            // index_repository anyway, which will surface the same error.
            tracing::warn!(
                report = ?e.report(),
                "codebase-memory: could not check list_projects; attempting auto-index anyway"
            );
        }
    }
    tracing::info!(
        workspace = %workspace.display(),
        "codebase-memory: kicking off background index_repository(mode=full)"
    );
    let arguments = json!({
        "repo_path": normalise_path(workspace),
        "mode": "full",
    });
    match invoke_mcp(
        "codebase-memory",
        &binary,
        &[],
        "index_repository",
        &arguments,
        CBM_INDEX_TIMEOUT,
    )
    .await
    {
        Ok(_) => tracing::info!(
            workspace = %workspace.display(),
            "codebase-memory: auto-index complete"
        ),
        Err(e) => tracing::warn!(
            report = ?e.report(),
            "codebase-memory: auto-index failed (the agent can still run; this just means \
             structural queries will be unavailable until the user runs index_repository)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_projects_handles_both_envelopes() {
        // Direct structured shape.
        let direct = json!({ "projects": [{ "name": "x", "root_path": "/repo" }] });
        let p = extract_projects(&direct);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["name"], json!("x"));

        // MCP `tools/call` text envelope.
        let wrapped = json!({
            "content": [{
                "type": "text",
                "text": "{\"projects\":[{\"name\":\"y\",\"root_path\":\"/r\"}]}"
            }]
        });
        let p = extract_projects(&wrapped);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0]["name"], json!("y"));

        // Nothing recognisable -> empty.
        let none = json!({ "ok": true });
        assert!(extract_projects(&none).is_empty());
    }

    #[test]
    fn normalise_path_falls_back_when_unreadable() {
        let bogus = Path::new("/this/should/not/exist/anywhere-12345");
        let n = normalise_path(bogus);
        assert!(!n.is_empty());
    }

    #[test]
    fn seed_writes_codebase_memory_entry() {
        let dir = std::env::temp_dir().join(format!("fleety-bmcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let path = dir.join("builtin.json");
        seed(&path).expect("seed");
        let text = std::fs::read_to_string(&path).expect("read");
        let v: Value = serde_json::from_str(&text).expect("json");
        let servers = v["servers"].as_array().expect("arr");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], json!("codebase-memory"));
        assert!(!servers[0]["command"].as_str().unwrap_or("").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_resolver_returns_something_nonempty() {
        // Whatever the host environment, the resolver must yield a usable string
        // (an absolute path, an env override, or the bare name).
        let resolved = resolve_cbm_binary();
        assert!(!resolved.is_empty());
    }
}
