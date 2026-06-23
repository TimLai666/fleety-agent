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

use agent_core::{CoreError, Result};
use serde_json::{json, Value};

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

#[cfg(test)]
mod tests {
    use super::*;

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
