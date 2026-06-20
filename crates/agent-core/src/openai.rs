//! OpenAI-compatible model provider.
//!
//! Speaks the `/chat/completions` API (OpenAI, OpenRouter, LM Studio, Ollama,
//! vLLM, …). M1: non-streaming request/response with tool-calling. Token
//! streaming over SSE is a later addition; the loop works without it.

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
        }
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

        let mut request = self.client.post(self.endpoint()).json(&Value::Object(body));
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

        let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
            CoreError::Provider(format!("unexpected response shape: {e}; body: {text}"))
        })?;
        parse_response(parsed)
    }
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
}
