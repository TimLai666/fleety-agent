//! Model abstraction: messages, tool specs, and the pluggable provider trait.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CoreError, Result};

/// Conversation roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// A single conversation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A `tool` role message carrying the result of a tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// How risky a tool is to run. Drives gating (read runs freely; mutate is
/// audited/rollback-backed; critical needs confirmation). See the spec policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    #[default]
    Read,
    Mutate,
    Critical,
}

/// A tool the model may call (name + JSON-schema parameters + risk class).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's arguments.
    pub parameters: Value,
    /// Risk class for gating; defaults to `read`.
    #[serde(default)]
    pub risk: RiskLevel,
}

/// A provider's response for one step of the loop.
#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub message: Message,
}

/// A pluggable LLM backend. Implementations: [`MockProvider`] (tests) and the
/// OpenAI-compatible provider (see the `openai` module).
#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    /// Produce the next assistant message, given the conversation and tools.
    async fn complete(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<ModelResponse>;

    /// Like [`complete`](Self::complete) but invokes `on_delta` with each content
    /// chunk as it streams, for token-by-token display. The default ignores
    /// `on_delta` and falls back to a single `complete` call; streaming providers
    /// override it.
    async fn complete_streaming(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        _on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ModelResponse> {
        self.complete(messages, tools).await
    }
}

/// A scripted provider for tests and demos: returns queued responses in order.
pub struct MockProvider {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl MockProvider {
    pub fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for MockProvider {
    async fn complete(&self, _messages: &[Message], _tools: &[ToolSpec]) -> Result<ModelResponse> {
        let mut queue = self
            .responses
            .lock()
            .map_err(|_| CoreError::Provider("mock provider mutex poisoned".to_string()))?;
        queue.pop_front().ok_or_else(|| {
            CoreError::Provider("mock provider ran out of scripted responses".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_constructors_set_roles_and_tool_call_id() {
        assert_eq!(Message::system("rules").role, Role::System);
        assert_eq!(Message::user("hi").role, Role::User);
        assert_eq!(Message::assistant("ok").role, Role::Assistant);

        let tool = Message::tool_result("call-1", "done");
        assert_eq!(tool.role, Role::Tool);
        assert_eq!(tool.content.as_deref(), Some("done"));
        assert_eq!(tool.tool_call_id.as_deref(), Some("call-1"));
        assert!(tool.tool_calls.is_empty());
    }

    #[test]
    fn tool_spec_deserializes_default_risk_as_read() {
        let spec: ToolSpec = serde_json::from_value(json!({
            "name": "read_file",
            "description": "read",
            "parameters": {"type": "object"}
        }))
        .expect("tool spec");
        assert_eq!(spec.risk, RiskLevel::Read);
    }

    #[tokio::test]
    async fn mock_provider_returns_scripted_responses_then_errors() {
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant("one"),
        }]);

        let first = provider.complete(&[], &[]).await.expect("first response");
        assert_eq!(first.message.content.as_deref(), Some("one"));

        let err = provider
            .complete(&[], &[])
            .await
            .expect_err("empty script should error");
        assert!(err.report().message.contains("ran out"));
    }
}
