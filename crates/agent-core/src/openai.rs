//! OpenAI-compatible model provider.
//!
//! Speaks the `/chat/completions` API (OpenAI, OpenRouter, LM Studio, Ollama,
//! vLLM, …): non-streaming request/response with tool-calling, plus SSE
//! streaming (`with_streaming` / `complete_streaming`) for token-by-token output.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::model::{Message, ModelProvider, ModelResponse, Role, ToolCall, ToolSpec};
use crate::{CoreError, Result};

/// A provider backed by any OpenAI-compatible `/chat/completions` endpoint.
pub struct OpenAiCompat {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
    stream: bool,
}

impl OpenAiCompat {
    /// `base_url` is the API root, e.g. `http://localhost:1234/v1`.
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key,
            client: reqwest::Client::new(),
            stream: false,
        }
    }

    /// Request the streaming (`stream: true`) chat-completions API and assemble
    /// the SSE chunks into a full response. The result is identical to the
    /// non-streaming path; useful for endpoints that prefer/require streaming.
    pub fn with_streaming(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    /// Discover available model ids via `GET {base_url}/models`.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let url = format!("{}/models", self.base_url);
        let mut request = self.client.get(&url);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| CoreError::Provider(format!("request to {url} failed: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CoreError::Provider(format!("reading /models body failed: {e}")))?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!(
                "/models returned HTTP {status}: {text}"
            )));
        }
        parse_models(&text)
    }
}

fn parse_models(body: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
    }
    let parsed: ModelsResponse = serde_json::from_str(body).map_err(|e| {
        CoreError::Provider(format!("unexpected /models response: {e}; body: {body}"))
    })?;
    Ok(parsed.data.into_iter().map(|m| m.id).collect())
}

#[async_trait::async_trait]
impl ModelProvider for OpenAiCompat {
    async fn complete(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<ModelResponse> {
        let mut request = self
            .client
            .post(self.endpoint())
            .json(&self.request_body(messages, tools));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request.send().await.map_err(|e| {
            CoreError::Provider(format!("request to {} failed: {e}", self.endpoint()))
        })?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CoreError::Provider(format!("reading response body failed: {e}")))?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!(
                "endpoint returned HTTP {status}: {text}"
            )));
        }

        if self.stream {
            return assemble_sse(&text);
        }
        let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
            CoreError::Provider(format!("unexpected response shape: {e}; body: {text}"))
        })?;
        parse_response(parsed)
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ModelResponse> {
        if !self.stream {
            return self.complete(messages, tools).await;
        }
        let mut request = self
            .client
            .post(self.endpoint())
            .json(&self.request_body(messages, tools));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(|e| {
            CoreError::Provider(format!("request to {} failed: {e}", self.endpoint()))
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(CoreError::Provider(format!(
                "endpoint returned HTTP {status}: {text}"
            )));
        }

        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut acc = SseAccumulator::default();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let bytes =
                chunk.map_err(|e| CoreError::Provider(format!("stream read failed: {e}")))?;
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                if let Some(data) = line.trim().strip_prefix("data:") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return Ok(acc.finish());
                    }
                    if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                        acc.push(&chunk_json, on_delta);
                    }
                }
            }
        }
        Ok(acc.finish())
    }
}

impl OpenAiCompat {
    fn request_body(&self, messages: &[Message], tools: &[ToolSpec]) -> Value {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(self.model));
        body.insert(
            "messages".to_string(),
            json!(messages.iter().map(wire_message).collect::<Vec<_>>()),
        );
        if !tools.is_empty() {
            body.insert(
                "tools".to_string(),
                json!(tools.iter().map(wire_tool).collect::<Vec<_>>()),
            );
        }
        if self.stream {
            body.insert("stream".to_string(), json!(true));
        }
        Value::Object(body)
    }
}

/// Accumulates OpenAI SSE delta chunks (content + tool-call deltas merged by
/// index) into a final [`ModelResponse`]; `push` reports content slices to
/// `on_delta` for live display.
#[derive(Default)]
struct SseAccumulator {
    content: String,
    calls: Vec<(String, String, String)>,
}

impl SseAccumulator {
    fn push<F: FnMut(&str) + ?Sized>(&mut self, chunk: &Value, on_delta: &mut F) {
        let delta = &chunk["choices"][0]["delta"];
        if let Some(c) = delta.get("content").and_then(Value::as_str) {
            self.content.push_str(c);
            on_delta(c);
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tool_calls {
                let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                while self.calls.len() <= idx {
                    self.calls
                        .push((String::new(), String::new(), String::new()));
                }
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        self.calls[idx].0 = id.to_string();
                    }
                }
                if let Some(func) = tc.get("function") {
                    if let Some(name) = func.get("name").and_then(Value::as_str) {
                        self.calls[idx].1.push_str(name);
                    }
                    if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                        self.calls[idx].2.push_str(args);
                    }
                }
            }
        }
    }

    fn finish(self) -> ModelResponse {
        let tool_calls: Vec<ToolCall> = self
            .calls
            .into_iter()
            .filter(|(_, name, _)| !name.is_empty())
            .map(|(id, name, args)| ToolCall {
                id: if id.is_empty() {
                    format!("call_{name}")
                } else {
                    id
                },
                name,
                arguments: parse_args(&args),
            })
            .collect();
        let content = if self.content.is_empty() && !tool_calls.is_empty() {
            None
        } else {
            Some(self.content)
        };
        ModelResponse {
            message: Message {
                role: Role::Assistant,
                content,
                tool_calls,
                tool_call_id: None,
            },
        }
    }
}

/// Assemble a full SSE body into a [`ModelResponse`] (used when `complete`
/// collects the whole stream at once).
fn assemble_sse(body: &str) -> Result<ModelResponse> {
    let mut acc = SseAccumulator::default();
    let mut noop = |_: &str| {};
    for line in body.lines() {
        let data = match line.trim().strip_prefix("data:") {
            Some(d) => d.trim(),
            None => continue,
        };
        if data == "[DONE]" {
            break;
        }
        if let Ok(chunk) = serde_json::from_str::<Value>(data) {
            acc.push(&chunk, &mut noop);
        }
    }
    Ok(acc.finish())
}

// --- wire mapping (pure, unit-tested without network) ---

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn wire_message(message: &Message) -> Value {
    let mut object = Map::new();
    object.insert("role".to_string(), json!(role_str(message.role)));
    object.insert(
        "content".to_string(),
        message
            .content
            .as_ref()
            .map(|c| json!(c))
            .unwrap_or(Value::Null),
    );
    if !message.tool_calls.is_empty() {
        let calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(|tc| {
                json!({
                    "id": tc.id,
                    "type": "function",
                    "function": { "name": tc.name, "arguments": tc.arguments.to_string() }
                })
            })
            .collect();
        object.insert("tool_calls".to_string(), json!(calls));
    }
    if let Some(id) = &message.tool_call_id {
        object.insert("tool_call_id".to_string(), json!(id));
    }
    Value::Object(object)
}

fn wire_tool(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": { "name": spec.name, "description": spec.description, "parameters": spec.parameters }
    })
}

fn parse_args(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| json!({ "_unparsed": raw }))
}

fn parse_response(parsed: ChatResponse) -> Result<ModelResponse> {
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Provider("response contained no choices".to_string()))?;
    let tool_calls = choice
        .message
        .tool_calls
        .into_iter()
        .map(|tc| ToolCall {
            id: tc.id,
            name: tc.function.name,
            arguments: parse_args(&tc.function.arguments),
        })
        .collect();
    Ok(ModelResponse {
        message: Message {
            role: Role::Assistant,
            content: choice.message.content,
            tool_calls,
            tool_call_id: None,
        },
    })
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Deserialize)]
struct ResponseToolCall {
    id: String,
    function: ResponseFunction,
}

#[derive(Debug, Deserialize)]
struct ResponseFunction {
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn serve_once(
        status: &'static str,
        content_type: &'static str,
        body: String,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buf = vec![0_u8; 8192];
            let n = stream.read(&mut buf).expect("read request");
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(header.as_bytes())
                .and_then(|_| stream.write_all(body.as_bytes()))
                .expect("write response");
            let _ = tx.send(request);
        });
        (format!("http://{addr}/v1"), rx)
    }

    #[test]
    fn assemble_sse_streams_content_and_tool_calls() {
        let mut body = String::new();
        body.push_str(&format!(
            "data: {}\n",
            json!({"choices":[{"delta":{"content":"Hel"}}]})
        ));
        body.push_str(&format!(
            "data: {}\n",
            json!({"choices":[{"delta":{"content":"lo"}}]})
        ));
        body.push_str("data: [DONE]\n");
        let r = assemble_sse(&body).expect("assemble");
        assert_eq!(r.message.content.as_deref(), Some("Hello"));
        assert!(r.message.tool_calls.is_empty());

        let mut tb = String::new();
        tb.push_str(&format!(
            "data: {}\n",
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"echo","arguments":"{\"x\":"}}]}}]})
        ));
        tb.push_str(&format!(
            "data: {}\n",
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]})
        ));
        tb.push_str("data: [DONE]\n");
        let r2 = assemble_sse(&tb).expect("assemble2");
        assert_eq!(r2.message.tool_calls.len(), 1);
        assert_eq!(r2.message.tool_calls[0].name, "echo");
        assert_eq!(r2.message.tool_calls[0].arguments, json!({ "x": 1 }));
    }

    #[test]
    fn endpoint_trims_trailing_slash() {
        let provider = OpenAiCompat::new("http://localhost:1234/v1/", "m", None);
        assert_eq!(
            provider.endpoint(),
            "http://localhost:1234/v1/chat/completions"
        );
    }

    #[test]
    fn wire_message_maps_tool_calls() {
        let msg = Message {
            role: Role::Assistant,
            content: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: json!({ "text": "hi" }),
            }],
            tool_call_id: None,
        };
        let wire = wire_message(&msg);
        assert_eq!(wire["role"], json!("assistant"));
        assert_eq!(wire["content"], Value::Null);
        assert_eq!(wire["tool_calls"][0]["function"]["name"], json!("echo"));
        // arguments is serialized as a JSON string, per the OpenAI format
        assert_eq!(
            wire["tool_calls"][0]["function"]["arguments"],
            json!("{\"text\":\"hi\"}")
        );
    }

    #[test]
    fn parse_args_handles_empty_and_json() {
        assert_eq!(parse_args(""), json!({}));
        assert_eq!(parse_args("{\"a\":1}"), json!({ "a": 1 }));
        assert_eq!(parse_args("not json"), json!({ "_unparsed": "not json" }));
    }

    #[test]
    fn parse_models_extracts_ids() {
        let body = r#"{"object":"list","data":[{"id":"gpt-5.5","object":"model"},{"id":"qwen3"}]}"#;
        let ids = parse_models(body).expect("parse");
        assert_eq!(ids, vec!["gpt-5.5".to_string(), "qwen3".to_string()]);
    }

    #[test]
    fn parse_response_extracts_message() {
        let raw = r#"{"choices":[{"message":{"content":"hello","tool_calls":[]}}]}"#;
        let parsed: ChatResponse = serde_json::from_str(raw).expect("parse");
        let response = parse_response(parsed).expect("map");
        assert_eq!(response.message.content.as_deref(), Some("hello"));
        assert!(response.message.tool_calls.is_empty());
    }

    #[test]
    fn request_body_includes_tools_and_stream_flag_when_enabled() {
        let provider = OpenAiCompat::new("http://localhost:1234/v1", "model-a", Some("k".into()))
            .with_streaming(true);
        let body = provider.request_body(
            &[Message::user("hi")],
            &[ToolSpec {
                name: "echo".into(),
                description: "Echo input".into(),
                parameters: json!({"type":"object"}),
                risk: crate::model::RiskLevel::Mutate,
            }],
        );

        assert_eq!(body["model"], json!("model-a"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("echo"));
    }

    #[test]
    fn wire_message_maps_tool_result_id() {
        let wire = wire_message(&Message::tool_result("call-1", "ok"));

        assert_eq!(wire["role"], json!("tool"));
        assert_eq!(wire["content"], json!("ok"));
        assert_eq!(wire["tool_call_id"], json!("call-1"));
    }

    #[test]
    fn assemble_sse_ignores_noise_and_synthesizes_missing_tool_id() {
        let mut body = String::from("event: ping\n\n");
        body.push_str(&format!(
            "data: {}\n",
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"name":"run","arguments":"not-json"}}]}}]})
        ));
        body.push_str("data: [DONE]\n");

        let response = assemble_sse(&body).expect("sse");
        assert_eq!(response.message.content, None);
        assert_eq!(response.message.tool_calls.len(), 1);
        assert_eq!(response.message.tool_calls[0].id, "call_run");
        assert_eq!(
            response.message.tool_calls[0].arguments,
            json!({"_unparsed":"not-json"})
        );
    }

    #[test]
    fn parse_models_and_response_reject_bad_shapes() {
        assert!(parse_models(r#"{"data":[{"name":"missing-id"}]}"#).is_err());

        let parsed: ChatResponse = serde_json::from_str(r#"{"choices":[]}"#).expect("shape");
        let err = parse_response(parsed).expect_err("empty choices should error");
        assert!(err.report().message.contains("no choices"));
    }

    #[tokio::test]
    async fn list_models_uses_bearer_auth_and_reports_http_errors() {
        let (base, rx) = serve_once(
            "200 OK",
            "application/json",
            r#"{"data":[{"id":"model-a"}]}"#.to_string(),
        );
        let provider = OpenAiCompat::new(base, "unused", Some("key-1".into()));
        assert_eq!(
            provider.list_models().await.expect("models"),
            vec!["model-a"]
        );
        let request = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("request captured");
        assert!(request.starts_with("GET /v1/models "));
        assert!(request.contains("Bearer key-1"));

        let (base, _) = serve_once(
            "500 Internal Server Error",
            "text/plain",
            "nope".to_string(),
        );
        let provider = OpenAiCompat::new(base, "unused", None);
        let err = provider.list_models().await.expect_err("http error");
        assert!(err.report().message.contains("HTTP"));
    }

    #[tokio::test]
    async fn complete_parses_non_streaming_response_and_errors() {
        let body = r#"{"choices":[{"message":{"content":"hello","tool_calls":[]}}]}"#.to_string();
        let (base, rx) = serve_once("200 OK", "application/json", body);
        let provider = OpenAiCompat::new(base, "model-a", Some("key-2".into()));
        let response = provider
            .complete(&[Message::user("hi")], &[])
            .await
            .expect("complete");
        assert_eq!(response.message.content.as_deref(), Some("hello"));
        let request = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("request captured");
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.contains("Bearer key-2"));
        assert!(request.contains("\"model\":\"model-a\""));

        let (base, _) = serve_once(
            "429 Too Many Requests",
            "text/plain",
            "slow down".to_string(),
        );
        let provider = OpenAiCompat::new(base, "model-a", None);
        let err = provider
            .complete(&[Message::user("hi")], &[])
            .await
            .expect_err("status error");
        assert!(err.report().message.contains("429"));
    }

    #[tokio::test]
    async fn complete_streaming_streams_deltas_and_non_streaming_delegates() {
        let mut stream_body = String::new();
        stream_body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"content":"Hel"}}]})
        ));
        stream_body.push_str(&format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"content":"lo"}}]})
        ));
        stream_body.push_str("data: [DONE]\n\n");
        let (base, _) = serve_once("200 OK", "text/event-stream", stream_body);
        let provider = OpenAiCompat::new(base, "model-a", None).with_streaming(true);
        let mut deltas = String::new();
        let response = provider
            .complete_streaming(&[Message::user("hi")], &[], &mut |chunk| {
                deltas.push_str(chunk)
            })
            .await
            .expect("streaming");
        assert_eq!(deltas, "Hello");
        assert_eq!(response.message.content.as_deref(), Some("Hello"));

        let body =
            r#"{"choices":[{"message":{"content":"fallback","tool_calls":[]}}]}"#.to_string();
        let (base, _) = serve_once("200 OK", "application/json", body);
        let provider = OpenAiCompat::new(base, "model-a", None);
        let mut ignored = String::new();
        let response = provider
            .complete_streaming(&[Message::user("hi")], &[], &mut |chunk| {
                ignored.push_str(chunk)
            })
            .await
            .expect("delegated complete");
        assert_eq!(ignored, "");
        assert_eq!(response.message.content.as_deref(), Some("fallback"));
    }
}
