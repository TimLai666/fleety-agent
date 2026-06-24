//! Native Gemini provider — speaks Google's `generateContent` API directly.
//!
//! Unlike the OpenAI shape, Gemini puts the model name in the path and uses a
//! single `inline_data` part type for every media flavour (image / audio /
//! video). This lets video attachments actually reach the model — the
//! OpenAI-compatible shim falls back to `image_url` with a `data:` URL, which
//! pure-OpenAI rejects. Tool-calling is supported via `functionDeclarations`
//! at the request level; the response carries `functionCall` parts.
//!
//! Streaming uses `:streamGenerateContent?alt=sse` and `:streamGenerateContent`
//! is mapped to `complete_streaming`. The non-streaming `complete` uses
//! `:generateContent` and assembles the response in one shot.
//!
//! Endpoint (defaults if not overridden):
//!     https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={KEY}
//!
//! Configure via the same env vars as the OpenAI provider — the server picks
//! Gemini when `FLEETY_MODEL_BASE_URL` looks like a Gemini API root.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::model::{Message, ModelProvider, ModelResponse, Role, ToolCall, ToolSpec};
use crate::{CoreError, Result};

/// Default base URL for the Gemini API. Override via `FLEETY_MODEL_BASE_URL`.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct Gemini {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
    stream: bool,
}

impl Gemini {
    /// Build a Gemini provider. `base_url` typically ends in `/v1beta` (no
    /// trailing slash). `api_key` is sent as a `?key=` query parameter, which
    /// is the simplest Gemini auth shape; pass `None` for endpoints behind
    /// IAM proxies.
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

    pub fn with_streaming(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    fn endpoint(&self, streaming: bool) -> String {
        let path = if streaming {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        let mut url = format!("{}/models/{}:{}", self.base_url, self.model, path);
        if streaming {
            url.push_str("?alt=sse");
        }
        if let Some(key) = &self.api_key {
            url.push_str(if streaming { "&" } else { "?" });
            url.push_str("key=");
            url.push_str(key);
        }
        url
    }
}

#[async_trait::async_trait]
impl ModelProvider for Gemini {
    async fn complete(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<ModelResponse> {
        let request = self.client.post(self.endpoint(false)).json(&build_request(
            messages,
            tools,
            &self.model,
        ));
        let response = request
            .send()
            .await
            .map_err(|e| CoreError::Provider(format!("gemini request failed: {e}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| CoreError::Provider(format!("gemini reading body failed: {e}")))?;
        if !status.is_success() {
            return Err(CoreError::Provider(format!(
                "gemini endpoint returned HTTP {status}: {text}"
            )));
        }
        let parsed: Value = serde_json::from_str(&text).map_err(|e| {
            CoreError::Provider(format!(
                "gemini unexpected response shape: {e}; body: {text}"
            ))
        })?;
        parse_response(&parsed)
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
        let request = self.client.post(self.endpoint(true)).json(&build_request(
            messages,
            tools,
            &self.model,
        ));
        let response = request
            .send()
            .await
            .map_err(|e| CoreError::Provider(format!("gemini request failed: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(CoreError::Provider(format!(
                "gemini endpoint returned HTTP {status}: {text}"
            )));
        }
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut content = String::new();
        let mut function_calls: Vec<ToolCall> = Vec::new();
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let bytes =
                chunk.map_err(|e| CoreError::Provider(format!("gemini stream failed: {e}")))?;
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                if let Some(data) = line.trim().strip_prefix("data:") {
                    let data = data.trim();
                    if data.is_empty() {
                        continue;
                    }
                    if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                        accumulate_sse_chunk(
                            &chunk_json,
                            &mut content,
                            &mut function_calls,
                            on_delta,
                        );
                    }
                }
            }
        }
        Ok(ModelResponse {
            message: Message {
                role: Role::Assistant,
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                tool_calls: function_calls,
                tool_call_id: None,
                attachments: Vec::new(),
            },
        })
    }
}

/// Build the JSON body for a `generateContent` request. Gemini splits the
/// `contents` array by turn (alternating user/model roles) with a separate
/// `tools` block. The `systemInstruction` field carries the system prompt
/// once at request level, not as a first-class role in `contents`.
fn build_request(messages: &[Message], tools: &[ToolSpec], _model: &str) -> Value {
    let mut body = Map::new();
    let (system, contents) = split_messages(messages);
    if let Some(sys) = system {
        body.insert(
            "systemInstruction".to_string(),
            json!({ "role": "user", "parts": [{ "text": sys }] }),
        );
    }
    body.insert("contents".to_string(), Value::Array(contents));
    if !tools.is_empty() {
        let decls: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        body.insert(
            "tools".to_string(),
            json!([{ "functionDeclarations": decls }]),
        );
    }
    Value::Object(body)
}

/// Split a flat `[Message]` into `(systemPrompt, contents[])`. Gemini doesn't
/// have a system role in `contents`; the system prompt rides in
/// `systemInstruction` and the rest becomes alternating user/model parts.
fn split_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut contents = Vec::new();
    for msg in messages {
        match msg.role {
            Role::System => {
                let text = msg.content.clone().unwrap_or_default();
                system = match system {
                    Some(prev) => Some(format!("{prev}\n\n{text}")),
                    None => Some(text),
                };
            }
            Role::User => {
                contents.push(json!({
                    "role": "user",
                    "parts": build_parts(msg),
                }));
            }
            Role::Assistant => {
                let mut parts: Vec<Value> = Vec::new();
                if let Some(text) = &msg.content {
                    if !text.is_empty() {
                        parts.push(json!({ "text": text }));
                    }
                }
                for call in &msg.tool_calls {
                    parts.push(json!({
                        "functionCall": {
                            "name": call.name,
                            "args": call.arguments,
                        }
                    }));
                }
                if parts.is_empty() {
                    parts.push(json!({ "text": "" }));
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Role::Tool => {
                // Tool results land in a `user` turn with a `functionResponse`
                // part — Gemini doesn't have a separate "tool" role.
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": msg.tool_call_id.clone().unwrap_or_default(),
                            "response": parse_tool_payload(msg.content.as_deref().unwrap_or("")),
                        }
                    }]
                }));
            }
        }
    }
    (system, contents)
}

/// Build the `parts` array for a user message: text first, then one
/// `inline_data` (or `file_data`) part per attachment, routed by MIME.
fn build_parts(msg: &Message) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    if let Some(text) = &msg.content {
        if !text.is_empty() {
            parts.push(json!({ "text": text }));
        }
    }
    for att in &msg.attachments {
        // Gemini accepts `inline_data` with raw base64 bytes for image / audio
        // / video / pdf alike — same shape, different mime. URL-only attachments
        // ride along as `file_data { file_uri }` (requires Files API to have
        // staged them; the provider doesn't upload here).
        if let Some(b64) = &att.bytes_b64 {
            parts.push(json!({
                "inline_data": { "mime_type": att.mime, "data": b64 }
            }));
        } else if let Some(url) = &att.url {
            parts.push(json!({
                "file_data": { "mime_type": att.mime, "file_uri": url }
            }));
        } else {
            tracing::warn!(mime = %att.mime, "attachment has neither bytes nor url; dropping");
        }
    }
    if parts.is_empty() {
        parts.push(json!({ "text": "" }));
    }
    parts
}

/// Parse `Message::tool_result` content into a JSON value — the model expects a
/// structured payload here, not the raw string.
fn parse_tool_payload(content: &str) -> Value {
    serde_json::from_str(content).unwrap_or_else(|_| json!({ "result": content }))
}

/// Parse a non-streaming Gemini response into the canonical ModelResponse shape.
fn parse_response(value: &Value) -> Result<ModelResponse> {
    let candidate = value
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| CoreError::Provider("gemini response missing candidates[0]".into()))?;
    let parts = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    for part in &parts {
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            content.push_str(text);
        }
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
            tool_calls.push(ToolCall {
                id: uuid::id(),
                name,
                arguments: args,
            });
        }
    }
    Ok(ModelResponse {
        message: Message {
            role: Role::Assistant,
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls,
            tool_call_id: None,
            attachments: Vec::new(),
        },
    })
}

/// Append one SSE chunk's parts into the running accumulators. Gemini streams
/// full `candidate.content.parts` arrays in each event (not text-diff style
/// like OpenAI), so we just push everything we see.
fn accumulate_sse_chunk(
    chunk: &Value,
    content: &mut String,
    function_calls: &mut Vec<ToolCall>,
    on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
) {
    let parts = chunk
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array());
    let Some(parts) = parts else { return };
    for part in parts {
        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
            content.push_str(text);
            on_delta(text);
        }
        if let Some(fc) = part.get("functionCall") {
            let name = fc
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
            function_calls.push(ToolCall {
                id: uuid::id(),
                name,
                arguments: args,
            });
        }
    }
}

/// We don't pull in the `uuid` crate just for ids in this module; Gemini doesn't
/// assign call ids itself, but the agent loop threads them through `id`. A
/// short counter-based id is enough for matching tool_call ↔ tool_result within
/// one turn.
mod uuid {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    pub fn id() -> String {
        format!("gem-{}", N.fetch_add(1, Ordering::Relaxed))
    }
}

/// Tiny model-name family detector used by the runtime to pick this provider
/// over the OpenAI-compatible one when the user's `FLEETY_MODEL` looks like a
/// Gemini model. Kept here so additions land alongside the provider.
#[allow(dead_code)]
pub fn looks_like_gemini_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("gemini") || m.starts_with("models/gemini")
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct UnusedDeserializeKeepClippyHappy;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Attachment;

    #[test]
    fn endpoint_picks_correct_path_and_streaming_marker() {
        let p = Gemini::new(DEFAULT_BASE_URL, "gemini-1.5-pro", Some("KEY".into()));
        let s = p.endpoint(false);
        assert!(
            s.starts_with(&format!(
                "{}/models/gemini-1.5-pro:generateContent",
                DEFAULT_BASE_URL
            )),
            "{s}"
        );
        assert!(s.contains("key=KEY"), "{s}");
        let stream = p.endpoint(true);
        assert!(stream.contains(":streamGenerateContent"), "{stream}");
        assert!(stream.contains("alt=sse"), "{stream}");
    }

    #[test]
    fn split_messages_pulls_system_into_system_instruction() {
        let msgs = vec![
            Message::system("be terse"),
            Message::user("hi"),
            Message::assistant("hello"),
        ];
        let body = build_request(&msgs, &[], "gemini-1.5-pro");
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            json!("be terse")
        );
        let contents = body["contents"].as_array().expect("contents");
        assert_eq!(contents[0]["role"], json!("user"));
        assert_eq!(contents[0]["parts"][0]["text"], json!("hi"));
        assert_eq!(contents[1]["role"], json!("model"));
    }

    #[test]
    fn attachments_serialize_as_inline_data_for_every_mime() {
        let msg = Message::user_with_attachments(
            "look",
            vec![
                Attachment::from_bytes("image/png", b"\x89PNG"),
                Attachment::from_bytes("audio/mp3", b"\xff"),
                Attachment::from_bytes("video/mp4", b"\x00"),
                Attachment::from_bytes("application/pdf", b"%PDF"),
                Attachment::from_url("video/mp4", "gs://bucket/clip.mp4"),
            ],
        );
        let body = build_request(&[msg], &[], "gemini-1.5-pro");
        let parts = body["contents"][0]["parts"].as_array().expect("parts");
        // text + 4 inline_data + 1 file_data = 6
        assert_eq!(parts.len(), 6);
        assert_eq!(parts[1]["inline_data"]["mime_type"], json!("image/png"));
        assert_eq!(parts[2]["inline_data"]["mime_type"], json!("audio/mp3"));
        assert_eq!(parts[3]["inline_data"]["mime_type"], json!("video/mp4"));
        assert_eq!(
            parts[4]["inline_data"]["mime_type"],
            json!("application/pdf")
        );
        assert_eq!(
            parts[5]["file_data"]["file_uri"],
            json!("gs://bucket/clip.mp4")
        );
    }

    #[test]
    fn parse_response_extracts_text_and_function_calls() {
        let v = json!({
            "candidates": [{
                "content": { "parts": [
                    { "text": "ok," },
                    { "text": " calling tool" },
                    { "functionCall": { "name": "read_file", "args": { "path": "x" } } }
                ]}
            }]
        });
        let r = parse_response(&v).expect("parse");
        assert_eq!(r.message.content.as_deref(), Some("ok, calling tool"));
        assert_eq!(r.message.tool_calls.len(), 1);
        assert_eq!(r.message.tool_calls[0].name, "read_file");
        assert_eq!(r.message.tool_calls[0].arguments, json!({ "path": "x" }));
    }

    #[test]
    fn tool_messages_become_function_response_parts() {
        let mut messages = vec![Message::user("do it")];
        let mut asst = Message::assistant("");
        asst.tool_calls = vec![ToolCall {
            id: "gem-0".into(),
            name: "read_file".into(),
            arguments: json!({ "path": "x" }),
        }];
        messages.push(asst);
        messages.push(Message::tool_result("read_file", "{\"content\":\"hi\"}"));
        let body = build_request(&messages, &[], "gemini-1.5-pro");
        let contents = body["contents"].as_array().expect("contents");
        // user, model, user(functionResponse)
        assert_eq!(contents.len(), 3);
        let fr = &contents[2]["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], json!("read_file"));
        assert_eq!(fr["response"], json!({ "content": "hi" }));
    }

    #[test]
    fn looks_like_gemini_catches_common_names() {
        assert!(looks_like_gemini_model("gemini-1.5-pro"));
        assert!(looks_like_gemini_model("gemini-2.0-flash"));
        assert!(looks_like_gemini_model("models/gemini-1.5-pro-002"));
        assert!(!looks_like_gemini_model("gpt-4o"));
        assert!(!looks_like_gemini_model("claude-sonnet-4-6"));
    }
}
