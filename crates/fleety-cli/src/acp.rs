//! `fleety acp` — make Fleety an Agent Client Protocol (ACP) agent.
//!
//! An ACP-capable editor (e.g. Zed) launches `fleety acp` as a subprocess and
//! speaks JSON-RPC 2.0 over stdio, with messages delimited by newlines (one JSON
//! object per line, per the ACP transport spec). This adapter bridges ACP to the
//! existing fleety-server: it maps initialize / session.new / session.load /
//! session.prompt / session.cancel to the server's conversation protocol, streams
//! the server's assistant output back as `session/update` notifications, and
//! surfaces tool approvals as `session/request_permission`. Only JSON-RPC goes to
//! stdout; logs go to stderr.
//!
//! The framing + JSON-RPC types and the ACP↔server mappings are pure and
//! unit-tested, and verified end-to-end against Zed 1.9: newline framing, an
//! `initialize`/`session.new`/`session.prompt` round-trip, and `session/update`
//! streaming (tagged by `sessionUpdate`, carrying a text ContentBlock) render in
//! the editor.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

// ---- JSON-RPC 2.0 framing (ACP: one JSON object per line, newline-delimited) ----
//
// ACP over stdio delimits messages by `\n`, with no embedded newlines
// (agentclientprotocol.com/protocol/v1/transports). `serde_json::to_string`
// emits single-line JSON, so a trailing `\n` is a conformant frame.

/// One decoded inbound frame: end-of-input, a malformed (non-JSON) line, or a
/// parsed message.
pub enum FrameIn {
    Eof,
    Malformed,
    Message(Value),
}

/// Write one newline-delimited JSON-RPC message.
pub fn write_frame<W: Write>(w: &mut W, v: &Value) -> std::io::Result<()> {
    let mut body = serde_json::to_string(v)?;
    body.push('\n');
    w.write_all(body.as_bytes())?;
    w.flush()
}

/// Read one newline-delimited JSON-RPC message (sync; backs the framing tests).
/// `None` on EOF; a malformed line parses to `None` here — the async runtime
/// variant distinguishes malformed input so it can reply with a JSON-RPC error.
#[allow(dead_code)]
pub fn read_frame<R: BufRead>(r: &mut R) -> std::io::Result<Option<Value>> {
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let line = line.trim();
        if line.is_empty() {
            continue; // tolerate blank lines between messages
        }
        return Ok(serde_json::from_str(line).ok());
    }
}

// ---- JSON-RPC message builders ----

pub fn response_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn response_err(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Render an actionable core error without losing its stable category or
/// remediation in the JSON-RPC envelope.
pub fn response_err_report(id: Value, code: i64, report: &agent_core::ErrorReport) -> Value {
    let mut data = json!({ "kind": report.kind });
    if let Some(remediation) = &report.remediation {
        data["remediation"] = Value::String(remediation.clone());
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": report.message, "data": data }
    })
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// JSON-RPC method-not-found code.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC parse-error code (malformed input).
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC internal-error code for a failed agent operation. NOT the `-32000`
/// server-error code: ACP clients (Zed) treat `-32000` as "authentication
/// required", so a plain failure (e.g. the server is unreachable) sent as
/// `-32000` is mis-rendered as an auth prompt. Use the standard internal-error
/// code so the real message is shown.
pub const INTERNAL_ERROR: i64 = -32603;

// ---- ACP <-> fleety-server mappings (pure) ----

/// `session/update` for streamed assistant text. The update is an ACP
/// `SessionUpdate` tagged by `sessionUpdate` (NOT `kind`), carrying a ContentBlock
/// — Zed rejects any other shape with "missing field `sessionUpdate`".
pub fn assistant_update(session_id: &str, text: &str) -> Value {
    notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
}

/// `session/request_permission` params from a server approval request — emitted
/// to the editor when the server asks for tool approval (require-approval policy).
/// `tool_call_id` (we use the server's approval id) lets the editor associate
/// the permission dialog with a tool call — ACP's ToolCallUpdate requires it.
pub fn permission_request(
    session_id: &str,
    tool_call_id: &str,
    tool: &str,
    summary: &str,
) -> Value {
    json!({
        "sessionId": session_id,
        "toolCall": { "toolCallId": tool_call_id, "title": tool, "summary": summary },
        "options": [
            { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
            { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
        ]
    })
}

/// The stop reason for a completed prompt turn: a turn the user cancelled
/// (`session/cancel` → CancelTurn) reports `"cancelled"`, a turn that ran to
/// completion reports `"end_turn"`.
pub fn stop_reason(cancelled: bool) -> &'static str {
    if cancelled {
        "cancelled"
    } else {
        "end_turn"
    }
}

/// Build the `OriginContext` for a session's cwd so the server roots the
/// conversation there (session-workspace-cwd).
pub fn cwd_to_origin(cwd: Option<&str>) -> fleety_protocol::OriginContext {
    fleety_protocol::OriginContext {
        hostname: std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok(),
        os: Some(std::env::consts::OS.to_string()),
        cwd: cwd.map(str::to_string),
        home: fleety_tools::device::home_is_known().then(|| {
            fleety_tools::device::home_dir()
                .to_string_lossy()
                .into_owned()
        }),
    }
}

/// Capabilities returned from `initialize` — advertise session loading; we do
/// not use client-side fs/terminal in v1.
pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": { "loadSession": true },
        "serverInfo": { "name": "fleety", "version": agent_core::VERSION }
    })
}

/// The `session/load` reply, built to the ACP `LoadSessionResponse` shape (sent
/// after the conversation history is replayed as `session/update`
/// notifications). Every `LoadSessionResponse` field is optional — `modes`,
/// `configOptions`, `_meta`. This adapter advertises none of session modes or
/// config options, so `modes` is explicitly `null` (and the others omitted):
/// a deliberately-constructed response of the right shape, not an arbitrary
/// empty object that merely happens to deserialize.
pub fn load_session_result() -> Value {
    json!({ "modes": Value::Null })
}

// ---- `fleety acp install`: write the Zed agent-server config ----

/// Zed's `settings.json` path for this platform, if it can be located.
pub fn zed_settings_path() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        absolute_env_path(std::env::var_os("APPDATA")).map(|p| p.join("Zed").join("settings.json"))
    }
    #[cfg(not(windows))]
    {
        absolute_env_path(std::env::var_os("HOME"))
            .map(|p| p.join(".config").join("zed").join("settings.json"))
    }
}

fn absolute_env_path(value: Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(value?);
    (!path.as_os_str().is_empty() && path.is_absolute()).then_some(path)
}

/// The Zed `agent_servers` entry that launches this binary as a custom ACP agent.
pub fn fleety_agent_entry(command: &str, server: Option<&str>) -> Value {
    let mut env = serde_json::Map::new();
    if let Some(s) = server {
        env.insert("FLEETY_AGENT_URL".to_string(), json!(s));
    }
    json!({
        "type": "custom",
        "command": command,
        "args": ["acp"],
        "env": Value::Object(env),
    })
}

/// Merge the Fleety `agent_servers` entry into existing Zed settings JSON,
/// returning the updated pretty JSON and whether an existing Fleety entry was
/// replaced (an update vs a fresh add). Errors (without touching the file) when
/// the input is not plain JSON — Zed allows JSONC comments, which we won't
/// clobber. Re-running always overwrites the Fleety entry (e.g. a new binary path
/// after `cargo install`), leaving other settings and other agents intact.
pub fn merge_zed_settings(
    existing: &str,
    command: &str,
    server: Option<&str>,
) -> std::result::Result<(String, bool), String> {
    let mut root: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| {
            format!("settings.json is not plain JSON ({e}); it may contain comments")
        })?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "settings.json root is not a JSON object".to_string())?;
    let servers = obj
        .entry("agent_servers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "`agent_servers` is not a JSON object".to_string())?;
    let updated = servers.contains_key("Fleety");
    let mut preserved_env = servers
        .get("Fleety")
        .and_then(|entry| entry.get("env"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let previous_server = preserved_env
        .get("FLEETY_AGENT_URL")
        .and_then(Value::as_str);
    if server.is_none() || previous_server != server {
        preserved_env.remove("FLEETY_TOKEN");
    }
    match server {
        Some(server) => {
            preserved_env.insert("FLEETY_AGENT_URL".to_string(), json!(server));
        }
        None => {
            preserved_env.remove("FLEETY_AGENT_URL");
        }
    }
    let mut entry = fleety_agent_entry(command, None);
    entry["env"] = Value::Object(preserved_env);
    servers.insert("Fleety".to_string(), entry);
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok((pretty, updated))
}

/// Re-point any *already-installed* ACP agent configs at the current binary, for
/// `fleety update` to call. This self-heals a changed binary path or an evolved
/// `acp` invocation. It NEVER newly installs — only editors already set up for
/// Fleety are touched; a missing config or a settings file with no Fleety entry
/// is a no-op, while unreadable or invalid existing Fleety settings are reported
/// so `fleety update` cannot claim completion.
pub fn refresh_installed(server: Option<&str>) -> agent_core::Result<()> {
    use agent_core::CoreError;

    let Some(path) = zed_settings_path() else {
        return Err(CoreError::Message(
            "cannot locate Zed settings because the platform settings base is missing or not an absolute path; Fleety update is incomplete"
                .to_string(),
        ));
    };
    let existing = match std::fs::read_to_string(&path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CoreError::Message(format!(
            "cannot read the installed Zed settings at {}: {error}; Fleety update is incomplete",
            path.display()
        )))
        }
    };
    if let Some(merged) = refresh_zed_settings(&existing, &current_exe_str(), server)
        .map_err(|error| {
            CoreError::Message(format!(
                "cannot safely refresh the installed Zed settings at {}: {error}; Fleety update is incomplete",
                path.display()
            ))
        })?
    {
        let publication =
            atomic_replace_if_unchanged(&path, Some(existing.as_bytes()), merged.as_bytes())
                .map_err(|error| {
                CoreError::Message(format!(
                    "cannot replace the installed Zed settings at {}: {error}; Fleety update is incomplete",
                    path.display()
                ))
            })?;
        println!(
            "Refreshed the Fleety ACP agent in Zed ({}).",
            crate::terminal_safe_text(&path.display().to_string())
        );
        if let AtomicPublication::PublishedWithCleanupWarning(warning) = publication {
            eprintln!(
                "warning: Zed settings were refreshed, but cleanup is incomplete: {}",
                crate::terminal_safe_text(&warning)
            );
        }
    }
    Ok(())
}

/// Pure refresh decision for Zed: return the updated JSON only when a Fleety entry
/// is already present AND re-pointing it at `command` changes something.
/// A settings file without the literal Fleety key is a no-op before strict
/// parsing, so unrelated JSONC is not allowed to make update fail. Once that
/// key is present, invalid existing JSON remains an error and an unchanged
/// entry returns `Ok(None)`.
pub fn refresh_zed_settings(
    existing: &str,
    command: &str,
    server: Option<&str>,
) -> std::result::Result<Option<String>, String> {
    // Zed settings are JSONC in practice. We only need to parse the file when
    // there is an installed Fleety entry to refresh; otherwise unrelated
    // comments/trailing commas must not make `fleety update` incomplete.
    if !existing.contains("\"Fleety\"") {
        return Ok(None);
    }
    let mut root = serde_json::from_str::<Value>(existing).map_err(|error| error.to_string())?;
    let Some(entry) = root
        .get_mut("agent_servers")
        .and_then(|servers| servers.get_mut("Fleety"))
    else {
        return Ok(None);
    };
    let entry = entry
        .as_object_mut()
        .ok_or_else(|| "the installed Fleety ACP entry is not a JSON object".to_string())?;
    if let Some(existing_server) = entry
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("FLEETY_AGENT_URL"))
    {
        let existing_server = existing_server.as_str().ok_or_else(|| {
            "the installed Fleety ACP FLEETY_AGENT_URL is not a string".to_string()
        })?;
        fleety_tools::connection::validate_ws_url(existing_server)
            .map_err(|error| error.report().message)?;
    }
    let mut changed = entry.get("command").and_then(Value::as_str) != Some(command);
    if changed {
        entry.insert("command".to_string(), json!(command));
    }
    if let Some(server) = server {
        let env = entry
            .entry("env")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| "the installed Fleety ACP env is not a JSON object".to_string())?;
        if env.get("FLEETY_AGENT_URL").and_then(Value::as_str) != Some(server) {
            env.remove("FLEETY_TOKEN");
        }
        if env.get("FLEETY_AGENT_URL").and_then(Value::as_str) != Some(server) {
            env.insert("FLEETY_AGENT_URL".to_string(), json!(server));
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    serde_json::to_string_pretty(&root)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Replace a settings file through a uniquely named temporary file in the same
/// directory. Keeping the temporary file beside the destination makes the
/// rename a single-filesystem operation; failures before that point leave the
/// existing settings untouched.
fn atomic_replace(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    atomic_replace_with(path, contents, |_| Ok(()))
}

fn atomic_replace_if_unchanged(
    path: &std::path::Path,
    expected: Option<&[u8]>,
    contents: &[u8],
) -> std::io::Result<AtomicPublication> {
    atomic_replace_if_unchanged_with(path, expected, contents, |_| Ok(()))
}

#[derive(Debug, PartialEq, Eq)]
enum AtomicPublication {
    Clean,
    PublishedWithCleanupWarning(String),
}

fn atomic_replace_if_unchanged_with(
    path: &std::path::Path,
    expected: Option<&[u8]>,
    contents: &[u8],
    after_displace: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<AtomicPublication> {
    atomic_replace_if_unchanged_with_cleanup(
        path,
        expected,
        contents,
        after_displace,
        |candidate| std::fs::remove_file(candidate),
    )
}

fn atomic_replace_if_unchanged_with_cleanup(
    path: &std::path::Path,
    expected: Option<&[u8]>,
    contents: &[u8],
    after_displace: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
    cleanup: impl FnMut(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<AtomicPublication> {
    atomic_replace_if_unchanged_with_hooks(
        path,
        expected,
        contents,
        after_displace,
        set_private_file_permissions,
        cleanup,
    )
}

fn atomic_replace_if_unchanged_with_hooks(
    path: &std::path::Path,
    expected: Option<&[u8]>,
    contents: &[u8],
    after_displace: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
    set_permissions: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    mut cleanup: impl FnMut(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<AtomicPublication> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".fleety-zed-{}.tmp", uuid::Uuid::new_v4()));
    let recovery = parent.join(format!(".fleety-zed-{}.recovery", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    if let Err(error) = set_permissions(&file) {
        drop(file);
        return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
    }
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
    }
    drop(file);

    let displaced = if let Some(expected) = expected {
        match std::fs::rename(path, &recovery) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(cleanup_temp_or_report(
                    std::io::Error::other(
                        "settings changed while Fleety was preparing the update; retry",
                    ),
                    &tmp,
                    &mut cleanup,
                ));
            }
            Err(error) => {
                return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) =
                std::fs::set_permissions(&recovery, std::fs::Permissions::from_mode(0o600))
            {
                let error = restore_displaced_or_report(error, &recovery, path);
                return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
            }
        }
        let actual = match std::fs::read(&recovery) {
            Ok(actual) => actual,
            Err(error) => {
                let error = restore_displaced_or_report(error, &recovery, path);
                return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
            }
        };
        if actual.as_slice() != expected {
            let error = restore_displaced_or_report(
                std::io::Error::other(
                    "settings changed while Fleety was preparing the update; retry",
                ),
                &recovery,
                path,
            );
            return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
        }
        Some(recovery.as_path())
    } else {
        None
    };

    if let Err(error) = after_displace(displaced.unwrap_or(tmp.as_path())) {
        if path.exists() {
            let error = if displaced.is_some() {
                std::io::Error::other(format!(
                    "{error}; the displaced settings remain as a recovery copy at {}",
                    recovery.display()
                ))
            } else {
                std::io::Error::other(format!(
                    "{error}; settings appeared while Fleety was preparing the update"
                ))
            };
            return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
        }
        if displaced.is_some() {
            let error = restore_displaced_or_report(error, &recovery, path);
            return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
        }
        return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
    }

    if let Err(error) = std::fs::hard_link(&tmp, path) {
        let path_was_recreated = path.exists();
        if !path_was_recreated && displaced.is_some() {
            let error = restore_displaced_or_report(error, &recovery, path);
            return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
        }
        let error = if path_was_recreated {
            if displaced.is_some() {
                std::io::Error::other(format!(
                    "settings changed during no-clobber publication: {error}; the displaced settings remain as a recovery copy at {}",
                    recovery.display()
                ))
            } else {
                std::io::Error::other(format!(
                    "settings appeared during no-clobber publication: {error}; the new file was preserved"
                ))
            }
        } else {
            error
        };
        return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
    }
    let mut cleanup_failures = Vec::new();
    if let Err(error) = cleanup(&tmp) {
        cleanup_failures.push(format!("{} ({error})", tmp.display()));
    }
    if displaced.is_some() {
        if let Err(error) = cleanup(&recovery) {
            cleanup_failures.push(format!("{} ({error})", recovery.display()));
        }
    }
    if cleanup_failures.is_empty() {
        Ok(AtomicPublication::Clean)
    } else {
        Ok(AtomicPublication::PublishedWithCleanupWarning(format!(
            "the new settings are active; remove the retained private file(s) after closing Zed: {}",
            cleanup_failures.join(", ")
        )))
    }
}

fn set_private_file_permissions(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

fn cleanup_temp_or_report(
    original: std::io::Error,
    tmp: &std::path::Path,
    cleanup: &mut impl FnMut(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Error {
    match cleanup(tmp) {
        Ok(()) => original,
        Err(error) => std::io::Error::other(format!(
            "{original}; a private temporary settings file could not be removed and remains at {}: {error}",
            tmp.display()
        )),
    }
}

enum DisplacedRestore {
    Clean,
    RetainedRecovery(std::io::Error),
}

fn restore_displaced_no_clobber_with_cleanup(
    recovery: &std::path::Path,
    path: &std::path::Path,
    cleanup: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<DisplacedRestore> {
    std::fs::hard_link(recovery, path)?;
    Ok(match cleanup(recovery) {
        Ok(()) => DisplacedRestore::Clean,
        Err(error) => DisplacedRestore::RetainedRecovery(error),
    })
}

fn restore_displaced_or_report(
    original: std::io::Error,
    recovery: &std::path::Path,
    path: &std::path::Path,
) -> std::io::Error {
    restore_displaced_or_report_with_cleanup(original, recovery, path, |candidate| {
        std::fs::remove_file(candidate)
    })
}

fn restore_displaced_or_report_with_cleanup(
    original: std::io::Error,
    recovery: &std::path::Path,
    path: &std::path::Path,
    cleanup: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Error {
    match restore_displaced_no_clobber_with_cleanup(recovery, path, cleanup) {
        Ok(DisplacedRestore::Clean) => original,
        Ok(DisplacedRestore::RetainedRecovery(cleanup)) => std::io::Error::other(format!(
            "{original}; canonical settings were restored, but recovery cleanup failed, so the retained copy remains at {}: {cleanup}",
            recovery.display()
        )),
        Err(restore) => std::io::Error::other(format!(
            "{original}; canonical settings could not be restored, so the displaced bytes remain at {}: {restore}",
            recovery.display()
        )),
    }
}

fn atomic_replace_with(
    path: &std::path::Path,
    contents: &[u8],
    before_replace: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    atomic_replace_with_cleanup(path, contents, before_replace, |candidate| {
        std::fs::remove_file(candidate)
    })
}

fn atomic_replace_with_cleanup(
    path: &std::path::Path,
    contents: &[u8],
    before_replace: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
    mut cleanup: impl FnMut(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".fleety-zed-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        before_replace(&tmp)?;
        std::fs::rename(&tmp, path)
    })();
    if let Err(error) = result {
        return Err(cleanup_temp_or_report(error, &tmp, &mut cleanup));
    }
    Ok(())
}

fn update_zed_settings_file(
    path: &std::path::Path,
    command: &str,
    server: Option<&str>,
) -> std::result::Result<ZedSettingsUpdate, String> {
    let (existing, expected) = match std::fs::read_to_string(path) {
        Ok(existing) => {
            let expected = Some(existing.as_bytes().to_vec());
            (existing, expected)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let (merged, updated) = merge_zed_settings(&existing, command, server)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    if !existing.trim().is_empty() {
        let backup = path.with_extension("json.bak");
        atomic_replace(&backup, existing.as_bytes())
            .map_err(|error| format!("cannot back up {}: {error}", backup.display()))?;
    }
    let publication = atomic_replace_if_unchanged(path, expected.as_deref(), merged.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    let cleanup_warning = match publication {
        AtomicPublication::Clean => None,
        AtomicPublication::PublishedWithCleanupWarning(warning) => Some(warning),
    };
    Ok(ZedSettingsUpdate {
        updated,
        cleanup_warning,
    })
}

#[derive(Debug)]
struct ZedSettingsUpdate {
    updated: bool,
    cleanup_warning: Option<String>,
}

/// This binary's path, for launching it as an ACP agent.
fn current_exe_str() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "fleety".to_string())
}

/// Dispatch `fleety acp install [<editor>]`. A known editor (currently `zed`) is
/// auto-configured; with no editor — or an unknown one — print the generic ACP
/// launch details that work with any ACP-capable client (Zed, JetBrains, neovim,
/// Emacs, …), since ACP is a shared protocol, not Zed-specific.
pub fn install(target: Option<String>, server: Option<String>) -> agent_core::Result<()> {
    if let Some(server) = server.as_deref() {
        fleety_tools::connection::validate_ws_url(server)?;
    }
    match target.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("zed") => install_zed(server),
        // An unsupported editor never reaches here: `unsupported_editor` turns
        // it into a usage error before install runs, because naming an editor
        // is a request to install for it and nothing gets installed.
        Some(other) => Err(agent_core::CoreError::Message(unsupported_editor_message(
            other,
        ))),
        None => {
            print_generic(server.as_deref());
            Ok(())
        }
    }
}

/// Print the editor-agnostic ACP setup: the command any ACP client launches.
fn print_generic(server: Option<&str>) {
    println!("{}", generic_setup(server));
}

/// The editors `install` can configure by itself.
const SUPPORTED_EDITORS: [&str; 1] = ["zed"];

/// `Some(usage message)` when `editor` names something this binary cannot
/// configure. `fleety acp install <editor>` used to print advice and exit 0, so
/// a script could not tell an install from a no-op.
pub fn unsupported_editor(editor: Option<&str>, server: Option<&str>) -> Option<String> {
    let editor = editor?.to_ascii_lowercase();
    if SUPPORTED_EDITORS.contains(&editor.as_str()) {
        return None;
    }
    Some(format!(
        "{}\n\n{}",
        unsupported_editor_message(&editor),
        generic_setup(server)
    ))
}

fn unsupported_editor_message(editor: &str) -> String {
    format!(
        "no built-in auto-config for editor '{}' (supported: {}); run `fleety acp install` with \
         no editor for the settings any ACP client needs",
        crate::terminal_safe_text(editor),
        SUPPORTED_EDITORS.join(", ")
    )
}

/// The editor-agnostic ACP setup: the command any ACP client launches.
fn generic_setup(server: Option<&str>) -> String {
    let cmd = current_exe_str();
    let env = match server {
        Some(s) => format!(
            "    env:     FLEETY_AGENT_URL={}\n\
             \x20            This endpoint is transient. Add a non-empty FLEETY_TOKEN for authentication,\n\
             \x20            or omit FLEETY_AGENT_URL to use the saved current profile.\n",
            crate::terminal_safe_endpoint(s)
        ),
        None => "    env:     (none — uses the saved current profile, then the trusted local default)\n"
            .to_string(),
    };
    format!(
        "Fleety is an ACP agent — point any ACP-capable editor at this command:\n\n\
         \x20   command: {}\n\
         \x20   args:    [\"acp\"]\n\
         {env}\n\
         Auto-configure a supported editor:\n\
         \x20   fleety acp install zed [--server ws://host:8787]\n\n\
         For other editors (JetBrains, neovim, Emacs, …), set their custom-ACP-agent\n\
         command to the above — ACP is a shared protocol, the same agent works for all.",
        crate::terminal_safe_text(&cmd)
    )
}

/// Register this binary as a custom ACP agent in Zed. Edits `settings.json` in
/// place (backing it up first); if it can't be parsed safely (JSONC comments),
/// prints the snippet to paste instead of clobbering it.
pub fn install_zed(server: Option<String>) -> agent_core::Result<()> {
    use agent_core::CoreError;
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot resolve this binary's path: {e}")))?;
    let command = exe.to_string_lossy().to_string();
    let snippet = serde_json::to_string_pretty(
        &json!({ "agent_servers": { "Fleety": fleety_agent_entry(&command, server.as_deref()) } }),
    )
    .unwrap_or_default();

    let Some(path) = zed_settings_path() else {
        println!(
            "Could not locate Zed's settings.json. Add this to it manually:\n\n{}",
            crate::terminal_safe_multiline(&snippet)
        );
        return Err(CoreError::Message(
            "Zed settings were not changed because the platform settings base is missing or not an absolute path"
                .to_string(),
        ));
    };
    match update_zed_settings_file(&path, &command, server.as_deref()) {
        Ok(update) => {
            let verb = if update.updated {
                "Updated"
            } else {
                "Configured"
            };
            println!(
                "{verb} the Fleety agent in Zed at {}.\nRestart Zed, then pick \"Fleety\" in the agent panel.\n\
                 (Agent binary: {})",
                crate::terminal_safe_text(&path.display().to_string()),
                crate::terminal_safe_text(&command)
            );
            if server.is_some() {
                println!(
                    "The configured FLEETY_AGENT_URL is transient. Add a non-empty FLEETY_TOKEN to Zed's Fleety environment for authentication, or reinstall without --server to use the saved current profile."
                );
            } else {
                println!("ACP will use Fleety's saved current profile when Zed starts the agent.");
            }
            if let Some(warning) = update.cleanup_warning {
                eprintln!(
                    "warning: Zed settings were updated, but cleanup is incomplete: {}",
                    crate::terminal_safe_text(&warning)
                );
            }
        }
        Err(e) => {
            println!(
                "Couldn't safely edit {} ({}).\nAdd this to your Zed settings.json manually:\n\n{}",
                crate::terminal_safe_text(&path.display().to_string()),
                crate::terminal_safe_text(&e),
                crate::terminal_safe_multiline(&snippet),
            );
            return Err(CoreError::Message(format!(
                "Zed settings were not changed: {}",
                crate::terminal_safe_text(&e)
            )));
        }
    }
    Ok(())
}

// ---- editor delegation: capability gating + tool→ACP-method mapping (pure) ----
//
// NOTE: the ACP method and capability field names below follow the Agent Client
// Protocol spec (agentclientprotocol.com). They are isolated here so a spec
// check only touches these constants/shapes.

#[allow(dead_code)] // consumed by acp-editor-delegation task 2.1 (bridge)
/// What the connected editor advertised it can serve, parsed from the ACP
/// `initialize` request's `clientCapabilities`. We only ever delegate what is
/// advertised.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditorCapabilities {
    pub read: bool,
    pub write: bool,
    pub terminal: bool,
}

/// Parse `clientCapabilities` from an `initialize` request's params.
#[allow(dead_code)]
pub fn parse_client_capabilities(init_params: &Value) -> EditorCapabilities {
    let caps = init_params.get("clientCapabilities");
    let fs = caps.and_then(|c| c.get("fs"));
    let b = |v: Option<&Value>| v.and_then(Value::as_bool).unwrap_or(false);
    EditorCapabilities {
        read: b(fs.and_then(|f| f.get("readTextFile"))),
        write: b(fs.and_then(|f| f.get("writeTextFile"))),
        terminal: b(caps.and_then(|c| c.get("terminal"))),
    }
}

/// The editor-backed tool names to advertise, given what the editor supports.
/// `editor_edit` needs both read and write (it is a read-modify-write).
#[allow(dead_code)]
pub fn editor_tool_names(caps: &EditorCapabilities) -> Vec<&'static str> {
    let mut names = Vec::new();
    if caps.read {
        names.push("editor_read_file");
    }
    if caps.write {
        names.push("editor_write_file");
    }
    if caps.read && caps.write {
        names.push("editor_edit");
    }
    if caps.terminal {
        names.push("editor_run");
    }
    names
}

/// Map an editor-backed tool call to the ACP client request (method, params) it
/// translates to. `editor_edit` is composed of a read + a write by the bridge,
/// so it has no single mapping here and returns `None`.
#[allow(dead_code)]
pub fn editor_request(session_id: &str, tool: &str, args: &Value) -> Option<(String, Value)> {
    let path = args.get("path").and_then(Value::as_str);
    match tool {
        "editor_read_file" => Some((
            "fs/read_text_file".to_string(),
            json!({ "sessionId": session_id, "path": path? }),
        )),
        "editor_write_file" => Some((
            "fs/write_text_file".to_string(),
            json!({
                "sessionId": session_id,
                "path": path?,
                "content": args.get("content").and_then(Value::as_str).unwrap_or("")
            }),
        )),
        "editor_run" => Some((
            "terminal/create".to_string(),
            json!({
                "sessionId": session_id,
                "command": args.get("command").and_then(Value::as_str).unwrap_or(""),
                "args": args.get("args").cloned().unwrap_or_else(|| json!([])),
                "cwd": args.get("cwd").cloned().unwrap_or(Value::Null)
            }),
        )),
        _ => None,
    }
}

/// The `editor_*` tool specs to advertise to the server (in Hello), gated by the
/// editor's capabilities. Their descriptions tell the agent to prefer them for
/// the user's files and how the surface differs (editor buffer, may be unsaved),
/// so the agent reasons correctly without a separate system-prompt change.
#[allow(dead_code)]
pub fn editor_tool_specs(caps: &EditorCapabilities) -> Vec<agent_core::ToolSpec> {
    use agent_core::{RiskLevel, ToolSpec};
    let pref =
        "Prefer this over the disk file tools for files the user is editing in this session.";
    let mut specs = Vec::new();
    if caps.read {
        specs.push(ToolSpec {
            name: "editor_read_file".to_string(),
            description: format!(
                "Read a file as the user's editor sees it (including unsaved buffer changes). {pref}"
            ),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
            risk: RiskLevel::Read,
        });
    }
    if caps.write {
        specs.push(ToolSpec {
            name: "editor_write_file".to_string(),
            description: format!(
                "Write a file through the user's editor — the change appears in their buffer (may be \
                 unsaved, pending their approval); disk-reading tools (git, search) won't see it \
                 until they save. {pref}"
            ),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] }),
            risk: RiskLevel::Mutate,
        });
    }
    if caps.read && caps.write {
        specs.push(ToolSpec {
            name: "editor_edit".to_string(),
            description: format!(
                "Edit a file through the user's editor: replace `old` with `new` (shows in their \
                 buffer/diff, may be unsaved). {pref}"
            ),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" }, "old": { "type": "string" }, "new": { "type": "string" } }, "required": ["path", "old", "new"] }),
            risk: RiskLevel::Mutate,
        });
    }
    if caps.terminal {
        specs.push(ToolSpec {
            name: "editor_run".to_string(),
            description: format!(
                "Run a command in the user's editor terminal (on the editor's host, in its cwd). \
                 Use for git/build/test/listing/etc. {pref}"
            ),
            parameters: json!({ "type": "object", "properties": { "command": { "type": "string" }, "cwd": { "type": "string" } }, "required": ["command"] }),
            risk: RiskLevel::Mutate,
        });
    }
    specs
}

// ---- dispatch + bridge ----

/// The server-facing side of the adapter, injectable so dispatch is testable
/// without a live socket.
#[async_trait::async_trait]
pub trait AcpBridge: Send + Sync {
    /// Open a conversation for a new session; returns its id (the ACP sessionId).
    async fn new_session(&self, cwd: Option<String>) -> agent_core::Result<String>;
    /// Run a prompt turn; returns the assistant text chunks to stream.
    async fn prompt(&self, session_id: &str, text: &str) -> agent_core::Result<Vec<String>>;
    /// Resume a session; returns its history as text chunks to replay.
    async fn load(&self, session_id: &str) -> agent_core::Result<Vec<String>>;

    /// Forward the editor's `session/cancel` to the server (best-effort — a
    /// cancel must never fail the adapter) and mark the session cancelled so
    /// an in-flight prompt closes with stopReason `"cancelled"`.
    async fn cancel(&self, _session_id: &str) {}

    /// Whether the session's current turn was cancelled; reading consumes the
    /// flag, and starting a new turn resets it, so one cancel affects exactly
    /// one prompt response.
    fn take_cancelled(&self, _session_id: &str) -> bool {
        false
    }

    /// Note the editor's advertised capabilities (from the `initialize` request)
    /// so the bridge can gate which `editor_*` tools it offers the server.
    fn note_capabilities(&self, _init_params: &Value) {}
}

/// Join an ACP prompt's content blocks into a single text string.
fn extract_prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .or_else(|| {
            params
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Handle one inbound ACP message, returning the JSON-RPC frames to send back
/// (responses and/or `session/update` notifications). Pure w.r.t. I/O — the
/// server interaction is behind `bridge`, so this is unit-testable.
pub async fn handle_message(msg: &Value, bridge: &dyn AcpBridge) -> Vec<Value> {
    // A frame with no `method` is a response or error from the editor (e.g. it
    // reporting a bad notification), not a request — ignore it rather than
    // replying "method not found".
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return vec![];
    };
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let reply_id = || id.clone().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            bridge.note_capabilities(&params);
            vec![response_ok(reply_id(), initialize_result())]
        }
        "authenticate" => vec![response_ok(reply_id(), json!({}))],
        "session/new" => {
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string);
            match bridge.new_session(cwd).await {
                Ok(sid) => vec![response_ok(reply_id(), json!({ "sessionId": sid }))],
                Err(e) => vec![response_err_report(reply_id(), INTERNAL_ERROR, &e.report())],
            }
        }
        "session/prompt" => {
            let sid = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let text = extract_prompt_text(&params);
            match bridge.prompt(&sid, &text).await {
                Ok(chunks) => {
                    let mut out: Vec<Value> =
                        chunks.iter().map(|c| assistant_update(&sid, c)).collect();
                    // A turn the user cancelled mid-flight closes with
                    // "cancelled"; reading the flag consumes it, so the
                    // session's next prompt is back to normal.
                    out.push(response_ok(
                        reply_id(),
                        json!({ "stopReason": stop_reason(bridge.take_cancelled(&sid)) }),
                    ));
                    out
                }
                Err(e) => vec![response_err_report(reply_id(), INTERNAL_ERROR, &e.report())],
            }
        }
        "session/load" => {
            let sid = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match bridge.load(&sid).await {
                Ok(chunks) => {
                    let mut out: Vec<Value> =
                        chunks.iter().map(|c| assistant_update(&sid, c)).collect();
                    out.push(response_ok(reply_id(), load_session_result()));
                    out
                }
                Err(e) => vec![response_err_report(reply_id(), INTERNAL_ERROR, &e.report())],
            }
        }
        // Cancel is a notification (no id → no response). Forward it to the
        // server as a CancelTurn frame and flag the session so an in-flight
        // prompt closes with stopReason "cancelled" once the server winds the
        // turn down; with no turn in flight the server ignores the frame
        // silently, and the stale flag is reset when the next turn starts.
        "session/cancel" => {
            let sid = params
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            bridge.cancel(sid).await;
            vec![]
        }
        _ => match id {
            Some(id) => vec![response_err(
                id,
                METHOD_NOT_FOUND,
                &format!("method not found: {method}"),
            )],
            None => vec![],
        },
    }
}

/// Read one framed JSON-RPC message from an async reader (`None` on EOF).
async fn read_frame_async<R: tokio::io::AsyncBufRead + Unpin>(
    r: &mut R,
) -> std::io::Result<FrameIn> {
    use tokio::io::AsyncBufReadExt;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await?;
        if n == 0 {
            return Ok(FrameIn::Eof);
        }
        let line = line.trim();
        if line.is_empty() {
            continue; // tolerate blank lines between messages
        }
        return Ok(match serde_json::from_str::<Value>(line) {
            Ok(v) => FrameIn::Message(v),
            Err(_) => FrameIn::Malformed,
        });
    }
}

/// Run ACP against one immutable connection resolution. Callers that resolve a
/// named/current profile must pass the complete snapshot so its URL and paired
/// token stay bound together for every bridge connection.
pub async fn run_resolved(target: fleety_tools::connection::Resolved) -> agent_core::Result<()> {
    // The stdin reader is shared: the main loop reads requests from it, and during
    // a prompt the bridge borrows it — to read the editor's responses to the
    // agent's fs/terminal calls, and to watch for a mid-turn `session/cancel`
    // (the editor is awaiting our prompt reply then, so stdin carries only those
    // frames — no contention).
    let reader: SharedReader = std::sync::Arc::new(tokio::sync::Mutex::new(
        tokio::io::BufReader::new(tokio::io::stdin()),
    ));
    let bridge = WsBridge::new(target, std::sync::Arc::clone(&reader));
    loop {
        let frame = {
            let mut r = reader.lock().await;
            read_frame_async(&mut *r).await
        };
        match frame {
            Ok(FrameIn::Message(msg)) => {
                let frames = handle_message(&msg, &bridge).await;
                let mut stdout = std::io::stdout();
                for f in frames {
                    if write_frame(&mut stdout, &f).is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(FrameIn::Malformed) => {
                // Reply with a JSON-RPC parse error and keep running, rather than
                // silently exiting on a bad line.
                let err = response_err(Value::Null, PARSE_ERROR, "parse error");
                let mut stdout = std::io::stdout();
                if write_frame(&mut stdout, &err).is_err() {
                    return Ok(());
                }
            }
            Ok(FrameIn::Eof) => return Ok(()), // editor closed
            Err(e) => {
                tracing::warn!(%e, "acp: stdin read error; exiting");
                return Ok(());
            }
        }
    }
}

/// A shared, lockable stdin reader (see [`run`]).
type SharedReader = std::sync::Arc<tokio::sync::Mutex<tokio::io::BufReader<tokio::io::Stdin>>>;

/// The Hello frame that opens any adapter→server connection.
fn hello_json(token: Option<&str>, local_tools_json: Option<String>) -> serde_json::Result<String> {
    serde_json::to_string(&fleety_protocol::ClientMsg::Hello {
        device_id: crate::device_id(),
        protocol: fleety_protocol::PROTOCOL_VERSION,
        token: token.filter(|s| !s.is_empty()).map(str::to_string),
        pairing_code: None,
        local_tools_json,
        hostname: fleety_tools::device::hostname(),
    })
}

/// Real bridge: each prompt opens a short-lived WebSocket to the server, sends
/// the user message rooted at the session's cwd, and collects the assistant
/// reply. Stateless per prompt (the server persists the conversation by id).
/// While a turn runs, the editor's input is also watched for `session/cancel`,
/// forwarded to the server as a CancelTurn frame on the turn's connection.
/// Generic over the editor-input reader (stdin in production, in-memory in
/// tests).
const ACP_WELCOME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const ACP_CANCEL_WELCOME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const ACP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const ACP_CANCEL_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Open an authenticated ACP session on whichever saved endpoint answers.
///
/// ACP used to dial `target.url()` directly with a bare WebSocket, which made it
/// the one client surface that could neither roam onto a saved alternative nor
/// open the encrypted channel. It now goes through the same handshake as every
/// other surface, so the policy and the per-candidate deadline are identical.
async fn open_acp_session(
    target: &fleety_tools::connection::Resolved,
    local_tools_json: Option<String>,
    wait: std::time::Duration,
) -> agent_core::Result<(
    fleety_tools::transport::Sender,
    fleety_tools::transport::Receiver,
    fleety_tools::connection::Resolved,
)> {
    fleety_tools::connection::connect_first_healthy(target, wait, move |session| {
        let local_tools_json = local_tools_json.clone();
        async move {
            let fleety_tools::connection::CandidateSession {
                connection,
                target,
                sealed,
            } = session;
            let (mut tx, mut rx) = connection.split();
            let hello = hello_json(target.token(), local_tools_json)
                .map_err(|e| agent_core::CoreError::Message(format!("serialize hello: {e}")))?;
            tx.send_text(hello).await?;
            let committed = receive_authenticated_welcome(&mut rx, &target, sealed).await?;
            Ok((tx, rx, committed))
        }
    })
    .await
}

/// The caller bounds this: `connect_first_healthy` gives each candidate one
/// deadline covering the connect, the handshake, and this reply together.
async fn receive_authenticated_welcome(
    rx: &mut fleety_tools::transport::Receiver,
    target: &fleety_tools::connection::Resolved,
    sealed: bool,
) -> agent_core::Result<fleety_tools::connection::Resolved> {
    use agent_core::CoreError;

    let frame = rx.recv_text().await.ok_or_else(|| {
        CoreError::Message("the Server closed before authenticating the ACP session".to_string())
    })?;
    let welcome = serde_json::from_str::<fleety_protocol::ServerMsg>(&frame).ok();
    match welcome {
        Some(fleety_protocol::ServerMsg::Welcome {
            protocol,
            server_fingerprint,
            server_endpoints,
            ..
        }) if protocol == fleety_protocol::PROTOCOL_VERSION => {
            crate::verify_and_learn_welcome_identity(
                server_fingerprint.as_deref(),
                &server_endpoints,
                target,
                sealed,
            )
        }
        Some(fleety_protocol::ServerMsg::Welcome { protocol, .. }) => Err(CoreError::Message(
            format!(
                "the Server uses incompatible protocol {protocol}; this client requires {} — update all Fleety binaries to the same release",
                fleety_protocol::PROTOCOL_VERSION
            ),
        )),
        other => Err(CoreError::Message(crate::hello_failure_message_for_target(
            other.as_ref(),
            target,
        ))),
    }
}

struct WsBridge<R> {
    target: std::sync::Mutex<fleety_tools::connection::Resolved>,
    cwds: tokio::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
    /// Sessions whose current turn the editor cancelled (`session/cancel`).
    /// Set when a cancel is seen, consumed by `take_cancelled`, reset when a
    /// new turn starts — so one cancel affects exactly one prompt.
    cancelled: std::sync::Mutex<std::collections::HashSet<String>>,
    /// The editor's advertised capabilities (from `initialize`), gating which
    /// `editor_*` tools we offer the server.
    caps: std::sync::Mutex<EditorCapabilities>,
    /// Shared editor-input reader, for the editor's fs/terminal responses and
    /// the mid-turn `session/cancel` watch.
    reader: std::sync::Arc<tokio::sync::Mutex<R>>,
    /// JSON-RPC request id counter for our calls to the editor.
    next_req: std::sync::atomic::AtomicI64,
}

impl<R: tokio::io::AsyncBufRead + Unpin + Send> WsBridge<R> {
    fn new(
        target: fleety_tools::connection::Resolved,
        reader: std::sync::Arc<tokio::sync::Mutex<R>>,
    ) -> Self {
        Self {
            target: std::sync::Mutex::new(target),
            cwds: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            cancelled: std::sync::Mutex::new(std::collections::HashSet::new()),
            caps: std::sync::Mutex::new(EditorCapabilities::default()),
            reader,
            next_req: std::sync::atomic::AtomicI64::new(1),
        }
    }

    /// Whether the editor cancelled `session_id`'s current turn.
    fn session_cancelled(&self, session_id: &str) -> bool {
        self.cancelled
            .lock()
            .map(|c| c.contains(session_id))
            .unwrap_or(false)
    }

    /// Call one ACP client method on the editor and await its response. Borrows
    /// the shared stdin reader (free during a prompt). Frames that aren't our
    /// response (notifications, a mid-prompt request) are skipped.
    async fn editor_call(&self, method: &str, params: Value) -> agent_core::Result<Value> {
        use agent_core::CoreError;
        let id = self
            .next_req
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        {
            let mut so = std::io::stdout();
            write_frame(&mut so, &req)
                .map_err(|e| CoreError::Message(format!("write editor request: {e}")))?;
        }
        let mut reader = self.reader.lock().await;
        loop {
            match read_frame_async(&mut *reader).await {
                Ok(FrameIn::Message(v)) => {
                    if v.get("id").and_then(Value::as_i64) == Some(id) {
                        if let Some(err) = v.get("error") {
                            let m = err
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("error");
                            return Err(CoreError::Message(format!("editor: {m}")));
                        }
                        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // Not our response. A `session/cancel` seen here (while a
                    // tool or approval was in flight) must not be lost: flag
                    // its session — the turn loop forwards a CancelTurn to the
                    // server once this editor call returns. Anything else is
                    // dropped, as before.
                    if v.get("method").and_then(Value::as_str) == Some("session/cancel") {
                        if let Some(sid) = v
                            .get("params")
                            .and_then(|p| p.get("sessionId"))
                            .and_then(Value::as_str)
                        {
                            if let Ok(mut c) = self.cancelled.lock() {
                                c.insert(sid.to_string());
                            }
                        }
                    }
                }
                Ok(FrameIn::Malformed) => {
                    // Skip a malformed line while awaiting the editor's response.
                }
                Ok(FrameIn::Eof) => {
                    return Err(CoreError::Message("editor connection closed".to_string()))
                }
                Err(e) => return Err(CoreError::Message(format!("read editor response: {e}"))),
            }
        }
    }

    /// Execute one editor-backed tool by translating it to ACP client calls.
    /// Results carry a `surface` (and `saved` for writes) so the agent knows the
    /// change is in the editor's buffer, not yet on disk.
    async fn dispatch_editor(
        &self,
        session_id: &str,
        tool: &str,
        args: &Value,
    ) -> agent_core::Result<Value> {
        use agent_core::CoreError;
        match tool {
            "editor_read_file" => {
                let (m, p) = editor_request(session_id, tool, args).ok_or_else(|| {
                    CoreError::Message("editor_read_file needs 'path'".to_string())
                })?;
                let r = self.editor_call(&m, p).await?;
                Ok(
                    json!({ "surface": "editor-buffer", "content": r.get("content").cloned().unwrap_or(Value::Null) }),
                )
            }
            "editor_write_file" => {
                let (m, p) = editor_request(session_id, tool, args).ok_or_else(|| {
                    CoreError::Message("editor_write_file needs 'path'".to_string())
                })?;
                self.editor_call(&m, p).await?;
                Ok(
                    json!({ "surface": "editor-buffer", "saved": false, "applied": true, "path": args.get("path").cloned().unwrap_or(Value::Null) }),
                )
            }
            "editor_edit" => {
                let path = args
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CoreError::Message("editor_edit needs 'path'".to_string()))?;
                let old = args.get("old").and_then(Value::as_str).unwrap_or("");
                let new = args.get("new").and_then(Value::as_str).unwrap_or("");
                let read = self
                    .editor_call(
                        "fs/read_text_file",
                        json!({ "sessionId": session_id, "path": path }),
                    )
                    .await?;
                let content = read.get("content").and_then(Value::as_str).unwrap_or("");
                if !old.is_empty() && !content.contains(old) {
                    return Err(CoreError::Message(format!(
                        "editor_edit: `old` text not found in {path}"
                    )));
                }
                let updated = content.replacen(old, new, 1);
                self.editor_call(
                    "fs/write_text_file",
                    json!({ "sessionId": session_id, "path": path, "content": updated }),
                )
                .await?;
                Ok(
                    json!({ "surface": "editor-buffer", "saved": false, "applied": true, "path": path }),
                )
            }
            "editor_run" => {
                let (m, p) = editor_request(session_id, tool, args)
                    .ok_or_else(|| CoreError::Message("editor_run needs 'command'".to_string()))?;
                let created = self.editor_call(&m, p).await?;
                let term = created
                    .get("terminalId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let by = json!({ "sessionId": session_id, "terminalId": term });
                let _ = self.editor_call("terminal/wait_for_exit", by.clone()).await;
                let out = self.editor_call("terminal/output", by.clone()).await?;
                let _ = self.editor_call("terminal/release", by).await;
                Ok(json!({
                    "surface": "editor-terminal",
                    "output": out.get("output").cloned().unwrap_or(Value::Null),
                    "exitStatus": out.get("exitStatus").cloned().unwrap_or(Value::Null)
                }))
            }
            other => Err(CoreError::Message(format!("unknown editor tool '{other}'"))),
        }
    }

    /// Connect, Hello, send one UserMessage for `conversation`, collect the
    /// assistant texts until Done.
    async fn run_turn(
        &self,
        conversation: &str,
        text: &str,
        cwd: Option<String>,
        resume: bool,
    ) -> agent_core::Result<Vec<String>> {
        use agent_core::CoreError;

        // A stale cancel (e.g. one that arrived while idle) must not poison
        // this turn: the flag only reflects cancels seen while it runs.
        if let Ok(mut c) = self.cancelled.lock() {
            c.remove(conversation);
        }

        let target = self
            .target
            .lock()
            .map_err(|_| CoreError::Message("ACP connection state is unavailable".to_string()))?
            .clone();
        fleety_tools::connection::validate_resolved_profile_before_transport(&target)?;
        // Advertise the editor-backed tools gated by what the editor supports, so
        // the server offers the agent `editor_*` tools routed back to us.
        let editor_specs = self
            .caps
            .lock()
            .map(|c| editor_tool_specs(&c))
            .unwrap_or_default();
        let local_tools_json = if editor_specs.is_empty() {
            None
        } else {
            serde_json::to_string(&editor_specs).ok()
        };
        let (mut tx, mut rx, committed_target) = open_acp_session(
            &target,
            local_tools_json,
            ACP_CONNECT_TIMEOUT + ACP_WELCOME_TIMEOUT,
        )
        .await?;
        *self
            .target
            .lock()
            .map_err(|_| CoreError::Message("ACP connection state is unavailable".to_string()))? =
            committed_target;

        let outbound = if resume {
            serde_json::to_string(&fleety_protocol::ClientMsg::Resume {
                conversation_id: conversation.to_string(),
                after_seq: 0,
            })
        } else {
            serde_json::to_string(&fleety_protocol::ClientMsg::UserMessage {
                message_id: uuid::Uuid::new_v4().to_string(),
                conversation_id: Some(conversation.to_string()),
                text: text.to_string(),
                origin: cwd_to_origin(cwd.as_deref()),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            })
        }
        .map_err(|e| CoreError::Message(format!("serialize message: {e}")))?;
        tx.send_text(outbound)
            .await
            .map_err(|e| CoreError::Provider(format!("send message: {e}")))?;

        let mut chunks = Vec::new();
        // Mid-turn cancellation (design decision seven): while the turn runs,
        // this task is the editor's only reader (the dispatch loop is blocked
        // awaiting our prompt reply), so the editor's `session/cancel` has to
        // be picked up here, alongside the server socket. A seen cancel flags
        // the session and is forwarded as one CancelTurn frame on THIS
        // connection; the server acks, stops at its next checkpoint, and ends
        // the turn with Done — which ends this loop normally, and the flag
        // turns the prompt's stop reason into "cancelled".
        enum Race {
            Server(Option<String>),
            EditorReady(bool),
        }
        let mut cancel_sent = false;
        let mut editor_open = true; // stop watching the editor after EOF/error
        loop {
            // Forward a cancel flagged for this conversation — by the editor
            // watch below, or by editor_call while a tool/approval ran.
            if !cancel_sent && self.session_cancelled(conversation) {
                cancel_sent = true;
                if let Ok(t) = serde_json::to_string(&fleety_protocol::ClientMsg::CancelTurn {
                    conversation_id: Some(conversation.to_string()),
                }) {
                    let _ = tx.send_text(t).await;
                }
            }
            // Race the server socket against editor input. The editor side
            // only signals readiness (`fill_buf`, which is cancellation-safe);
            // the frame is read after the race is decided, so a concurrent
            // server frame can never cost us a half-read line.
            let server_frame = if editor_open {
                let mut r = self.reader.lock().await;
                let race = {
                    use tokio::io::AsyncBufReadExt;
                    tokio::select! {
                        f = rx.recv_text() => Race::Server(f),
                        b = r.fill_buf() => Race::EditorReady(matches!(b, Ok(x) if !x.is_empty())),
                    }
                };
                match race {
                    Race::Server(f) => Some(f),
                    Race::EditorReady(false) => {
                        editor_open = false; // editor input closed/errored
                        None
                    }
                    Race::EditorReady(true) => {
                        match read_frame_async(&mut *r).await {
                            Ok(FrameIn::Message(v))
                                if v.get("method").and_then(Value::as_str)
                                    == Some("session/cancel") =>
                            {
                                // One turn is in flight per connection, so a
                                // mid-turn cancel targets this conversation.
                                if let Ok(mut c) = self.cancelled.lock() {
                                    c.insert(conversation.to_string());
                                }
                            }
                            Ok(FrameIn::Message(_) | FrameIn::Malformed) => {
                                tracing::warn!(
                                    "acp: dropped a mid-turn editor frame (only session/cancel is handled during a turn)"
                                );
                            }
                            Ok(FrameIn::Eof) | Err(_) => editor_open = false,
                        }
                        None
                    }
                }
            } else {
                Some(rx.recv_text().await)
            };
            let Some(next) = server_frame else { continue };
            let Some(text) = next else {
                return Err(CoreError::Message(
                    "the Server disconnected before completing the ACP turn".to_string(),
                ));
            };
            let msg = serde_json::from_str::<fleety_protocol::ServerMsg>(&text).map_err(|_| {
                CoreError::Message(
                    "the Server sent a malformed protocol message during the ACP turn".to_string(),
                )
            })?;
            match msg {
                fleety_protocol::ServerMsg::Welcome { .. } => {
                    return Err(CoreError::Message(
                        "the Server sent a duplicate Welcome; the ACP session was closed"
                            .to_string(),
                    ))
                }
                fleety_protocol::ServerMsg::Assistant {
                    conversation_id,
                    text,
                    ..
                } if conversation_id == conversation => chunks.push(text),
                fleety_protocol::ServerMsg::Replay {
                    conversation_id,
                    content,
                    ..
                } if conversation_id == conversation => chunks.push(content),
                fleety_protocol::ServerMsg::Done { conversation_id }
                    if conversation_id == conversation =>
                {
                    break;
                }
                fleety_protocol::ServerMsg::Assistant { .. }
                | fleety_protocol::ServerMsg::Replay { .. }
                | fleety_protocol::ServerMsg::Done { .. } => {
                    return Err(CoreError::Message(
                        "the Server sent an ACP reply for a different conversation".to_string(),
                    ))
                }
                fleety_protocol::ServerMsg::Error { error } => {
                    return Err(CoreError::Message(error.message))
                }
                // The agent invoked an editor-backed tool: run it via the editor's
                // ACP fs/terminal methods and reply with the result.
                fleety_protocol::ServerMsg::RunTool {
                    call_id,
                    tool,
                    args_json,
                } => {
                    let args: Value =
                        serde_json::from_str(&args_json).unwrap_or_else(|_| json!({}));
                    let reply = match self.dispatch_editor(conversation, &tool, &args).await {
                        Ok(v) => fleety_protocol::ClientMsg::ToolResult {
                            call_id,
                            result_json: v.to_string(),
                        },
                        Err(e) => fleety_protocol::ClientMsg::ToolError {
                            call_id,
                            error: fleety_protocol::WireError {
                                kind: "editor".to_string(),
                                message: e.report().message,
                                remediation: None,
                            },
                        },
                    };
                    let text = serde_json::to_string(&reply).map_err(|error| {
                        CoreError::Message(format!("serialize tool reply: {error}"))
                    })?;
                    tx.send_text(text).await.map_err(|error| {
                        CoreError::Provider(format!("send tool reply: {error}"))
                    })?;
                }
                // The server wants approval for a tool: ask the editor via ACP
                // session/request_permission, then relay the user's choice back.
                fleety_protocol::ServerMsg::ApprovalRequested {
                    approval_id,
                    tool,
                    summary,
                    ..
                } => {
                    let params = permission_request(conversation, &approval_id, &tool, &summary);
                    let allow = self
                        .editor_call("session/request_permission", params)
                        .await
                        .ok()
                        .and_then(|v| {
                            v.get("outcome")
                                .and_then(|o| o.get("optionId"))
                                .and_then(Value::as_str)
                                .map(|opt| opt == "allow")
                        })
                        .unwrap_or(false); // error / cancel → deny (fail safe)
                    let reply = if allow {
                        fleety_protocol::ClientMsg::Approve { approval_id }
                    } else {
                        fleety_protocol::ClientMsg::Deny { approval_id }
                    };
                    let text = serde_json::to_string(&reply).map_err(|error| {
                        CoreError::Message(format!("serialize approval reply: {error}"))
                    })?;
                    tx.send_text(text).await.map_err(|error| {
                        CoreError::Provider(format!("send approval reply: {error}"))
                    })?;
                }
                _ => {}
            }
        }
        let _ = tx.close().await;
        Ok(chunks)
    }
}

#[async_trait::async_trait]
impl<R: tokio::io::AsyncBufRead + Unpin + Send> AcpBridge for WsBridge<R> {
    async fn new_session(&self, cwd: Option<String>) -> agent_core::Result<String> {
        let sid = uuid::Uuid::new_v4().to_string();
        self.cwds.lock().await.insert(sid.clone(), cwd);
        Ok(sid)
    }

    async fn prompt(&self, session_id: &str, text: &str) -> agent_core::Result<Vec<String>> {
        let cwd = self.cwds.lock().await.get(session_id).cloned().flatten();
        self.run_turn(session_id, text, cwd, false).await
    }

    async fn load(&self, session_id: &str) -> agent_core::Result<Vec<String>> {
        self.run_turn(session_id, "", None, true).await
    }

    async fn cancel(&self, session_id: &str) {
        // Flag first: were a turn somehow in flight for this session, its
        // loop would forward the cancel on the live connection.
        if !session_id.is_empty() {
            if let Ok(mut c) = self.cancelled.lock() {
                c.insert(session_id.to_string());
            }
        }
        // This dispatch path only runs between turns (the loop is
        // sequential; a mid-turn cancel is picked up inside run_turn), so
        // there is no live turn connection here: best-effort send CancelTurn
        // on a short-lived one — an idle server ignores it silently, by
        // design. A cancel never fails the adapter.
        let target = match self.target.lock() {
            Ok(target) => target.clone(),
            Err(_) => return,
        };
        if let Err(error) =
            fleety_tools::connection::validate_resolved_profile_before_transport(&target)
        {
            tracing::warn!(
                error = %crate::terminal_safe_text(&error.report().message),
                "acp: cancel: saved profile changed; nothing to cancel"
            );
            return;
        }
        let opened = open_acp_session(
            &target,
            None,
            ACP_CANCEL_CONNECT_TIMEOUT + ACP_CANCEL_WELCOME_TIMEOUT,
        )
        .await;
        let (mut tx, _rx, committed_target) = match opened {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(
                    endpoint = %crate::terminal_safe_endpoint(target.url()),
                    error = %crate::terminal_safe_text(&error.report().message),
                    "acp: cancel: server unreachable; nothing to cancel"
                );
                return;
            }
        };
        if let Ok(mut stored) = self.target.lock() {
            *stored = committed_target;
        }
        let conversation_id = (!session_id.is_empty()).then(|| session_id.to_string());
        if let Ok(cancel) =
            serde_json::to_string(&fleety_protocol::ClientMsg::CancelTurn { conversation_id })
        {
            let _ = tx.send_text(cancel).await;
        }
        tx.close().await;
    }

    fn take_cancelled(&self, session_id: &str) -> bool {
        self.cancelled
            .lock()
            .map(|mut c| c.remove(session_id))
            .unwrap_or(false)
    }

    fn note_capabilities(&self, init_params: &Value) {
        if let Ok(mut c) = self.caps.lock() {
            *c = parse_client_capabilities(init_params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let msg = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg).unwrap();
        // The wire form is one JSON object on a single line, newline-terminated
        // (ACP transport), with no Content-Length header and no embedded newline.
        let text = String::from_utf8_lossy(&buf);
        assert!(!text.contains("Content-Length"));
        assert!(text.ends_with('\n'));
        assert_eq!(text.trim_end().matches('\n').count(), 0);
        // ...and reads back to the same value.
        let mut cur = Cursor::new(buf);
        let got = read_frame(&mut cur).unwrap().unwrap();
        assert_eq!(got, msg);
        // A second read at EOF yields None.
        assert!(read_frame(&mut cur).unwrap().is_none());
    }

    #[test]
    fn malformed_line_is_none_not_panic() {
        // A line that isn't JSON → None here (the async runtime replies with a
        // JSON-RPC parse error and keeps going).
        let mut cur = Cursor::new(b"not json\n".to_vec());
        assert!(read_frame(&mut cur).unwrap().is_none());
        // Blank lines between messages are tolerated.
        let mut cur = Cursor::new(b"\n\n{\"jsonrpc\":\"2.0\",\"id\":9}\n".to_vec());
        assert_eq!(read_frame(&mut cur).unwrap().unwrap()["id"], json!(9));
    }

    #[test]
    fn message_builders_shape() {
        let ok = response_ok(json!(1), json!({"a":1}));
        assert_eq!(ok["jsonrpc"], "2.0");
        assert_eq!(ok["id"], json!(1));
        assert_eq!(ok["result"]["a"], 1);
        let err = response_err(json!(2), METHOD_NOT_FOUND, "nope");
        assert_eq!(err["error"]["code"], METHOD_NOT_FOUND);
        let report = agent_core::CoreError::ConnectionStoreIncompatible {
            path: "/tmp/connections.toml".to_string(),
            reason: "writer marker missing".to_string(),
        }
        .report();
        let classified = response_err_report(json!(3), INTERNAL_ERROR, &report);
        assert_eq!(
            classified["error"]["data"]["kind"],
            "connection_store_incompatible"
        );
        assert!(classified["error"]["data"]["remediation"]
            .as_str()
            .is_some_and(|value| value.contains("fleety init <ws-url>")));
        let note = notification("session/update", json!({"x":1}));
        assert!(note.get("id").is_none());
        assert_eq!(note["method"], "session/update");
    }

    struct MockBridge;
    #[async_trait::async_trait]
    impl AcpBridge for MockBridge {
        async fn new_session(&self, _cwd: Option<String>) -> agent_core::Result<String> {
            Ok("sess-1".to_string())
        }
        async fn prompt(&self, _session_id: &str, _text: &str) -> agent_core::Result<Vec<String>> {
            Ok(vec!["hello".to_string(), " world".to_string()])
        }
        async fn load(&self, _session_id: &str) -> agent_core::Result<Vec<String>> {
            Ok(vec!["replayed".to_string()])
        }
    }

    #[tokio::test]
    async fn dispatch_routes_methods() {
        let b = MockBridge;
        // initialize → one response with capabilities.
        let r = handle_message(&json!({"id":1,"method":"initialize","params":{}}), &b).await;
        assert_eq!(r.len(), 1);
        assert!(r[0]["result"]["agentCapabilities"]["loadSession"]
            .as_bool()
            .unwrap());
        // session/new → response with sessionId.
        let r = handle_message(
            &json!({"id":2,"method":"session/new","params":{"cwd":"/p"}}),
            &b,
        )
        .await;
        assert_eq!(r[0]["result"]["sessionId"], "sess-1");
        // session/prompt → streamed updates + final stopReason.
        let r = handle_message(
            &json!({"id":3,"method":"session/prompt","params":{"sessionId":"sess-1","prompt":[{"type":"text","text":"hi"}]}}),
            &b,
        )
        .await;
        assert_eq!(r.len(), 3, "two chunks + one response");
        assert_eq!(r[0]["method"], "session/update");
        assert_eq!(
            r[0]["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert_eq!(r[0]["params"]["update"]["content"]["text"], "hello");
        assert_eq!(r[2]["result"]["stopReason"], "end_turn");
        // unknown request → method-not-found error.
        let r = handle_message(&json!({"id":9,"method":"frobnicate","params":{}}), &b).await;
        assert_eq!(r[0]["error"]["code"], METHOD_NOT_FOUND);
        // unknown notification (no id) → nothing.
        let r = handle_message(&json!({"method":"frobnicate"}), &b).await;
        assert!(r.is_empty());
        // cancel notification → nothing.
        let r = handle_message(
            &json!({"method":"session/cancel","params":{"sessionId":"s"}}),
            &b,
        )
        .await;
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn session_load_replays_then_returns_conformant_response() {
        let b = MockBridge; // load replays a single "replayed" chunk
        let r = handle_message(
            &json!({"id":7,"method":"session/load","params":{"sessionId":"sess-1"}}),
            &b,
        )
        .await;
        // The history replay (session/update notifications) comes first...
        assert_eq!(r.len(), 2, "one replay update + one response");
        assert_eq!(r[0]["method"], "session/update");
        assert_eq!(
            r[0]["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert_eq!(r[0]["params"]["update"]["content"]["text"], "replayed");
        // ...then the load response, addressed to the request id.
        assert_eq!(r[1]["jsonrpc"], "2.0");
        assert_eq!(r[1]["id"], json!(7));
        // It is a deliberately-built ACP LoadSessionResponse, not an accidental
        // empty object: it carries the `modes` field (null — no session modes).
        let result = r[1]["result"].as_object().expect("result is an object");
        assert!(
            result.contains_key("modes"),
            "load response must be the LoadSessionResponse shape, got {result:?}"
        );
        assert_eq!(r[1]["result"]["modes"], Value::Null);
        assert_eq!(r[1]["result"], load_session_result());
    }

    #[test]
    fn capabilities_gate_editor_tools() {
        // Full capabilities → all four editor tools.
        let full = parse_client_capabilities(&json!({
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true }, "terminal": true }
        }));
        assert_eq!(
            full,
            EditorCapabilities {
                read: true,
                write: true,
                terminal: true
            }
        );
        assert_eq!(
            editor_tool_names(&full),
            vec![
                "editor_read_file",
                "editor_write_file",
                "editor_edit",
                "editor_run"
            ]
        );
        // Read-only, no terminal → just the reader.
        let ro = parse_client_capabilities(&json!({
            "clientCapabilities": { "fs": { "readTextFile": true } }
        }));
        assert_eq!(editor_tool_names(&ro), vec!["editor_read_file"]);
        // Nothing advertised → no editor tools.
        assert!(editor_tool_names(&parse_client_capabilities(&json!({}))).is_empty());
    }

    #[test]
    fn editor_tool_specs_gated_by_capabilities() {
        let full = EditorCapabilities {
            read: true,
            write: true,
            terminal: true,
        };
        let names: Vec<String> = editor_tool_specs(&full)
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert_eq!(
            names,
            vec![
                "editor_read_file",
                "editor_write_file",
                "editor_edit",
                "editor_run"
            ]
        );
        // Read-only editor → only the reader; descriptions steer the agent to prefer it.
        let ro = EditorCapabilities {
            read: true,
            write: false,
            terminal: false,
        };
        let ro_specs = editor_tool_specs(&ro);
        assert_eq!(ro_specs.len(), 1);
        assert_eq!(ro_specs[0].name, "editor_read_file");
        assert!(ro_specs[0].description.to_lowercase().contains("prefer"));
        // No capabilities → no editor tools.
        assert!(editor_tool_specs(&EditorCapabilities::default()).is_empty());
    }

    #[test]
    fn editor_request_maps_to_acp_methods() {
        let (m, p) = editor_request("s1", "editor_read_file", &json!({ "path": "a.rs" })).unwrap();
        assert_eq!(m, "fs/read_text_file");
        assert_eq!(p["sessionId"], "s1");
        assert_eq!(p["path"], "a.rs");
        let (m, p) = editor_request(
            "s1",
            "editor_write_file",
            &json!({ "path": "a.rs", "content": "x" }),
        )
        .unwrap();
        assert_eq!(m, "fs/write_text_file");
        assert_eq!(p["content"], "x");
        let (m, p) =
            editor_request("s1", "editor_run", &json!({ "command": "git status" })).unwrap();
        assert_eq!(m, "terminal/create");
        assert_eq!(p["command"], "git status");
        // editor_edit is composed (read+write), no single mapping.
        assert!(editor_request("s1", "editor_edit", &json!({ "path": "a.rs" })).is_none());
        // A read without a path → None (caller surfaces an error).
        assert!(editor_request("s1", "editor_read_file", &json!({})).is_none());
    }

    #[test]
    fn prompt_text_joins_blocks() {
        let p = json!({"prompt":[{"type":"text","text":"a"},{"type":"text","text":"b"}]});
        assert_eq!(extract_prompt_text(&p), "ab");
    }

    #[test]
    fn zed_agent_entry_and_merge() {
        // Entry shape: custom command + acp arg; server → FLEETY_AGENT_URL env.
        let e = fleety_agent_entry("/bin/fleety", Some("ws://host:8787"));
        assert_eq!(e["type"], "custom");
        assert_eq!(e["command"], "/bin/fleety");
        assert_eq!(e["args"][0], "acp");
        assert_eq!(e["env"]["FLEETY_AGENT_URL"], "ws://host:8787");
        // No server → no env var.
        let e2 = fleety_agent_entry("/bin/fleety", None);
        assert!(e2["env"].as_object().unwrap().is_empty());

        // Merge into empty settings → a fresh add (not an update).
        let (out, updated) = merge_zed_settings("", "/bin/fleety", None).expect("merge empty");
        assert!(!updated, "empty settings → fresh add");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["agent_servers"]["Fleety"]["command"], "/bin/fleety");

        // Merge preserves other keys and other agent servers; adds Fleety.
        let existing =
            r#"{"theme":"dark","agent_servers":{"Other":{"type":"custom","command":"x"}}}"#;
        let (out, updated) =
            merge_zed_settings(existing, "/new/fleety", None).expect("merge existing");
        assert!(!updated, "no prior Fleety entry → add");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["agent_servers"]["Other"]["command"], "x");
        assert_eq!(v["agent_servers"]["Fleety"]["command"], "/new/fleety");

        // Re-running over an existing Fleety entry overwrites it (update path).
        let (out, updated) = merge_zed_settings(&out, "/updated/fleety", None).expect("re-merge");
        assert!(updated, "existing Fleety entry → update");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["agent_servers"]["Fleety"]["command"], "/updated/fleety");
        assert_eq!(
            v["agent_servers"]["Other"]["command"], "x",
            "other agents preserved"
        );

        let with_token = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{"FLEETY_AGENT_URL":"ws://old","FLEETY_TOKEN":"keep-token","EDITOR_FLAG":"keep"}}}}"#;
        let (out, updated) =
            merge_zed_settings(with_token, "/updated/fleety", Some("ws://new")).expect("merge");
        assert!(updated);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["agent_servers"]["Fleety"]["env"]
                .get("FLEETY_TOKEN")
                .is_none(),
            "changing endpoint must clear its bound token"
        );
        assert_eq!(v["agent_servers"]["Fleety"]["env"]["EDITOR_FLAG"], "keep");
        assert_eq!(
            v["agent_servers"]["Fleety"]["env"]["FLEETY_AGENT_URL"],
            "ws://new"
        );

        let same_endpoint = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{"FLEETY_AGENT_URL":"ws://same","FLEETY_TOKEN":"same-token","EDITOR_FLAG":"keep"}}}}"#;
        let (out, _) =
            merge_zed_settings(same_endpoint, "/updated/fleety", Some("ws://same")).expect("merge");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["agent_servers"]["Fleety"]["env"]["FLEETY_TOKEN"], "same-token",
            "reinstalling the same transient endpoint keeps its explicit token"
        );

        let (out, _) = merge_zed_settings(same_endpoint, "/updated/fleety", None).expect("merge");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["agent_servers"]["Fleety"]["env"]
                .get("FLEETY_TOKEN")
                .is_none(),
            "returning to saved-profile resolution must remove the raw endpoint token"
        );
        let token_without_endpoint = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{"FLEETY_TOKEN":"orphaned-token","EDITOR_FLAG":"keep"}}}}"#;
        let (out, _) =
            merge_zed_settings(token_without_endpoint, "/updated/fleety", None).expect("merge");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["agent_servers"]["Fleety"]["env"]
                .get("FLEETY_TOKEN")
                .is_none(),
            "saved-profile mode must clear a token that has no endpoint binding"
        );
        assert_eq!(v["agent_servers"]["Fleety"]["env"]["EDITOR_FLAG"], "keep");

        // JSONC with comments is refused (never clobbered).
        assert!(
            merge_zed_settings("// my settings\n{\"theme\":\"dark\"}", "/bin/fleety", None)
                .is_err()
        );
    }

    #[test]
    fn hello_uses_the_resolved_profile_token() {
        let hello = hello_json(Some("paired-profile-token"), None).expect("serialize hello");
        let msg: fleety_protocol::ClientMsg =
            serde_json::from_str(&hello).expect("deserialize hello");
        assert!(matches!(
            msg,
            fleety_protocol::ClientMsg::Hello {
                token: Some(token),
                ..
            } if token == "paired-profile-token"
        ));
    }

    #[test]
    fn refresh_only_repoints_when_already_installed() {
        // Not installed → no refresh (never newly installs).
        assert_eq!(
            refresh_zed_settings(r#"{"theme":"dark"}"#, "/bin/fleety", None).unwrap(),
            None
        );
        assert_eq!(refresh_zed_settings("", "/bin/fleety", None).unwrap(), None);
        // Unrelated JSONC is ignored when Fleety is not installed.
        let unrelated_jsonc = r#"// Zed settings
{
  "agent_servers": {"opencode": {"type": "registry"}},
  "theme": {"dark": "One Dark",},
}"#;
        assert_eq!(
            refresh_zed_settings(unrelated_jsonc, "/bin/fleety", None).unwrap(),
            None
        );
        // Unparseable JSONC with an installed Fleety entry remains protected.
        let jsonc_with_fleety = "// c\n{\"agent_servers\":{\"Fleety\":{}}}";
        assert!(refresh_zed_settings(jsonc_with_fleety, "/bin/fleety", None).is_err());
        let invalid_endpoint = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{"FLEETY_AGENT_URL":"wss://example.test/ws#fragment"}}}}"#;
        assert!(
            refresh_zed_settings(invalid_endpoint, "/new/fleety", None).is_err(),
            "update must not preserve an endpoint that current Fleety would refuse"
        );

        // Installed at an old path → refresh re-points it at the current binary.
        let old = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{}}}}"#;
        let refreshed = refresh_zed_settings(old, "/new/fleety", None)
            .expect("valid settings")
            .expect("should refresh");
        let v: Value = serde_json::from_str(&refreshed).unwrap();
        assert_eq!(v["agent_servers"]["Fleety"]["command"], "/new/fleety");

        // Already pointing at the current binary → nothing changes → no rewrite.
        assert_eq!(
            refresh_zed_settings(&refreshed, "/new/fleety", None).unwrap(),
            None
        );

        // A path-only refresh preserves the complete installed entry except
        // for `command`, including its server binding and editor-specific env.
        let bound = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp","--editor-mode"],"env":{"FLEETY_AGENT_URL":"wss://paired.example/ws","FLEETY_TOKEN":"old-token","EDITOR_FLAG":"keep"},"extra":"keep"}}}"#;
        let refreshed = refresh_zed_settings(bound, "/new/fleety", None)
            .expect("valid settings")
            .expect("refresh");
        let before: Value = serde_json::from_str(bound).unwrap();
        let after: Value = serde_json::from_str(&refreshed).unwrap();
        assert_eq!(
            after["agent_servers"]["Fleety"]["env"],
            before["agent_servers"]["Fleety"]["env"]
        );
        assert_eq!(
            after["agent_servers"]["Fleety"]["args"],
            before["agent_servers"]["Fleety"]["args"]
        );
        assert_eq!(after["agent_servers"]["Fleety"]["extra"], "keep");
        assert_eq!(after["agent_servers"]["Fleety"]["command"], "/new/fleety");

        // An explicit replacement changes only the server binding while
        // retaining unrelated editor env values.
        let rebound = refresh_zed_settings(&refreshed, "/new/fleety", Some("wss://new/ws"))
            .expect("valid settings")
            .expect("explicit server replacement");
        let rebound: Value = serde_json::from_str(&rebound).unwrap();
        assert_eq!(
            rebound["agent_servers"]["Fleety"]["env"]["FLEETY_AGENT_URL"],
            "wss://new/ws"
        );
        assert_eq!(
            rebound["agent_servers"]["Fleety"]["env"]["EDITOR_FLAG"],
            "keep"
        );
        assert!(
            rebound["agent_servers"]["Fleety"]["env"]
                .get("FLEETY_TOKEN")
                .is_none(),
            "changing the refreshed endpoint must clear its bound token"
        );
    }

    #[test]
    fn settings_base_must_be_nonempty_and_absolute() {
        assert!(absolute_env_path(None).is_none());
        assert!(absolute_env_path(Some(std::ffi::OsString::from(""))).is_none());
        assert!(absolute_env_path(Some(std::ffi::OsString::from("relative"))).is_none());
        #[cfg(not(windows))]
        assert_eq!(
            absolute_env_path(Some(std::ffi::OsString::from("/absolute"))),
            Some(std::path::PathBuf::from("/absolute"))
        );
        #[cfg(windows)]
        assert_eq!(
            absolute_env_path(Some(std::ffi::OsString::from(r"C:\absolute"))),
            Some(std::path::PathBuf::from(r"C:\absolute"))
        );
    }

    #[test]
    fn zed_replace_atomically_replaces_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("fleety-acp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, b"old").unwrap();

        atomic_replace(&path, b"new").expect("replace existing settings");

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "temporary file must not remain after success"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zed_replace_rejects_content_drift() {
        let dir = std::env::temp_dir().join(format!("fleety-acp-drift-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, b"{\"theme\":\"old\"}").unwrap();
        let expected = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"{\"theme\":\"newer\"}").unwrap();

        let error = atomic_replace_if_unchanged(
            &path,
            Some(expected.as_slice()),
            b"{\"agent_servers\":{}}",
        )
        .expect_err("drift must abort replacement");
        assert!(error.to_string().contains("settings changed"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"theme\":\"newer\"}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zed_replace_never_clobbers_a_path_recreated_during_publication() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acp-recreate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = b"{\"theme\":\"old\"}";
        std::fs::write(&path, original).unwrap();

        let error = atomic_replace_if_unchanged_with(
            &path,
            Some(original),
            b"{\"agent_servers\":{}}",
            |_| {
                std::fs::write(&path, b"{\"theme\":\"concurrent\"}")?;
                Ok(())
            },
        )
        .expect_err("a concurrent path owner must win without being overwritten");

        assert!(
            error.to_string().contains("recovery copy"),
            "error must identify recoverable partial state: {error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"theme\":\"concurrent\"}");
        let recovery = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().contains(".recovery"))
            .expect("the displaced original must remain recoverable");
        assert_eq!(std::fs::read(recovery.path()).unwrap(), original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn displaced_permission_or_read_failure_reports_recovery_when_restore_fails() {
        for cause in ["injected chmod failure", "injected recovery read failure"] {
            let dir = std::env::temp_dir().join(format!(
                "fleety-acp-restore-report-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("settings.json");
            let recovery = dir.join(".fleety-zed-test.recovery");
            std::fs::write(&path, b"concurrent owner").unwrap();
            std::fs::write(&recovery, b"displaced secret settings").unwrap();

            let error = restore_displaced_or_report(std::io::Error::other(cause), &recovery, &path);
            let message = error.to_string();

            assert!(message.contains(cause), "{message}");
            assert!(
                message.contains(&recovery.display().to_string()),
                "{message}"
            );
            assert!(message.contains("could not be restored"), "{message}");
            assert_eq!(
                std::fs::read(&recovery).unwrap(),
                b"displaced secret settings"
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"concurrent owner");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn displaced_restore_reports_canonical_active_when_only_cleanup_fails() {
        let dir = std::env::temp_dir().join(format!(
            "fleety-acp-restore-cleanup-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let recovery = dir.join(".fleety-zed-test.recovery");
        std::fs::write(&recovery, b"displaced secret settings").unwrap();

        let error = restore_displaced_or_report_with_cleanup(
            std::io::Error::other("injected read failure"),
            &recovery,
            &path,
            |_| Err(std::io::Error::other("injected unlink failure")),
        );
        let message = error.to_string();

        assert!(
            message.contains("canonical settings were restored"),
            "{message}"
        );
        assert!(message.contains("recovery cleanup failed"), "{message}");
        assert!(!message.contains("could not be restored"), "{message}");
        assert_eq!(std::fs::read(&path).unwrap(), b"displaced secret settings");
        assert_eq!(
            std::fs::read(&recovery).unwrap(),
            b"displaced secret settings"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prepublication_failure_reports_a_retained_private_temp_file() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acp-prepublish-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let error = atomic_replace_if_unchanged_with_cleanup(
            &path,
            None,
            b"{\"FLEETY_TOKEN\":\"secret\"}",
            |_| Err(std::io::Error::other("injected publication failure")),
            |_| Err(std::io::Error::other("injected temp unlink failure")),
        )
        .expect_err("prepublication cleanup failure must be explicit");
        let message = error.to_string();

        assert!(
            message.contains("injected publication failure"),
            "{message}"
        );
        assert!(
            message.contains("private temporary settings file"),
            "{message}"
        );
        let retained = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .expect("reported temp file remains");
        assert!(message.contains(&retained.path().display().to_string()));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn permission_failure_reports_a_retained_private_temp_file() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acp-permission-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let error = atomic_replace_if_unchanged_with_hooks(
            &path,
            None,
            b"{\"FLEETY_TOKEN\":\"secret\"}",
            |_| Ok(()),
            |_| Err(std::io::Error::other("injected permission failure")),
            |_| Err(std::io::Error::other("injected temp unlink failure")),
        )
        .expect_err("permission cleanup failure must be explicit");
        let message = error.to_string();

        assert!(message.contains("injected permission failure"), "{message}");
        assert!(
            message.contains("private temporary settings file"),
            "{message}"
        );
        let retained = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .expect("reported temp file remains");
        assert!(message.contains(&retained.path().display().to_string()));
        assert_eq!(std::fs::read(retained.path()).unwrap(), b"");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn backup_replace_failure_reports_a_retained_private_temp_file() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acp-backup-temp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json.bak");

        let error = atomic_replace_with_cleanup(
            &path,
            b"{\"FLEETY_TOKEN\":\"secret\"}",
            |_| Err(std::io::Error::other("injected backup publication failure")),
            |_| Err(std::io::Error::other("injected backup temp unlink failure")),
        )
        .expect_err("backup cleanup failure must be explicit");
        let message = error.to_string();

        assert!(
            message.contains("injected backup publication failure"),
            "{message}"
        );
        assert!(
            message.contains("private temporary settings file"),
            "{message}"
        );
        let retained = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .expect("reported backup temp remains");
        assert!(message.contains(&retained.path().display().to_string()));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zed_replace_reports_published_when_temp_cleanup_fails() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acp-tmp-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = b"{\"theme\":\"old\"}";
        std::fs::write(&path, original).unwrap();

        let publication = atomic_replace_if_unchanged_with_cleanup(
            &path,
            Some(original),
            b"{\"agent_servers\":{}}",
            |_| Ok(()),
            |candidate| {
                if candidate
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".tmp"))
                {
                    Err(std::io::Error::other("injected temp cleanup failure"))
                } else {
                    std::fs::remove_file(candidate)
                }
            },
        )
        .expect("publication remains successful after cleanup failure");

        let AtomicPublication::PublishedWithCleanupWarning(warning) = publication else {
            panic!("cleanup failure must be explicit");
        };
        assert!(warning.contains(".tmp"));
        assert!(warning.contains("new settings are active"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"agent_servers\":{}}");
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp")),
            "the warning must name a retained file that actually exists"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zed_replace_reports_published_when_recovery_cleanup_fails() {
        let dir = std::env::temp_dir().join(format!(
            "fleety-acp-recovery-cleanup-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = b"{\"theme\":\"old\"}";
        std::fs::write(&path, original).unwrap();

        let publication = atomic_replace_if_unchanged_with_cleanup(
            &path,
            Some(original),
            b"{\"agent_servers\":{}}",
            |_| Ok(()),
            |candidate| {
                if candidate
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".recovery"))
                {
                    Err(std::io::Error::other("injected recovery cleanup failure"))
                } else {
                    std::fs::remove_file(candidate)
                }
            },
        )
        .expect("publication remains successful after cleanup failure");

        let AtomicPublication::PublishedWithCleanupWarning(warning) = publication else {
            panic!("cleanup failure must be explicit");
        };
        assert!(warning.contains(".recovery"));
        assert!(warning.contains("new settings are active"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"agent_servers\":{}}");
        let recovery = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().ends_with(".recovery"))
            .expect("the warning must name a retained recovery file");
        assert_eq!(std::fs::read(recovery.path()).unwrap(), original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_zed_replace_keeps_the_existing_file() {
        let dir = std::env::temp_dir().join(format!("fleety-acp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = br#"{"agent_servers":{"Fleety":{"command":"old"}}}"#;
        std::fs::write(&path, original).unwrap();

        let result = atomic_replace_with(&path, b"replacement", |_| {
            Err(std::io::Error::other("injected before replace"))
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "temporary file must be cleaned up"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zed_install_refuses_to_replace_unreadable_settings_bytes() {
        let dir = std::env::temp_dir().join(format!("fleety-acp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create settings dir");
        let path = dir.join("settings.json");
        let original = [0xff, 0xfe, 0xfd];
        std::fs::write(&path, original).expect("write invalid utf-8 settings");

        let error = update_zed_settings_file(&path, "/bin/fleety", Some("ws://new:8787"))
            .expect_err("unreadable text settings must fail closed");
        assert!(error.contains("cannot read"));
        assert_eq!(
            std::fs::read(&path).expect("read preserved settings"),
            original
        );
        assert!(!path.with_extension("json.bak").exists());

        std::fs::remove_dir_all(dir).expect("remove settings fixture");
    }

    #[test]
    fn zed_install_refuses_to_report_success_when_backup_fails() {
        let dir = std::env::temp_dir().join(format!("fleety-acp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create settings dir");
        let path = dir.join("settings.json");
        let original = br#"{"theme":"keep"}"#;
        std::fs::write(&path, original).expect("write existing settings");
        std::fs::create_dir(path.with_extension("json.bak"))
            .expect("block backup with a directory");

        let error = update_zed_settings_file(&path, "/bin/fleety", Some("ws://new:8787"))
            .expect_err("backup failure must fail the install");
        assert!(error.contains("cannot back up"));
        assert_eq!(
            std::fs::read(&path).expect("read preserved settings"),
            original
        );

        std::fs::remove_dir_all(dir).expect("remove settings fixture");
    }

    #[cfg(unix)]
    #[test]
    fn zed_install_keeps_token_bearing_settings_and_backup_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("fleety-acp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create settings dir");
        let path = dir.join("settings.json");
        let original = br#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{"FLEETY_AGENT_URL":"ws://same:8787","FLEETY_TOKEN":"private-token"}}}}"#;
        std::fs::write(&path, original).expect("write settings");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("secure source settings");

        update_zed_settings_file(&path, "/new/fleety", Some("ws://same:8787"))
            .expect("update settings");

        for protected in [&path, &path.with_extension("json.bak")] {
            assert_eq!(
                std::fs::metadata(protected)
                    .expect("protected file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{} must remain private",
                protected.display()
            );
            assert!(String::from_utf8_lossy(
                &std::fs::read(protected).expect("read protected file")
            )
            .contains("private-token"));
        }

        std::fs::remove_dir_all(dir).expect("remove settings fixture");
    }

    #[test]
    fn mappings_are_well_formed() {
        let u = assistant_update("s1", "hello");
        assert_eq!(u["method"], "session/update");
        assert_eq!(u["params"]["sessionId"], "s1");
        // ACP SessionUpdate: tagged by `sessionUpdate`, carrying a ContentBlock.
        assert_eq!(
            u["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert_eq!(u["params"]["update"]["content"]["type"], "text");
        assert_eq!(u["params"]["update"]["content"]["text"], "hello");
        let p = permission_request("s1", "appr-1", "write_file", "edit foo");
        assert_eq!(p["sessionId"], "s1");
        assert_eq!(p["toolCall"]["toolCallId"], "appr-1");
        assert_eq!(p["toolCall"]["title"], "write_file");
        assert!(p["options"].as_array().is_some());
        assert_eq!(stop_reason(false), "end_turn");
        assert_eq!(stop_reason(true), "cancelled");
        let origin = cwd_to_origin(Some("/home/alice/proj"));
        assert_eq!(origin.cwd.as_deref(), Some("/home/alice/proj"));
        let origin_none = cwd_to_origin(None);
        assert_eq!(origin_none.cwd, None);
        assert!(initialize_result()["agentCapabilities"]["loadSession"]
            .as_bool()
            .unwrap());
    }

    // ---- turn cancellation (design decision seven) ----

    use fleety_protocol::{ClientMsg, ServerMsg};

    /// Bridge double pinning the cancel dispatch semantics `handle_message`
    /// relies on: a new turn starts with a clean flag, a cancel during the
    /// turn sets it, and `take_cancelled` consumes it.
    #[derive(Default)]
    struct CancelMock {
        forwarded: std::sync::Mutex<Vec<String>>,
        flagged: std::sync::Mutex<std::collections::HashSet<String>>,
        cancel_during_prompt: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl AcpBridge for CancelMock {
        async fn new_session(&self, _cwd: Option<String>) -> agent_core::Result<String> {
            Ok("sess-c".to_string())
        }
        async fn prompt(&self, session_id: &str, _text: &str) -> agent_core::Result<Vec<String>> {
            let mut flagged = self.flagged.lock().unwrap();
            flagged.remove(session_id); // a new turn starts clean
            if self
                .cancel_during_prompt
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                flagged.insert(session_id.to_string()); // cancel landed mid-turn
            }
            Ok(vec!["chunk".to_string()])
        }
        async fn load(&self, _session_id: &str) -> agent_core::Result<Vec<String>> {
            Ok(vec![])
        }
        async fn cancel(&self, session_id: &str) {
            self.forwarded.lock().unwrap().push(session_id.to_string());
            self.flagged.lock().unwrap().insert(session_id.to_string());
        }
        fn take_cancelled(&self, session_id: &str) -> bool {
            self.flagged.lock().unwrap().remove(session_id)
        }
    }

    fn prompt_msg(sid: &str) -> Value {
        json!({"id":11,"method":"session/prompt","params":{"sessionId":sid,"prompt":[{"type":"text","text":"go"}]}})
    }

    #[tokio::test]
    async fn session_cancel_dispatch_forwards_and_parameterizes_stop_reason() {
        let b = CancelMock::default();
        // session/cancel is a notification: forwarded to the bridge, no reply.
        let r = handle_message(
            &json!({"method":"session/cancel","params":{"sessionId":"sess-c"}}),
            &b,
        )
        .await;
        assert!(r.is_empty(), "a notification gets no response");
        assert_eq!(*b.forwarded.lock().unwrap(), vec!["sess-c".to_string()]);
        // An idle-time cancel does not poison the next prompt (reset at turn
        // start — the fixed behavior for the no-prompt-in-flight case).
        let r = handle_message(&prompt_msg("sess-c"), &b).await;
        assert_eq!(r.last().unwrap()["result"]["stopReason"], "end_turn");
        // A cancel landing while the turn runs → stopReason "cancelled"...
        b.cancel_during_prompt
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let r = handle_message(&prompt_msg("sess-c"), &b).await;
        assert_eq!(r.last().unwrap()["result"]["stopReason"], "cancelled");
        // ...consumed with the response: the next prompt is normal again.
        b.cancel_during_prompt
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let r = handle_message(&prompt_msg("sess-c"), &b).await;
        assert_eq!(r.last().unwrap()["result"]["stopReason"], "end_turn");
    }

    /// Scripted WS server (the cli_smoke pattern, multi-connection): per
    /// connection, each step reads one client frame then sends its responses;
    /// each connection's received frames are delivered on the channel.
    fn scripted_server(
        conns: Vec<Vec<Vec<ServerMsg>>>,
    ) -> (String, std::sync::mpsc::Receiver<Vec<ClientMsg>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind script server");
        let addr = listener.local_addr().expect("script server addr");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for steps in conns {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let Ok(mut ws) = tokio_tungstenite::tungstenite::accept(stream) else {
                    return;
                };
                let mut received = Vec::new();
                for responses in steps {
                    let Ok(frame) = ws.read() else { break };
                    if let Ok(text) = frame.to_text() {
                        if let Ok(msg) = serde_json::from_str::<ClientMsg>(text) {
                            received.push(msg);
                        }
                    }
                    for r in responses {
                        let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(
                            serde_json::to_string(&r).expect("serialize server msg"),
                        ));
                    }
                }
                let _ = ws.close(None);
                let _ = tx.send(received);
            }
        });
        (format!("ws://{addr}"), rx)
    }

    fn assistant(conv: &str, text: &str, seq: u64) -> ServerMsg {
        ServerMsg::Assistant {
            conversation_id: conv.to_string(),
            text: text.to_string(),
            seq,
            speech: None,
            attention: None,
        }
    }

    fn acp_welcome() -> ServerMsg {
        ServerMsg::Welcome {
            session_id: "acp-session".to_string(),
            conversation_id: "acp-conversation".to_string(),
            protocol: fleety_protocol::PROTOCOL_VERSION,
            server_version: String::new(),
            audio_input: false,
            config_protocol: 0,
            server_fingerprint: Some("acp-server".to_string()),
            server_endpoints: Vec::new(),
            loopback_trusted: false,
            token: None,
        }
    }

    #[tokio::test]
    async fn bridge_hello_uses_the_resolved_profile_token() {
        let (url, server_rx) = scripted_server(vec![vec![
            vec![acp_welcome()],
            vec![ServerMsg::Done {
                conversation_id: "c-token".to_string(),
            }],
        ]]);
        let (editor_in, _editor_out) = tokio::io::duplex(1024);
        let bridge = WsBridge::new(
            fleety_tools::connection::Resolved::unowned(
                url,
                Some("paired-profile-token".to_string()),
                fleety_tools::connection::Source::Profile("paired".to_string()),
            ),
            std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::BufReader::new(
                editor_in,
            ))),
        );

        bridge
            .run_turn("c-token", "hello", None, false)
            .await
            .expect("turn completes");

        let received = server_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("server frames");
        assert!(matches!(
            received.first(),
            Some(ClientMsg::Hello {
                token: Some(token),
                ..
            }) if token == "paired-profile-token"
        ));
    }

    #[tokio::test]
    async fn bridge_rejects_control_before_welcome_without_sending_user_data() {
        let pre_welcome_control = ServerMsg::RunTool {
            call_id: "pre-welcome-call".to_string(),
            tool: "unknown-pre-welcome-tool".to_string(),
            args_json: "{}".to_string(),
        };
        let rejection = ServerMsg::Error {
            error: fleety_protocol::WireError {
                kind: "unauthenticated".to_string(),
                message: "pair first".to_string(),
                remediation: None,
            },
        };
        let (url, server_rx) =
            scripted_server(vec![vec![vec![pre_welcome_control], vec![rejection]]]);
        let (bridge, _editor_out) = duplex_bridge(&url);

        assert!(
            bridge
                .run_turn("c-pre-welcome", "private editor prompt", None, false)
                .await
                .is_err(),
            "a control frame cannot authenticate an ACP session"
        );

        let received = server_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("server frames");
        assert!(
            matches!(received.as_slice(), [ClientMsg::Hello { .. }]),
            "ACP sent user data or a control reply before Welcome: {received:?}"
        );
    }

    #[tokio::test]
    async fn bridge_rejects_partial_output_when_server_disconnects_before_done() {
        let (url, server_rx) = scripted_server(vec![vec![
            vec![acp_welcome()],
            vec![assistant("c-partial", "unfinished", 1)],
        ]]);
        let (bridge, _editor_out) = duplex_bridge(&url);

        let error = bridge
            .run_turn("c-partial", "do work", None, false)
            .await
            .expect_err("disconnect before Done must not become end_turn");
        assert!(
            error
                .report()
                .message
                .contains("disconnected before completing"),
            "{}",
            error.report().message
        );
        let received = server_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("server frames");
        assert!(matches!(
            received.as_slice(),
            [ClientMsg::Hello { .. }, ClientMsg::UserMessage { .. }]
        ));
    }

    #[tokio::test]
    async fn bridge_rejects_malformed_protocol_after_partial_output() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind malformed server");
        let addr = listener.local_addr().expect("malformed server addr");
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept malformed client");
            let mut ws =
                tokio_tungstenite::tungstenite::accept(stream).expect("upgrade malformed client");
            let _hello = ws.read().expect("read hello");
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&acp_welcome()).expect("serialize welcome"),
            ))
            .expect("send welcome");
            let _user = ws.read().expect("read user message");
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&assistant("c-malformed", "unfinished", 1))
                    .expect("serialize assistant"),
            ))
            .expect("send assistant");
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                "{not protocol json".to_string(),
            ))
            .expect("send malformed protocol");
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&ServerMsg::Done {
                    conversation_id: "c-malformed".to_string(),
                })
                .expect("serialize done"),
            ))
            .expect("send done");
        });
        let (bridge, _editor_out) = duplex_bridge(&format!("ws://{addr}"));

        let error = bridge
            .run_turn("c-malformed", "do work", None, false)
            .await
            .expect_err("malformed protocol must reject the partial turn");
        assert!(
            error.report().message.contains("malformed protocol"),
            "{}",
            error.report().message
        );
    }

    /// A real WsBridge whose editor input is an in-memory duplex; returns the
    /// write half the test uses to play the editor.
    fn duplex_bridge(
        url: &str,
    ) -> (
        WsBridge<tokio::io::BufReader<tokio::io::DuplexStream>>,
        tokio::io::DuplexStream,
    ) {
        let (editor_in, editor_out) = tokio::io::duplex(1024);
        let bridge = WsBridge::new(
            fleety_tools::connection::Resolved::unowned(
                url.to_string(),
                None,
                fleety_tools::connection::Source::OverrideUrl,
            ),
            std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::BufReader::new(
                editor_in,
            ))),
        );
        (bridge, editor_out)
    }

    /// The editor's stop gesture, end to end against the real bridge: a
    /// `session/cancel` arriving while the turn runs is written to the turn's
    /// server connection as a CancelTurn frame, and the prompt closes with
    /// stopReason "cancelled" once the server's wind-down and Done arrive.
    #[tokio::test]
    async fn mid_turn_session_cancel_sends_cancel_turn_and_prompt_stops_cancelled() {
        let (url, server_rx) = scripted_server(vec![vec![
            vec![acp_welcome()], // Hello
            vec![],              // UserMessage — the server now waits for the CancelTurn
            vec![
                // CancelTurn → ack, wind-down, Done (decision five's shape).
                assistant("c-mid", "cancelling — stopping at the next safe point", 1),
                assistant(
                    "c-mid",
                    "Cancelled at your request — work completed so far is preserved.",
                    2,
                ),
                ServerMsg::Done {
                    conversation_id: "c-mid".to_string(),
                },
            ],
        ]]);
        let (bridge, mut editor_out) = duplex_bridge(&url);
        // The stop gesture: a cancel notification sitting on the editor input
        // while the prompt turn runs.
        tokio::io::AsyncWriteExt::write_all(
            &mut editor_out,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"c-mid\"}}\n",
        )
        .await
        .unwrap();
        let frames = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            handle_message(
                &json!({"id":5,"method":"session/prompt","params":{"sessionId":"c-mid","prompt":[{"type":"text","text":"do it"}]}}),
                &bridge,
            ),
        )
        .await
        .expect("cancelled prompt must still complete");
        // The wind-down still streams to the editor...
        let texts: Vec<&str> = frames
            .iter()
            .filter_map(|f| f["params"]["update"]["content"]["text"].as_str())
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t.contains("Cancelled at your request")),
            "wind-down chunk missing: {texts:?}"
        );
        // ...the response closes with "cancelled" (not end_turn)...
        assert_eq!(frames.last().unwrap()["result"]["stopReason"], "cancelled");
        // ...and the server connection saw the CancelTurn for this conversation.
        let received = server_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("server frames");
        assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
        assert!(matches!(
            received.get(1),
            Some(ClientMsg::UserMessage { .. })
        ));
        assert!(matches!(
            received.get(2),
            Some(ClientMsg::CancelTurn { conversation_id: Some(c) }) if c == "c-mid"
        ));
        // One cancel affects exactly one prompt: the flag was consumed.
        assert!(!bridge.take_cancelled("c-mid"));
    }

    /// A cancel with no prompt in flight is still forwarded (the idle server
    /// ignores it silently, by design) and leaves no state behind: the next
    /// prompt ends normally with end_turn.
    #[tokio::test]
    async fn idle_session_cancel_forwards_and_leaves_next_prompt_normal() {
        let (url, server_rx) = scripted_server(vec![
            // Connection 1 — the idle cancel: Hello, CancelTurn.
            vec![vec![acp_welcome()], vec![]],
            // Connection 2 — the next prompt: Hello, UserMessage → reply+Done.
            vec![
                vec![acp_welcome()],
                vec![
                    assistant("c-idle", "fresh answer", 1),
                    ServerMsg::Done {
                        conversation_id: "c-idle".to_string(),
                    },
                ],
            ],
        ]);
        let (bridge, _editor_out) = duplex_bridge(&url);
        let frames = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            handle_message(
                &json!({"method":"session/cancel","params":{"sessionId":"c-idle"}}),
                &bridge,
            ),
        )
        .await
        .expect("idle cancel must not hang");
        assert!(frames.is_empty(), "cancel is a notification — no response");
        let conn1 = server_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("idle-cancel frames");
        assert!(matches!(conn1.first(), Some(ClientMsg::Hello { .. })));
        assert!(matches!(
            conn1.get(1),
            Some(ClientMsg::CancelTurn { conversation_id: Some(c) }) if c == "c-idle"
        ));
        // The stale flag is reset when the next turn starts: end_turn.
        let frames = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            handle_message(
                &json!({"id":6,"method":"session/prompt","params":{"sessionId":"c-idle","prompt":[{"type":"text","text":"hi"}]}}),
                &bridge,
            ),
        )
        .await
        .expect("prompt after idle cancel must complete");
        assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    }

    /// An endpoint that upgrades and then says nothing must not hold the turn
    /// open. The bound now lives in the shared handshake driver, which is the
    /// same one every other client surface uses.
    #[tokio::test]
    async fn a_silent_endpoint_cannot_hold_an_acp_session_open() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind silent server");
        let address = listener.local_addr().expect("silent server address");
        let silent = tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            use futures::StreamExt;
            let _ = ws.next().await;
            std::future::pending::<()>().await;
        });
        let target = fleety_tools::connection::Resolved::unowned(
            format!("ws://{address}"),
            None,
            fleety_tools::connection::Source::OverrideUrl,
        );

        let outcome = open_acp_session(&target, None, std::time::Duration::from_millis(200)).await;
        silent.abort();
        let Err(error) = outcome else {
            panic!("a silent endpoint must not hold the session open");
        };
        assert!(
            error
                .report()
                .message
                .contains("never completed the handshake"),
            "{}",
            error.report().message
        );
    }

    /// A stalled upgrade is bounded too, and the failure never echoes the
    /// credentials or query values embedded in the endpoint.
    #[tokio::test]
    async fn a_stalled_upgrade_is_bounded_and_never_echoes_endpoint_secrets() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stalled upgrade server");
        let address = listener.local_addr().expect("stalled server address");
        let stalled = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept ACP connection");
            std::future::pending::<()>().await;
        });
        let target = fleety_tools::connection::Resolved::unowned(
            format!("ws://user:password@{address}/acp?token=secret#fragment"),
            None,
            fleety_tools::connection::Source::OverrideUrl,
        );

        let outcome = open_acp_session(&target, None, std::time::Duration::from_millis(200)).await;
        stalled.abort();
        let Err(error) = outcome else {
            panic!("a stalled WebSocket upgrade must be bounded");
        };
        let message = error.report().message;
        assert!(!message.contains("password"), "{message}");
        assert!(!message.contains("secret"), "{message}");
        assert!(!message.contains("fragment"), "{message}");
    }
}
