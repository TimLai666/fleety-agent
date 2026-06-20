//! A stand-in model provider for M2: echoes the last user message.
//!
//! Lets the WebSocket/session/persistence plumbing be exercised end-to-end with
//! no network or API key. Real providers (OpenAI-compatible) are wired in a
//! later milestone via config.

use agent_core::{Message, ModelProvider, ModelResponse, Result, Role};

pub struct EchoProvider;

#[async_trait::async_trait]
impl ModelProvider for EchoProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _tools: &[agent_core::ToolSpec],
    ) -> Result<ModelResponse> {
        let last_user = messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ModelResponse {
            message: Message::assistant(format!("echo: {last_user}")),
        })
    }
}
