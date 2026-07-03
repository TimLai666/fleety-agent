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
//! unit-tested. NOTE: the ACP *message shapes* (e.g. the `session/update` body)
//! still need verification against a live editor — the framing is fixed, the
//! shapes are the next thing to confirm end-to-end.

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

// ---- ACP <-> fleety-server mappings (pure) ----

/// `session/update` for streamed assistant text.
pub fn assistant_update(session_id: &str, text: &str) -> Value {
    notification(
        "session/update",
        json!({
            "sessionId": session_id,
            "update": { "kind": "agent_message_chunk", "text": text }
        }),
    )
}

/// `session/request_permission` params from a server approval request — emitted
/// to the editor when the server asks for tool approval (require-approval policy).
pub fn permission_request(session_id: &str, tool: &str, summary: &str) -> Value {
    json!({
        "sessionId": session_id,
        "toolCall": { "title": tool, "summary": summary },
        "options": [
            { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
            { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
        ]
    })
}

/// The stop reason for a completed prompt turn.
pub fn stop_reason() -> &'static str {
    "end_turn"
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
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
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
                Err(e) => vec![response_err(reply_id(), -32000, &e.report().message)],
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
                    out.push(response_ok(
                        reply_id(),
                        json!({ "stopReason": stop_reason() }),
                    ));
                    out
                }
                Err(e) => vec![response_err(reply_id(), -32000, &e.report().message)],
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
                    out.push(response_ok(reply_id(), json!({})));
                    out
                }
                Err(e) => vec![response_err(reply_id(), -32000, &e.report().message)],
            }
        }
        // Cancel is a notification (no id); the in-flight turn is best-effort.
        "session/cancel" => vec![],
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

/// Run the ACP agent over stdio, bridging to the fleety-server. Only JSON-RPC is
/// written to stdout; logs go to stderr (configured by the caller).
pub async fn run(agent_url: String) -> agent_core::Result<()> {
    // The stdin reader is shared: the main loop reads requests from it, and during
    // a prompt the bridge borrows it to read the editor's responses to the agent's
    // fs/terminal calls (the editor is awaiting our prompt reply then, so stdin
    // carries only those responses — no contention).
    let reader = std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::BufReader::new(
        tokio::io::stdin(),
    )));
    let bridge = WsBridge::new(agent_url, std::sync::Arc::clone(&reader));
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

/// Real bridge: each prompt opens a short-lived WebSocket to the server, sends
/// the user message rooted at the session's cwd, and collects the assistant
/// reply. Stateless per prompt (the server persists the conversation by id).
struct WsBridge {
    agent_url: String,
    cwds: tokio::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
    /// The editor's advertised capabilities (from `initialize`), gating which
    /// `editor_*` tools we offer the server.
    caps: std::sync::Mutex<EditorCapabilities>,
    /// Shared stdin reader, for reading the editor's fs/terminal responses.
    reader: SharedReader,
    /// JSON-RPC request id counter for our calls to the editor.
    next_req: std::sync::atomic::AtomicI64,
}

impl WsBridge {
    fn new(agent_url: String, reader: SharedReader) -> Self {
        Self {
            agent_url,
            cwds: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            caps: std::sync::Mutex::new(EditorCapabilities::default()),
            reader,
            next_req: std::sync::atomic::AtomicI64::new(1),
        }
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
                    // Not our response — ignore (e.g. a mid-prompt session/cancel).
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

        let (ws, _) = tokio_tungstenite::connect_async(&self.agent_url)
            .await
            .map_err(|e| {
                CoreError::Provider(format!("cannot connect to {}: {e}", self.agent_url))
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
        let hello = serde_json::to_string(&fleety_protocol::ClientMsg::Hello {
            device_id: crate::device_id(),
            protocol: fleety_protocol::PROTOCOL_VERSION,
            token: std::env::var("FLEETY_TOKEN").ok().filter(|s| !s.is_empty()),
            pairing_code: None,
            local_tools_json,
            hostname: fleety_tools::device::hostname(),
        })
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
        while let Some(frame) = rx.next().await {
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
                    let params = permission_request(conversation, &tool, &summary);
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
impl AcpBridge for WsBridge {
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
        assert_eq!(r[0]["params"]["update"]["text"], "hello");
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
    fn mappings_are_well_formed() {
        let u = assistant_update("s1", "hello");
        assert_eq!(u["method"], "session/update");
        assert_eq!(u["params"]["sessionId"], "s1");
        assert_eq!(u["params"]["update"]["text"], "hello");
        let p = permission_request("s1", "write_file", "edit foo");
        assert_eq!(p["sessionId"], "s1");
        assert_eq!(p["toolCall"]["title"], "write_file");
        assert!(p["options"].as_array().is_some());
        assert_eq!(stop_reason(), "end_turn");
        let origin = cwd_to_origin(Some("/home/alice/proj"));
        assert_eq!(origin.cwd.as_deref(), Some("/home/alice/proj"));
        let origin_none = cwd_to_origin(None);
        assert_eq!(origin_none.cwd, None);
        assert!(initialize_result()["agentCapabilities"]["loadSession"]
            .as_bool()
            .unwrap());
    }
}
