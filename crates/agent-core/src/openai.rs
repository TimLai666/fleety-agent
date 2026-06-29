//! OpenAI-compatible model provider.
//!
//! Speaks the `/chat/completions` API (OpenAI, OpenRouter, LM Studio, Ollama,
//! vLLM, …): non-streaming request/response with tool-calling, plus SSE
//! streaming (`with_streaming` / `complete_streaming`) for token-by-token output.

use serde::Deserialize;
use serde_json::{json, Map, Value};

use std::sync::Arc;

use crate::model::{
    Effort, EffortScheme, Message, ModelCapabilities, ModelProvider, ModelResponse, Role, ToolCall,
    ToolSpec,
};
use crate::{CoreError, Result};

/// A provider backed by any OpenAI-compatible `/chat/completions` endpoint.
pub struct OpenAiCompat {
    base_url: String,
    model: String,
    api_key: Option<String>,
    client: reqwest::Client,
    stream: bool,
    retry: crate::retry::RetryConfig,
    caps: ModelCapabilities,
    effort: Option<Effort>,
    effort_scheme: EffortScheme,
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
            retry: crate::retry::RetryConfig::from_env(),
            caps: ModelCapabilities::ALL,
            effort: None,
            effort_scheme: EffortScheme::None,
        }
    }

    /// Set the model's input modality capabilities (unsupported attachments
    /// then degrade to a text note instead of being sent and rejected).
    pub fn with_capabilities(mut self, caps: ModelCapabilities) -> Self {
        self.caps = caps;
        self
    }

    /// Set how this model encodes reasoning effort and its default effort.
    pub fn with_effort_config(mut self, scheme: EffortScheme, default: Option<Effort>) -> Self {
        self.effort_scheme = scheme;
        self.effort = default;
        self
    }

    fn clone_with_effort(&self, effort: Option<Effort>) -> OpenAiCompat {
        OpenAiCompat {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            client: self.client.clone(),
            stream: self.stream,
            retry: self.retry,
            caps: self.caps,
            effort,
            effort_scheme: self.effort_scheme,
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
        let body = self.request_body(messages, tools);
        // Retry transient failures (429/5xx/connection/timeout) with backoff;
        // 4xx fail fast. The whole request is re-sent on each attempt.
        let text = crate::retry::run_with_retry(&self.retry, || {
            let mut request = self.client.post(self.endpoint()).json(&body);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            async move {
                use crate::retry::{classify, AttemptOutcome, Retryable};
                let response = match request.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        let transient = e.is_timeout() || e.is_connect();
                        let err = CoreError::Provider(format!(
                            "request to {} failed: {e}",
                            self.endpoint()
                        ));
                        return match classify(None, transient) {
                            Retryable::Retry => AttemptOutcome::Retry {
                                err,
                                retry_after: None,
                            },
                            Retryable::Fatal => AttemptOutcome::Fatal(err),
                        };
                    }
                };
                let status = response.status();
                let retry_after = crate::retry::parse_retry_after(
                    response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                );
                let text = match response.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        return AttemptOutcome::Fatal(CoreError::Provider(format!(
                            "reading response body failed: {e}"
                        )))
                    }
                };
                if status.is_success() {
                    return AttemptOutcome::Done(text);
                }
                let err = CoreError::Provider(format!("endpoint returned HTTP {status}: {text}"));
                match classify(Some(status.as_u16()), false) {
                    Retryable::Retry => AttemptOutcome::Retry { err, retry_after },
                    Retryable::Fatal => AttemptOutcome::Fatal(err),
                }
            }
        })
        .await?;

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
        let body = self.request_body(messages, tools);
        // Retry only the connection + initial HTTP status (before any delta is
        // emitted). Once the stream is producing output we never retry — that
        // would duplicate already-emitted tokens.
        let response = crate::retry::run_with_retry(&self.retry, || {
            let mut request = self.client.post(self.endpoint()).json(&body);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            async move {
                use crate::retry::{classify, AttemptOutcome, Retryable};
                let response = match request.send().await {
                    Ok(r) => r,
                    Err(e) => {
                        let transient = e.is_timeout() || e.is_connect();
                        let err = CoreError::Provider(format!(
                            "request to {} failed: {e}",
                            self.endpoint()
                        ));
                        return match classify(None, transient) {
                            Retryable::Retry => AttemptOutcome::Retry {
                                err,
                                retry_after: None,
                            },
                            Retryable::Fatal => AttemptOutcome::Fatal(err),
                        };
                    }
                };
                let status = response.status();
                if status.is_success() {
                    return AttemptOutcome::Done(response);
                }
                let retry_after = crate::retry::parse_retry_after(
                    response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                );
                let text = response.text().await.unwrap_or_default();
                let err = CoreError::Provider(format!("endpoint returned HTTP {status}: {text}"));
                match classify(Some(status.as_u16()), false) {
                    Retryable::Retry => AttemptOutcome::Retry { err, retry_after },
                    Retryable::Fatal => AttemptOutcome::Fatal(err),
                }
            }
        })
        .await?;

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

    fn capabilities(&self) -> ModelCapabilities {
        self.caps
    }

    fn with_effort(&self, effort: Option<Effort>) -> Option<Arc<dyn ModelProvider>> {
        Some(Arc::new(self.clone_with_effort(effort)))
    }
}

impl OpenAiCompat {
    fn request_body(&self, messages: &[Message], tools: &[ToolSpec]) -> Value {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(self.model));
        body.insert(
            "messages".to_string(),
            json!(messages
                .iter()
                .map(|m| wire_message(m, self.caps))
                .collect::<Vec<_>>()),
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
        if let Some((key, value)) = crate::model::effort_field(self.effort_scheme, self.effort) {
            body.insert(key.to_string(), value);
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
                attachments: Vec::new(),
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

fn wire_message(message: &Message, caps: ModelCapabilities) -> Value {
    let mut object = Map::new();
    object.insert("role".to_string(), json!(role_str(message.role)));
    object.insert("content".to_string(), wire_content(message, caps));
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

/// Build the OpenAI `content` field. With no attachments we emit the plain
/// string the rest of the loop expects. With attachments — only legal on user
/// messages — we emit the multimodal content-array format: a `text` part
/// followed by one `image_url` / `input_audio` part per attachment, routed by
/// MIME. Unsupported MIME types are dropped with a tracing warning rather
/// than failing the turn (the model still sees the rest).
fn wire_content(message: &Message, caps: ModelCapabilities) -> Value {
    if message.attachments.is_empty() {
        return message
            .content
            .as_ref()
            .map(|c| json!(c))
            .unwrap_or(Value::Null);
    }
    // A short text note for an attachment we won't send (model lacks the
    // capability, or an unknown MIME) — so the model still knows it existed.
    let note = |att: &crate::model::Attachment| -> Value {
        let text = match (&att.name, &att.url) {
            (Some(name), _) => format!("[attachment: {} ({})]", name, att.mime),
            (None, Some(url)) => format!("[attachment: {} at {}]", att.mime, url),
            (None, None) => format!("[attachment: {} (omitted)]", att.mime),
        };
        json!({ "type": "text", "text": text })
    };
    let mut parts: Vec<Value> = Vec::new();
    if let Some(text) = &message.content {
        if !text.is_empty() {
            parts.push(json!({ "type": "text", "text": text }));
        }
    }
    for att in &message.attachments {
        let mime = att.mime.to_ascii_lowercase();
        let is_visual = mime.starts_with("image/") || mime.starts_with("video/");
        if is_visual && caps.image {
            // For images this is the standard OpenAI `image_url` part; for video
            // it's how OpenRouter/Gemini wrappers accept clips (a `data:` URL
            // in the same slot). Pure OpenAI rejects video here — that's an
            // honest error for the user, not a silent text-note degradation.
            let url = if let Some(u) = &att.url {
                u.clone()
            } else if let Some(b64) = &att.bytes_b64 {
                format!("data:{};base64,{}", att.mime, b64)
            } else {
                tracing::warn!(mime = %att.mime, "media attachment has no bytes or url; skipping");
                continue;
            };
            parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
        } else if mime.starts_with("audio/") && caps.audio {
            // `gpt-4o-audio-preview` style: data is bare base64 (no data: prefix),
            // format is the codec hint after the slash (mp3 / wav / ogg / …).
            let Some(b64) = att.bytes_b64.as_ref().or(att.url.as_ref()) else {
                tracing::warn!(mime = %att.mime, "audio attachment empty; skipping");
                continue;
            };
            let format = mime
                .strip_prefix("audio/")
                .map(str::to_string)
                .unwrap_or_else(|| "mp3".to_string());
            parts.push(json!({
                "type": "input_audio",
                "input_audio": { "data": b64, "format": format },
            }));
        } else if mime == "application/pdf" && caps.pdf {
            // OpenAI's PDF input shape: a `file` part with a data: URL.
            let Some(b64) = &att.bytes_b64 else {
                tracing::warn!("pdf attachment has no bytes; skipping");
                continue;
            };
            let url = format!("data:application/pdf;base64,{b64}");
            let mut file = json!({ "file_data": url });
            if let Some(name) = &att.name {
                if let Some(obj) = file.as_object_mut() {
                    obj.insert("filename".to_string(), json!(name));
                }
            }
            parts.push(json!({ "type": "file", "file": file }));
        } else {
            // The model lacks this modality, or the MIME is unknown: surface a
            // text note so the model still knows an attachment existed, rather
            // than sending media the endpoint would reject.
            tracing::warn!(mime = %att.mime, "attachment not sent (model lacks capability or unknown mime); text note");
            parts.push(note(att));
        }
    }
    if parts.is_empty() {
        // Every attachment was dropped and there was no text either — fall
        // back to null so we don't send an empty array.
        return Value::Null;
    }
    Value::Array(parts)
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
            attachments: Vec::new(),
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
    use crate::model::Attachment;

    #[test]
    fn wire_content_falls_back_to_string_with_no_attachments() {
        let msg = Message::user("hello");
        assert_eq!(wire_content(&msg, ModelCapabilities::ALL), json!("hello"));
    }

    #[test]
    fn wire_content_degrades_image_to_note_for_text_only_model() {
        let att = Attachment::from_bytes("image/png", b"\x89PNG\r\n");
        let msg = Message::user_with_attachments("what is this?", vec![att]);
        let v = wire_content(&msg, ModelCapabilities::TEXT_ONLY);
        let arr = v.as_array().expect("array");
        // No image_url part — the image is replaced by a text note.
        assert!(arr.iter().all(|p| p["type"] != json!("image_url")));
        assert!(arr.iter().any(|p| p["type"] == json!("text")
            && p["text"]
                .as_str()
                .unwrap_or_default()
                .contains("attachment")));
    }

    #[test]
    fn wire_content_emits_multimodal_array_for_image_attachment() {
        // Inline image bytes → data: URL in an image_url part.
        let att = Attachment::from_bytes("image/png", b"\x89PNG\r\n");
        let msg = Message::user_with_attachments("what is this?", vec![att]);
        let v = wire_content(&msg, ModelCapabilities::ALL);
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], json!("text"));
        assert_eq!(arr[0]["text"], json!("what is this?"));
        assert_eq!(arr[1]["type"], json!("image_url"));
        let url = arr[1]["image_url"]["url"].as_str().expect("url");
        assert!(url.starts_with("data:image/png;base64,"));

        // External URL → bare URL (no data: prefix).
        let att = Attachment::from_url("image/jpeg", "https://example.com/cat.jpg");
        let msg = Message::user_with_attachments("describe", vec![att]);
        let arr = wire_content(&msg, ModelCapabilities::ALL)
            .as_array()
            .cloned()
            .expect("array");
        assert_eq!(
            arr[1]["image_url"]["url"],
            json!("https://example.com/cat.jpg")
        );
    }

    #[test]
    fn wire_content_emits_input_audio_for_audio_attachment() {
        let att = Attachment::from_bytes("audio/mp3", b"\xff\xfb\x90");
        let msg = Message::user_with_attachments("transcribe", vec![att]);
        let arr = wire_content(&msg, ModelCapabilities::ALL)
            .as_array()
            .cloned()
            .expect("array");
        assert_eq!(arr[1]["type"], json!("input_audio"));
        assert_eq!(arr[1]["input_audio"]["format"], json!("mp3"));
        assert!(arr[1]["input_audio"]["data"].is_string());
    }

    #[test]
    fn wire_content_routes_video_through_image_url() {
        // OpenRouter / Gemini wrappers accept video in the `image_url` slot
        // with a `data:` URL. Pure OpenAI rejects it — that's an honest error
        // for the user, not a silent text-note degrade.
        let att = Attachment::from_bytes("video/mp4", b"\x00\x00\x00").with_name("clip.mp4");
        let msg = Message::user_with_attachments("look", vec![att]);
        let arr = wire_content(&msg, ModelCapabilities::ALL)
            .as_array()
            .cloned()
            .expect("array");
        assert_eq!(arr[1]["type"], json!("image_url"));
        let url = arr[1]["image_url"]["url"].as_str().expect("url");
        assert!(url.starts_with("data:video/mp4;base64,"));
    }

    #[test]
    fn wire_content_routes_pdf_through_file_part() {
        let att = Attachment::from_bytes("application/pdf", b"%PDF-1.4").with_name("report.pdf");
        let msg = Message::user_with_attachments("summarise", vec![att]);
        let arr = wire_content(&msg, ModelCapabilities::ALL)
            .as_array()
            .cloned()
            .expect("array");
        assert_eq!(arr[1]["type"], json!("file"));
        assert_eq!(arr[1]["file"]["filename"], json!("report.pdf"));
        assert!(arr[1]["file"]["file_data"]
            .as_str()
            .unwrap_or("")
            .starts_with("data:application/pdf;base64,"));
    }

    #[test]
    fn wire_content_drops_unknown_mime_into_text_note() {
        let att = Attachment::from_bytes("application/x-weird", b"\x00").with_name("blob.bin");
        let msg = Message::user_with_attachments("look", vec![att]);
        let arr = wire_content(&msg, ModelCapabilities::ALL)
            .as_array()
            .cloned()
            .expect("array");
        assert_eq!(arr[1]["type"], json!("text"));
        assert!(arr[1]["text"].as_str().unwrap_or("").contains("blob.bin"));
    }

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
            attachments: Vec::new(),
        };
        let wire = wire_message(&msg, ModelCapabilities::ALL);
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
        let wire = wire_message(
            &Message::tool_result("call-1", "ok"),
            ModelCapabilities::ALL,
        );

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
        let mut provider = OpenAiCompat::new(base, "model-a", None);
        provider.retry.retries = 0;
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
