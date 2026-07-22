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
        home: std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok()),
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
        std::env::var("APPDATA").ok().map(|p| {
            std::path::PathBuf::from(p)
                .join("Zed")
                .join("settings.json")
        })
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(|p| {
            std::path::PathBuf::from(p)
                .join(".config")
                .join("zed")
                .join("settings.json")
        })
    }
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
    servers.insert("Fleety".to_string(), fleety_agent_entry(command, server));
    let pretty = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    Ok((pretty, updated))
}

/// Re-point any *already-installed* ACP agent configs at the current binary, for
/// `fleety update` to call. This self-heals a changed binary path or an evolved
/// `acp` invocation. It NEVER newly installs — only editors already set up for
/// Fleety are touched; missing or unparseable (JSONC) configs are left alone.
pub fn refresh_installed(server: Option<&str>) {
    let Some(path) = zed_settings_path() else {
        return;
    };
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return; // not configured here
    };
    if let Some(merged) = refresh_zed_settings(&existing, &current_exe_str(), server) {
        match atomic_replace(&path, merged.as_bytes()) {
            Ok(()) => println!(
                "Refreshed the Fleety ACP agent in Zed ({}).",
                path.display()
            ),
            Err(e) => eprintln!(
                "Could not refresh the Fleety ACP agent in Zed ({}): {e}",
                path.display()
            ),
        }
    }
}

/// Pure refresh decision for Zed: return the updated JSON only when a Fleety entry
/// is already present AND re-pointing it at `command` changes something. Returns
/// `None` when Fleety isn't installed, the file is unparseable (JSONC), or nothing
/// would change — so a refresh never newly-installs or rewrites needlessly.
pub fn refresh_zed_settings(existing: &str, command: &str, server: Option<&str>) -> Option<String> {
    let mut root = serde_json::from_str::<Value>(existing).ok()?;
    let entry = root
        .get_mut("agent_servers")?
        .get_mut("Fleety")?
        .as_object_mut()?;
    let mut changed = entry.get("command").and_then(Value::as_str) != Some(command);
    if changed {
        entry.insert("command".to_string(), json!(command));
    }
    if let Some(server) = server {
        let env = entry
            .entry("env")
            .or_insert_with(|| json!({}))
            .as_object_mut()?;
        if env.get("FLEETY_AGENT_URL").and_then(Value::as_str) != Some(server) {
            env.insert("FLEETY_AGENT_URL".to_string(), json!(server));
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    serde_json::to_string_pretty(&root).ok()
}

/// Replace a settings file through a uniquely named temporary file in the same
/// directory. Keeping the temporary file beside the destination makes the
/// rename a single-filesystem operation; failures before that point leave the
/// existing settings untouched.
fn atomic_replace(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    atomic_replace_with(path, contents, |_| Ok(()))
}

fn atomic_replace_with(
    path: &std::path::Path,
    contents: &[u8],
    before_replace: impl FnOnce(&std::path::Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".fleety-zed-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        before_replace(&tmp)?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
    match target.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("zed") => install_zed(server),
        Some(other) => {
            println!(
                "No built-in auto-config for editor '{}' (supported: zed).\n",
                crate::terminal_safe_text(other)
            );
            print_generic(server.as_deref());
            Ok(())
        }
        None => {
            print_generic(server.as_deref());
            Ok(())
        }
    }
}

/// Print the editor-agnostic ACP setup: the command any ACP client launches.
fn print_generic(server: Option<&str>) {
    let cmd = current_exe_str();
    println!("Fleety is an ACP agent — point any ACP-capable editor at this command:\n");
    println!("    command: {}", crate::terminal_safe_text(&cmd));
    println!("    args:    [\"acp\"]");
    match server {
        Some(s) => println!(
            "    env:     FLEETY_AGENT_URL={}\n",
            crate::terminal_safe_endpoint(s)
        ),
        None => println!("    env:     FLEETY_AGENT_URL=ws://127.0.0.1:8787   (or your server)\n"),
    }
    println!("Auto-configure a supported editor:");
    println!("    fleety acp install zed [--server ws://host:8787]\n");
    println!("For other editors (JetBrains, neovim, Emacs, …), set their custom-ACP-agent");
    println!("command to the above — ACP is a shared protocol, the same agent works for all.");
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
        return Ok(());
    };
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    match merge_zed_settings(&existing, &command, server.as_deref()) {
        Ok((merged, updated)) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !existing.trim().is_empty() {
                let _ = std::fs::write(path.with_extension("json.bak"), &existing);
            }
            atomic_replace(&path, merged.as_bytes())
                .map_err(|e| CoreError::Message(format!("cannot write {}: {e}", path.display())))?;
            let verb = if updated { "Updated" } else { "Configured" };
            println!(
                "{verb} the Fleety agent in Zed at {}.\nRestart Zed, then pick \"Fleety\" in the agent panel.\n\
                 (Agent binary: {command})",
                path.display()
            );
        }
        Err(e) => {
            println!(
                "Couldn't safely edit {} ({e}).\nAdd this to your Zed settings.json manually:\n\n{snippet}",
                path.display()
            );
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
                Err(e) => vec![response_err(
                    reply_id(),
                    INTERNAL_ERROR,
                    &e.report().message,
                )],
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
                Err(e) => vec![response_err(
                    reply_id(),
                    INTERNAL_ERROR,
                    &e.report().message,
                )],
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
                Err(e) => vec![response_err(
                    reply_id(),
                    INTERNAL_ERROR,
                    &e.report().message,
                )],
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
struct WsBridge<R> {
    target: fleety_tools::connection::Resolved,
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
            target,
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
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        // A stale cancel (e.g. one that arrived while idle) must not poison
        // this turn: the flag only reflects cancels seen while it runs.
        if let Ok(mut c) = self.cancelled.lock() {
            c.remove(conversation);
        }

        let (ws, _) = tokio_tungstenite::connect_async(&self.target.url)
            .await
            .map_err(|e| {
                CoreError::Provider(format!("cannot connect to {}: {e}", self.target.url))
            })?;
        let (mut tx, mut rx) = ws.split();
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
        let hello = hello_json(self.target.token.as_deref(), local_tools_json)
            .map_err(|e| CoreError::Message(format!("serialize hello: {e}")))?;
        tx.send(WsMessage::Text(hello))
            .await
            .map_err(|e| CoreError::Provider(format!("send hello: {e}")))?;

        let outbound = if resume {
            serde_json::to_string(&fleety_protocol::ClientMsg::Resume {
                conversation_id: conversation.to_string(),
                after_seq: 0,
            })
        } else {
            serde_json::to_string(&fleety_protocol::ClientMsg::UserMessage {
                conversation_id: Some(conversation.to_string()),
                text: text.to_string(),
                origin: cwd_to_origin(cwd.as_deref()),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            })
        }
        .map_err(|e| CoreError::Message(format!("serialize message: {e}")))?;
        tx.send(WsMessage::Text(outbound))
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
            Server(Option<Result<WsMessage, tokio_tungstenite::tungstenite::Error>>),
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
                    let _ = tx.send(WsMessage::Text(t)).await;
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
                        f = rx.next() => Race::Server(f),
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
                Some(rx.next().await)
            };
            let Some(next) = server_frame else { continue };
            let Some(frame) = next else { break }; // server closed the socket
            let Ok(frame) = frame else { break };
            if !frame.is_text() {
                continue;
            }
            let Ok(text) = frame.to_text() else { continue };
            let Ok(msg) = serde_json::from_str::<fleety_protocol::ServerMsg>(text) else {
                continue;
            };
            match msg {
                fleety_protocol::ServerMsg::Assistant { text, .. } => chunks.push(text),
                fleety_protocol::ServerMsg::Replay { content, .. } => chunks.push(content),
                fleety_protocol::ServerMsg::Done { .. } => break,
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
                    if let Ok(t) = serde_json::to_string(&reply) {
                        let _ = tx.send(WsMessage::Text(t)).await;
                    }
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
                    if let Ok(t) = serde_json::to_string(&reply) {
                        let _ = tx.send(WsMessage::Text(t)).await;
                    }
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
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message as WsMessage;
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
        let Ok((ws, _)) = tokio_tungstenite::connect_async(&self.target.url).await else {
            tracing::warn!(url = %self.target.url, "acp: cancel: server unreachable; nothing to cancel");
            return;
        };
        let (mut tx, _rx) = ws.split();
        let conversation_id = (!session_id.is_empty()).then(|| session_id.to_string());
        if let (Ok(hello), Ok(cancel)) = (
            hello_json(self.target.token.as_deref(), None),
            serde_json::to_string(&fleety_protocol::ClientMsg::CancelTurn { conversation_id }),
        ) {
            let _ = tx.send(WsMessage::Text(hello)).await;
            let _ = tx.send(WsMessage::Text(cancel)).await;
        }
        let _ = tx.close().await;
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
            refresh_zed_settings(r#"{"theme":"dark"}"#, "/bin/fleety", None),
            None
        );
        assert_eq!(refresh_zed_settings("", "/bin/fleety", None), None);
        // Unparseable (JSONC) → no refresh (never clobbers).
        assert_eq!(refresh_zed_settings("// c\n{}", "/bin/fleety", None), None);

        // Installed at an old path → refresh re-points it at the current binary.
        let old = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp"],"env":{}}}}"#;
        let refreshed = refresh_zed_settings(old, "/new/fleety", None).expect("should refresh");
        let v: Value = serde_json::from_str(&refreshed).unwrap();
        assert_eq!(v["agent_servers"]["Fleety"]["command"], "/new/fleety");

        // Already pointing at the current binary → nothing changes → no rewrite.
        assert_eq!(refresh_zed_settings(&refreshed, "/new/fleety", None), None);

        // A path-only refresh preserves the complete installed entry except
        // for `command`, including its server binding and editor-specific env.
        let bound = r#"{"agent_servers":{"Fleety":{"type":"custom","command":"/old/fleety","args":["acp","--editor-mode"],"env":{"FLEETY_AGENT_URL":"wss://paired.example/ws","EDITOR_FLAG":"keep"},"extra":"keep"}}}"#;
        let refreshed = refresh_zed_settings(bound, "/new/fleety", None).expect("refresh");
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

    #[tokio::test]
    async fn bridge_hello_uses_the_resolved_profile_token() {
        let (url, server_rx) = scripted_server(vec![vec![
            vec![],
            vec![ServerMsg::Done {
                conversation_id: "c-token".to_string(),
            }],
        ]]);
        let (editor_in, _editor_out) = tokio::io::duplex(1024);
        let bridge = WsBridge::new(
            fleety_tools::connection::Resolved {
                url,
                token: Some("paired-profile-token".to_string()),
                source: fleety_tools::connection::Source::Profile("paired".to_string()),
            },
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
            fleety_tools::connection::Resolved {
                url: url.to_string(),
                token: None,
                source: fleety_tools::connection::Source::OverrideUrl,
            },
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
            vec![], // Hello
            vec![], // UserMessage — the server now waits for the CancelTurn
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
            vec![vec![], vec![]],
            // Connection 2 — the next prompt: Hello, UserMessage → reply+Done.
            vec![
                vec![],
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
}
