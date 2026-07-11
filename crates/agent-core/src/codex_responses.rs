//! Codex ChatGPT model provider over the OpenAI **Responses API**.
//!
//! Codex's ChatGPT-backed model is served by the Responses API
//! (`https://chatgpt.com/backend-api/codex/responses`), not `/chat/completions`.
//! This provider builds the Responses request shape, sets the Codex headers
//! (OAuth bearer + account id + beta/originator/session), and parses the SSE
//! stream into an assistant message with tool calls. The request/SSE contract
//! follows the upstream Codex CLI (mirrored by codex-openai-proxy and heddle).
//!
//! Credentials come from a [`CodexAuth`] source so this crate stays free of any
//! Fleety dependency — the OAuth implementation lives in `fleety-tools`.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::model::{
    Effort, Message, ModelCapabilities, ModelProvider, ModelResponse, Role, ToolCall, ToolSpec,
};
use crate::{CoreError, Result};

/// A bearer token plus the ChatGPT account id, for a Codex Responses call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexCreds {
    pub bearer: String,
    pub account_id: Option<String>,
}

/// Supplies Codex credentials per request (an OAuth implementation refreshes the
/// token as needed). Implemented in `fleety-tools`; kept here so the provider
/// carries no Fleety dependency.
#[async_trait::async_trait]
pub trait CodexAuth: Send + Sync {
    async fn credentials(&self) -> Result<CodexCreds>;
}

const DEFAULT_INSTRUCTIONS: &str = "You are a helpful assistant.";

/// Default originator sent to the Codex backend (overridable by the caller).
fn originator() -> String {
    std::env::var("FLEETY_CODEX_ORIGINATOR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "codex_cli_rs".to_string())
}

/// A provider that calls the Codex ChatGPT backend over the Responses API.
pub struct CodexResponses {
    base_url: String,
    model: String,
    auth: Arc<dyn CodexAuth>,
    client: reqwest::Client,
    caps: ModelCapabilities,
    effort: Option<Effort>,
}

impl CodexResponses {
    /// `base_url` is the Codex responses endpoint, e.g.
    /// `https://chatgpt.com/backend-api/codex/responses`.
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        auth: Arc<dyn CodexAuth>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            auth,
            client: reqwest::Client::new(),
            caps: ModelCapabilities::ALL,
            effort: None,
        }
    }

    pub fn with_capabilities(mut self, caps: ModelCapabilities) -> Self {
        self.caps = caps;
        self
    }

    /// Set the default reasoning effort (Responses `reasoning.effort`).
    pub fn with_effort(mut self, effort: Option<Effort>) -> Self {
        self.effort = effort;
        self
    }

    fn clone_with(&self, effort: Option<Effort>) -> CodexResponses {
        CodexResponses {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            auth: Arc::clone(&self.auth),
            client: self.client.clone(),
            caps: self.caps,
            effort,
        }
    }
}

/// Build the Responses API request body from the conversation and tools. Pure so
/// the mapping is unit-testable. System messages fold into `instructions`; other
/// messages and tool calls/results map to `input` items; tools map to function
/// tools.
pub fn build_request_body(
    model: &str,
    messages: &[Message],
    tools: &[ToolSpec],
    effort: Option<Effort>,
) -> Value {
    let mut instructions = String::new();
    let mut input: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {
                if let Some(c) = &m.content {
                    if !instructions.is_empty() {
                        instructions.push_str("\n\n");
                    }
                    instructions.push_str(c);
                }
            }
            Role::Tool => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": m.content.clone().unwrap_or_default(),
                }));
            }
            Role::User | Role::Assistant => {
                let role = if matches!(m.role, Role::User) {
                    "user"
                } else {
                    "assistant"
                };
                if let Some(c) = &m.content {
                    if !c.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": role,
                            "content": [{ "type": "input_text", "text": c }],
                        }));
                    }
                }
                for call in &m.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                        "call_id": call.id,
                    }));
                }
            }
        }
    }
    let tool_values: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
                "strict": false,
            })
        })
        .collect();

    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert(
        "instructions".into(),
        json!(if instructions.is_empty() {
            DEFAULT_INSTRUCTIONS.to_string()
        } else {
            instructions
        }),
    );
    body.insert("input".into(), json!(input));
    body.insert("tools".into(), json!(tool_values));
    body.insert("tool_choice".into(), json!("auto"));
    body.insert("parallel_tool_calls".into(), json!(false));
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    body.insert("include".into(), json!([]));
    if let Some(e) = effort {
        body.insert("reasoning".into(), json!({ "effort": e.as_str() }));
    }
    Value::Object(body)
}

/// Assemble the Responses SSE stream into an assistant message. Pure over the raw
/// body so it is unit-testable. Appends `response.output_text.delta` text (calling
/// `on_delta` for each), collects completed `function_call` items as tool calls,
/// and finishes on `response.completed` or `[DONE]`. Malformed lines are skipped.
pub fn assemble_responses_sse(
    body: &str,
    on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
) -> ModelResponse {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    for line in body.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    on_delta(delta);
                    text.push_str(delta);
                }
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item") {
                    if item.get("type").and_then(Value::as_str) == Some("function_call") {
                        if let Some(call) = function_call_from_item(item) {
                            tool_calls.push(call);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let content = if text.is_empty() { None } else { Some(text) };
    ModelResponse {
        message: Message {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            attachments: Vec::new(),
        },
    }
}

/// Parse a `function_call` output item into a `ToolCall` (arguments are a JSON
/// string; unparseable arguments are wrapped so nothing is lost).
fn function_call_from_item(item: &Value) -> Option<ToolCall> {
    let name = item.get("name").and_then(Value::as_str)?.to_string();
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| format!("call_{name}"));
    let arguments = match item.get("arguments").and_then(Value::as_str) {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({ "_unparsed": s })),
        None => json!({}),
    };
    Some(ToolCall {
        id,
        name,
        arguments,
    })
}

#[async_trait::async_trait]
impl ModelProvider for CodexResponses {
    async fn complete(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<ModelResponse> {
        let mut sink = |_: &str| {};
        self.complete_streaming(messages, tools, &mut sink).await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ModelResponse> {
        // Resolve credentials first (an OAuth source may refresh, and a logged-out
        // state surfaces as an actionable error before any request).
        let creds = self.auth.credentials().await?;
        let body = build_request_body(&self.model, messages, tools, self.effort);
        let session_id = uuid::Uuid::new_v4().to_string();

        let mut request = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", creds.bearer))
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", originator())
            .header("session_id", session_id)
            .header("Accept", "text/event-stream")
            .header(
                "User-Agent",
                "Mozilla/5.0 (compatible; Fleety/1.0; +https://github.com/) Codex",
            )
            .json(&body);
        if let Some(account) = &creds.account_id {
            request = request.header("chatgpt-account-id", account);
        }

        let response = request.send().await.map_err(|e| {
            CoreError::Provider(format!("request to {} failed: {e}", self.base_url))
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(CoreError::Provider(format!(
                "Codex responses endpoint returned HTTP {status}: {text}"
            )));
        }

        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut raw = String::new();
        while let Some(chunk) = stream.next().await {
            let bytes =
                chunk.map_err(|e| CoreError::Provider(format!("stream read failed: {e}")))?;
            raw.push_str(&String::from_utf8_lossy(&bytes));
        }
        Ok(assemble_responses_sse(&raw, on_delta))
    }

    fn capabilities(&self) -> ModelCapabilities {
        self.caps
    }

    fn with_effort(&self, effort: Option<Effort>) -> Option<Arc<dyn ModelProvider>> {
        Some(Arc::new(self.clone_with(effort)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct StubAuth {
        creds: std::result::Result<CodexCreds, String>,
    }

    #[async_trait::async_trait]
    impl CodexAuth for StubAuth {
        async fn credentials(&self) -> Result<CodexCreds> {
            self.creds.clone().map_err(CoreError::Provider)
        }
    }

    /// One-shot server: capture the request text, return an SSE body.
    fn serve_sse_once(body: String) -> (String, mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn complete_sets_codex_headers_and_returns_assembled_message() {
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n",
            "data: [DONE]\n",
        )
        .to_string();
        let (base, rx) = serve_sse_once(sse);
        let auth = Arc::new(StubAuth {
            creds: Ok(CodexCreds {
                bearer: "tok-123".into(),
                account_id: Some("acc-9".into()),
            }),
        });
        let provider = CodexResponses::new(base, "gpt-5-codex", auth);
        let resp = provider
            .complete(&[Message::user("hi")], &[])
            .await
            .expect("complete");
        assert_eq!(resp.message.content.as_deref(), Some("ok"));

        let req = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("request captured");
        assert!(req.starts_with("POST "));
        assert!(req.contains("authorization: Bearer tok-123"));
        assert!(req.contains("chatgpt-account-id: acc-9"));
        assert!(req.contains("openai-beta: responses=experimental"));
        assert!(req.contains("session_id:"));
        // Body is the Responses shape, not chat/completions.
        assert!(req.contains("\"input\""));
        assert!(req.contains("\"stream\":true"));
    }

    #[tokio::test]
    async fn complete_returns_actionable_error_when_logged_out_without_calling() {
        let auth = Arc::new(StubAuth {
            creds: Err("not signed in to ChatGPT; run `fleety auth login` to authenticate".into()),
        });
        // An unroutable base URL: if it tried to POST it would error differently.
        let provider = CodexResponses::new("http://127.0.0.1:1/responses", "m", auth);
        let err = provider
            .complete(&[Message::user("hi")], &[])
            .await
            .expect_err("logged out");
        assert!(err.report().message.contains("auth login"));
    }

    #[test]
    fn request_body_maps_system_messages_tools_and_history() {
        let messages = vec![
            Message::system("be terse"),
            Message::user("hi"),
            Message {
                role: Role::Assistant,
                content: Some("calling".into()),
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    arguments: json!({ "path": "a.txt" }),
                }],
                tool_call_id: None,
                attachments: Vec::new(),
            },
            Message::tool_result("c1", "file body"),
        ];
        let tools = vec![ToolSpec {
            name: "read_file".into(),
            description: "read a file".into(),
            parameters: json!({ "type": "object" }),
            risk: Default::default(),
        }];
        let body = build_request_body("gpt-5-codex", &messages, &tools, Some(Effort::Low));

        assert_eq!(body["model"], json!("gpt-5-codex"));
        assert_eq!(body["instructions"], json!("be terse"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["reasoning"]["effort"], json!("low"));
        // tools → function tool
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["name"], json!("read_file"));
        let input = body["input"].as_array().expect("input");
        // user message, assistant message, function_call, function_call_output
        assert_eq!(input[0]["type"], json!("message"));
        assert_eq!(input[0]["role"], json!("user"));
        assert_eq!(input[0]["content"][0]["text"], json!("hi"));
        let fc = input
            .iter()
            .find(|i| i["type"] == json!("function_call"))
            .expect("fc");
        assert_eq!(fc["name"], json!("read_file"));
        assert_eq!(fc["call_id"], json!("c1"));
        assert_eq!(fc["arguments"], json!(r#"{"path":"a.txt"}"#));
        let out = input
            .iter()
            .find(|i| i["type"] == json!("function_call_output"))
            .expect("out");
        assert_eq!(out["call_id"], json!("c1"));
        assert_eq!(out["output"], json!("file body"));
    }

    #[test]
    fn request_body_uses_default_instructions_when_no_system() {
        let body = build_request_body("m", &[Message::user("hi")], &[], None);
        assert_eq!(body["instructions"], json!(DEFAULT_INSTRUCTIONS));
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn sse_assembles_text_and_tool_call_and_skips_garbage() {
        let body = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n",
            "data: not-json\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"run\",\"call_id\":\"c9\",\"arguments\":\"{\\\"x\\\":1}\"}}\n",
            "data: {\"type\":\"response.completed\"}\n",
            "data: [DONE]\n",
        );
        let mut seen = String::new();
        let mut sink = |d: &str| seen.push_str(d);
        let resp = assemble_responses_sse(body, &mut sink);
        assert_eq!(resp.message.content.as_deref(), Some("Hello"));
        assert_eq!(seen, "Hello"); // deltas were emitted live
        assert_eq!(resp.message.tool_calls.len(), 1);
        assert_eq!(resp.message.tool_calls[0].name, "run");
        assert_eq!(resp.message.tool_calls[0].id, "c9");
        assert_eq!(resp.message.tool_calls[0].arguments, json!({ "x": 1 }));
    }
}
