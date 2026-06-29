//! `fleety acp` — make Fleety an Agent Client Protocol (ACP) agent.
//!
//! An ACP-capable editor (e.g. Zed) launches `fleety acp` as a subprocess and
//! speaks JSON-RPC 2.0 over stdio (LSP-style `Content-Length` framing). This
//! adapter bridges ACP to the existing fleety-server: it maps initialize /
//! session.new / session.load / session.prompt / session.cancel to the server's
//! conversation protocol, streams the server's assistant output back as
//! `session/update` notifications, and surfaces tool approvals as
//! `session/request_permission`. Only JSON-RPC goes to stdout; logs go to stderr.
//!
//! The framing + JSON-RPC types and the ACP↔server mappings are pure and
//! unit-tested; a live editor session is verified manually.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

// ---- JSON-RPC 2.0 framing (LSP-style Content-Length) ----

/// Write one framed JSON-RPC message.
pub fn write_frame<W: Write>(w: &mut W, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Read one framed JSON-RPC message; `None` on EOF or an unparsable frame.
/// The runtime loop uses the async variant; this sync one backs the framing
/// tests and is available for non-async callers.
#[allow(dead_code)]
pub fn read_frame<R: BufRead>(r: &mut R) -> std::io::Result<Option<Value>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break; // end of headers
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    let mut buf = vec![0u8; len];
    std::io::Read::read_exact(r, &mut buf)?;
    Ok(serde_json::from_slice(&buf).ok())
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

/// `session/request_permission` params from a server approval request. Used when
/// approval streaming is wired (a follow-up); the mapping is defined + tested now.
#[allow(dead_code)]
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
        "initialize" => vec![response_ok(reply_id(), initialize_result())],
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
) -> std::io::Result<Option<Value>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = rest.trim().parse::<usize>().ok();
        }
    }
    let Some(len) = content_length else {
        return Ok(None);
    };
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf).ok())
}

/// Run the ACP agent over stdio, bridging to the fleety-server. Only JSON-RPC is
/// written to stdout; logs go to stderr (configured by the caller).
pub async fn run(agent_url: String) -> agent_core::Result<()> {
    let bridge = WsBridge::new(agent_url);
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    loop {
        match read_frame_async(&mut reader).await {
            Ok(Some(msg)) => {
                let frames = handle_message(&msg, &bridge).await;
                let mut stdout = std::io::stdout();
                for f in frames {
                    if write_frame(&mut stdout, &f).is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(None) => return Ok(()), // EOF: editor closed
            Err(e) => {
                tracing::warn!(%e, "acp: stdin read error; exiting");
                return Ok(());
            }
        }
    }
}

/// Real bridge: each prompt opens a short-lived WebSocket to the server, sends
/// the user message rooted at the session's cwd, and collects the assistant
/// reply. Stateless per prompt (the server persists the conversation by id).
struct WsBridge {
    agent_url: String,
    cwds: tokio::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
}

impl WsBridge {
    fn new(agent_url: String) -> Self {
        Self {
            agent_url,
            cwds: tokio::sync::Mutex::new(std::collections::HashMap::new()),
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
        let hello = serde_json::to_string(&fleety_protocol::ClientMsg::Hello {
            device_id: crate::device_id(),
            protocol: fleety_protocol::PROTOCOL_VERSION,
            token: std::env::var("FLEETY_TOKEN").ok().filter(|s| !s.is_empty()),
            pairing_code: None,
            local_tools_json: None,
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
        // The wire form has a Content-Length header.
        let text = String::from_utf8_lossy(&buf);
        assert!(text.starts_with("Content-Length: "));
        // ...and reads back to the same value.
        let mut cur = Cursor::new(buf);
        let got = read_frame(&mut cur).unwrap().unwrap();
        assert_eq!(got, msg);
        // A second read at EOF yields None.
        assert!(read_frame(&mut cur).unwrap().is_none());
    }

    #[test]
    fn malformed_frame_is_none_not_panic() {
        // Headers but a body that isn't JSON → None (caller emits a JSON-RPC error).
        let mut cur = Cursor::new(b"Content-Length: 3\r\n\r\nxxx".to_vec());
        assert!(read_frame(&mut cur).unwrap().is_none());
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
        assert_eq!(full, EditorCapabilities { read: true, write: true, terminal: true });
        assert_eq!(
            editor_tool_names(&full),
            vec!["editor_read_file", "editor_write_file", "editor_edit", "editor_run"]
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
    fn editor_request_maps_to_acp_methods() {
        let (m, p) = editor_request("s1", "editor_read_file", &json!({ "path": "a.rs" })).unwrap();
        assert_eq!(m, "fs/read_text_file");
        assert_eq!(p["sessionId"], "s1");
        assert_eq!(p["path"], "a.rs");
        let (m, p) =
            editor_request("s1", "editor_write_file", &json!({ "path": "a.rs", "content": "x" }))
                .unwrap();
        assert_eq!(m, "fs/write_text_file");
        assert_eq!(p["content"], "x");
        let (m, p) = editor_request("s1", "editor_run", &json!({ "command": "git status" })).unwrap();
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
