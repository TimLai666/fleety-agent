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

/// A media attachment carried alongside a user message: an image, audio clip,
/// video, or file the multimodal model is meant to see directly. The agent
/// does NOT pre-process attachments through a vision / OCR / transcription
/// tool — they ride along with the user's text in a single multimodal request,
/// so the model can decide for itself how to interpret them.
///
/// Set exactly one of `bytes_b64` (raw bytes, base64-encoded) or `url` (an
/// HTTP(S) URL the model fetches itself, when supported). `mime` routes the
/// attachment into the right part of the provider's payload: `image/*` → an
/// image part, `audio/*` → an audio part, etc. Providers that don't speak
/// multimodal drop the attachment with a log line rather than failing the turn.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Attachment {
    /// IANA MIME type (e.g. `image/png`, `audio/mpeg`, `video/mp4`).
    pub mime: String,
    /// Inline bytes, base64-encoded. Use for files on disk we want the model
    /// to see directly. Mutually exclusive with `url`; if both are set, `url`
    /// wins (the model handles fetching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_b64: Option<String>,
    /// HTTP(S) URL the model fetches itself. Only useful for providers that
    /// support remote fetch (OpenAI's `image_url`, for example).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Optional filename / short description, for the provider's logs and for
    /// the user-visible event log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Attachment {
    /// Inline a file's bytes as an attachment (base64-encodes for you).
    pub fn from_bytes(mime: impl Into<String>, bytes: &[u8]) -> Self {
        use base64::Engine;
        Self {
            mime: mime.into(),
            bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            url: None,
            name: None,
        }
    }

    /// Reference an external URL the model will fetch.
    pub fn from_url(mime: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            mime: mime.into(),
            bytes_b64: None,
            url: Some(url.into()),
            name: None,
        }
    }

    /// Attach a short name (typically a filename) for logs / display.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
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
    /// User-message attachments handed to a multimodal model alongside `content`.
    /// Empty for non-user messages and for text-only turns. See [`Attachment`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            attachments: Vec::new(),
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            attachments: Vec::new(),
        }
    }

    /// A user message that ships attachments alongside its text. Use this
    /// when the user provides images, audio, or other media for the model.
    pub fn user_with_attachments(text: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            attachments,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            attachments: Vec::new(),
        }
    }

    /// A `tool` role message carrying the result of a tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            attachments: Vec::new(),
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
