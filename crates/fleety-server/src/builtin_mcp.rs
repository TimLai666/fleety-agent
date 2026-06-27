//! Built-in MCP servers shipped with the runtime. At server boot we seed
//! `builtin.json` next to the user-installed `installed.json`; the MCP runtime
//! merges them (installed shadows builtin by name), so Fleety gets a curated
//! set of MCP servers out of the box with no manual `mcp_add`.
//!
//! Today we ship one: **ddgs** (`deedy5/ddgs`), a metasearch MCP that gives the
//! agent web text / image / news / video / book search plus URL content
//! extraction. ddgs is a Python package — `pip install -U ddgs[mcp]` puts a
//! `ddgs` console script on `PATH` and `ddgs mcp` launches the stdio MCP. We
//! seed the entry every boot (so a binary upgrade picks up an updated default
//! command line); availability of the actual `ddgs` binary is reported by
//! [`check_ddgs`] with optional best-effort auto-install.

use std::path::Path;
use std::process::Stdio;

use agent_core::{CoreError, Result};
use serde_json::{json, Value};

/// Override the resolved `ddgs` binary path. Falls back to PATH lookup.
const DDGS_BIN_ENV: &str = "FLEETY_DDGS_BIN";
/// Override the args we pass when spawning ddgs. Defaults to `["mcp"]`. JSON
/// array (e.g. `["mcp","-pr","socks5h://127.0.0.1:9150"]`) for the proxy mode
/// in upstream's README.
const DDGS_ARGS_ENV: &str = "FLEETY_DDGS_ARGS";
/// Default behaviour at server boot when ddgs isn't on PATH: try to install
/// it (pipx → pip --user → python -m pip --user). Set
/// `FLEETY_DDGS_AUTO_INSTALL=0` to opt out and get a notify-only warning
/// instead — useful for hermetic / no-network environments where pip would
/// fail anyway. The previous behaviour (notify-only by default) was reversed
/// once we made ddgs the canonical built-in web-search MCP.
const DDGS_AUTO_INSTALL_ENV: &str = "FLEETY_DDGS_AUTO_INSTALL";

fn auto_install_enabled() -> bool {
    std::env::var(DDGS_AUTO_INSTALL_ENV).as_deref() != Ok("0")
}

fn ddgs_binary_name() -> &'static str {
    if cfg!(windows) {
        "ddgs.exe"
    } else {
        "ddgs"
    }
}

/// Resolve the ddgs command: env override > PATH lookup > bare name (errors at
/// spawn time with a clear "not on PATH" message via `mcp_call`).
fn resolve_ddgs_command() -> String {
    if let Ok(p) = std::env::var(DDGS_BIN_ENV) {
        if !p.is_empty() {
            return p;
        }
    }
    if let Some(found) = which_on_path(ddgs_binary_name()) {
        return found;
    }
    ddgs_binary_name().to_string()
}

fn which_on_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn ddgs_args() -> Vec<String> {
    if let Ok(raw) = std::env::var(DDGS_ARGS_ENV) {
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(&raw) {
            if !arr.is_empty() {
                return arr;
            }
        }
    }
    vec!["mcp".to_string()]
}

/// One built-in MCP server entry to write into `builtin.json`.
struct BuiltinEntry {
    name: &'static str,
    command: String,
    args: Vec<String>,
}

fn builtin_servers() -> Vec<BuiltinEntry> {
    vec![BuiltinEntry {
        name: "ddgs",
        command: resolve_ddgs_command(),
        args: ddgs_args(),
    }]
}

/// Write the built-in MCP server list into `path`, overwriting. Best-effort:
/// a write failure must not prevent server start (the merged list just won't
/// include the built-ins; agent can still `mcp_add` manually).
pub fn seed(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("create mcp dir: {e}")))?;
    }
    let arr: Vec<Value> = builtin_servers()
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "command": e.command,
                "args": e.args,
            })
        })
        .collect();
    let body = serde_json::to_string_pretty(&json!({ "servers": arr }))
        .map_err(|e| CoreError::Message(format!("serialize builtin mcp: {e}")))?;
    std::fs::write(path, body)
        .map_err(|e| CoreError::Message(format!("write builtin mcp: {e}")))?;
    Ok(())
}

/// Whether `ddgs --version` (or a similar light check) succeeds. We don't trust
/// `which` alone because PATH may include a stale shim.
async fn ddgs_runs(command: &str) -> bool {
    tokio::process::Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort `pip install` of `ddgs[mcp]`. Tries `pipx` first (isolated venv),
/// falls back to `pip install --user`. Returns true on success.
async fn try_install_ddgs() -> bool {
    let candidates: &[(&str, &[&str])] = &[
        ("pipx", &["install", "--force", "ddgs[mcp]"]),
        ("pip3", &["install", "--user", "-U", "ddgs[mcp]"]),
        ("pip", &["install", "--user", "-U", "ddgs[mcp]"]),
        (
            "python3",
            &["-m", "pip", "install", "--user", "-U", "ddgs[mcp]"],
        ),
        (
            "python",
            &["-m", "pip", "install", "--user", "-U", "ddgs[mcp]"],
        ),
    ];
    for (cmd, args) in candidates {
        tracing::info!(installer = cmd, "trying to auto-install ddgs[mcp]");
        match tokio::process::Command::new(cmd).args(*args).status().await {
            Ok(s) if s.success() => {
                tracing::info!(installer = cmd, "ddgs[mcp] installed");
                return true;
            }
            Ok(_) => continue,
            Err(_) => continue, // installer not present
        }
    }
    false
}

/// Check ddgs availability and, by default, install it if it isn't reachable
/// (`FLEETY_DDGS_AUTO_INSTALL=0` opts out for hermetic / no-network setups).
/// Runs after seeding the builtin so `mcp_call(server="ddgs", …)` works on
/// the very first turn after server boot.
pub async fn check_ddgs() {
    let command = resolve_ddgs_command();
    if ddgs_runs(&command).await {
        tracing::info!(%command, "ddgs MCP available");
        return;
    }
    if !auto_install_enabled() {
        tracing::warn!(
            "ddgs MCP binary not on PATH and FLEETY_DDGS_AUTO_INSTALL=0; the built-in \
             `ddgs` MCP server won't work until you install it manually. \
             Run `pip install -U ddgs[mcp]` (or `pipx install ddgs[mcp]`), or set \
             FLEETY_DDGS_BIN to an absolute path."
        );
        return;
    }
    if try_install_ddgs().await {
        // Refresh PATH lookup (pipx / --user may have just added a new dir).
        let refreshed = resolve_ddgs_command();
        if ddgs_runs(&refreshed).await {
            tracing::info!(command = %refreshed, "ddgs MCP installed and reachable");
            return;
        }
    }
    tracing::warn!(
        "ddgs auto-install failed (no pipx / pip / python on PATH, or the install itself errored). \
         The built-in `ddgs` MCP server won't work until you install it manually: \
         `pip install -U ddgs[mcp]` (or `pipx install ddgs[mcp]`). \
         Set FLEETY_DDGS_AUTO_INSTALL=0 to silence this warning."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_writes_ddgs_entry() {
        let dir = std::env::temp_dir().join(format!("fleety-bmcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let path = dir.join("builtin.json");
        seed(&path).expect("seed");
        let text = std::fs::read_to_string(&path).expect("read");
        let v: Value = serde_json::from_str(&text).expect("json");
        let servers = v["servers"].as_array().expect("arr");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], json!("ddgs"));
        assert_eq!(servers[0]["args"], json!(["mcp"]));
        // command is non-empty even when ddgs isn't installed (falls back to bare name).
        assert!(!servers[0]["command"].as_str().unwrap_or("").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_overrides_args() {
        std::env::set_var(DDGS_ARGS_ENV, r#"["mcp","-pr","socks5h://127.0.0.1:9150"]"#);
        let args = ddgs_args();
        std::env::remove_var(DDGS_ARGS_ENV);
        assert_eq!(args, vec!["mcp", "-pr", "socks5h://127.0.0.1:9150"]);
    }

    #[test]
    fn auto_install_defaults_on_and_opts_out_with_zero() {
        // Default (unset) → on.
        std::env::remove_var(DDGS_AUTO_INSTALL_ENV);
        assert!(auto_install_enabled());
        // `=0` → off (the only explicit opt-out shape).
        std::env::set_var(DDGS_AUTO_INSTALL_ENV, "0");
        assert!(!auto_install_enabled());
        // Anything else → on (so an accidental "true" / "1" doesn't change behaviour).
        std::env::set_var(DDGS_AUTO_INSTALL_ENV, "yes");
        assert!(auto_install_enabled());
        std::env::remove_var(DDGS_AUTO_INSTALL_ENV);
    }

    #[test]
    fn env_overrides_bin() {
        std::env::set_var(DDGS_BIN_ENV, "/opt/custom/ddgs");
        let cmd = resolve_ddgs_command();
        std::env::remove_var(DDGS_BIN_ENV);
        assert_eq!(cmd, "/opt/custom/ddgs");
    }
}
