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

/// Token usage a provider reported for one model call.
///
/// Only ever present when the provider actually reported it: a missing usage is
/// `Option::None`, never a zeroed `TokenUsage`, so "unknown" stays
/// distinguishable from "spent zero tokens". `cached_input` is the part of
/// `input` the provider served from its own prompt cache, when it says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub total: u64,
    /// Input tokens served from the provider's cache, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<u64>,
}

impl TokenUsage {
    /// Fold two usages together, summing every counter. `cached_input` is summed
    /// only across the sides that reported one; if neither did, it stays `None`.
    pub fn merge(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            total: self.total.saturating_add(other.total),
            cached_input: match (self.cached_input, other.cached_input) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
            },
        }
    }

    /// Sum the reported usages in `parts`, ignoring the unknown ones. Returns
    /// `None` when nothing reported — never a zeroed total, which would claim
    /// the calls were free rather than unmeasured.
    pub fn sum(parts: impl IntoIterator<Item = Option<Self>>) -> Option<Self> {
        parts
            .into_iter()
            .flatten()
            .fold(None, |acc: Option<Self>, u| {
                Some(acc.map_or(u, |a| a.merge(u)))
            })
    }
}

/// A provider's response for one step of the loop.
#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub message: Message,
    /// Token usage for this call, when the provider reported it.
    pub usage: Option<TokenUsage>,
}

impl ModelResponse {
    /// A response with no usage reported.
    pub fn new(message: Message) -> Self {
        Self {
            message,
            usage: None,
        }
    }

    /// Attach provider-reported usage.
    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = Some(usage);
        self
    }
}

impl From<Message> for ModelResponse {
    fn from(message: Message) -> Self {
        Self::new(message)
    }
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

    /// Which input modalities this model accepts. The default reports full
    /// support so existing providers/tests behave exactly as before; real
    /// providers report their configured/derived capabilities so unsupported
    /// attachments degrade gracefully instead of being sent and rejected.
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::ALL
    }

    /// Return a variant of this provider that applies `effort` to its model
    /// calls, or `None` if this provider doesn't carry a reasoning effort (the
    /// default — so existing impls/tests are unaffected). The caller applies the
    /// agent's per-turn effort or a subagent's spawn-time effort here.
    fn with_effort(&self, _effort: Option<Effort>) -> Option<std::sync::Arc<dyn ModelProvider>> {
        None
    }
}

/// A reasoning-effort level the agent or a parent can request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }

    /// Parse `low`/`medium`/`high` (case-insensitive); anything else is `None`.
    pub fn parse(s: &str) -> Option<Effort> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Effort::Low),
            "medium" => Some(Effort::Medium),
            "high" => Some(Effort::High),
            _ => None,
        }
    }
}

/// How a model family encodes reasoning effort in its request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortScheme {
    /// OpenAI-style top-level `reasoning_effort: "low|medium|high"`.
    OpenAiReasoning,
    /// Gemini-style `generationConfig.thinkingConfig.thinkingBudget`.
    GeminiThinking,
    /// The model doesn't accept an effort field — omit it.
    None,
}

/// The request field (key + JSON value) to merge for `(scheme, effort)`, or
/// `None` when nothing should be sent (no effort, or an effort-less scheme).
/// Pure, so the mapping is unit-testable.
pub fn effort_field(
    scheme: EffortScheme,
    effort: Option<Effort>,
) -> Option<(&'static str, serde_json::Value)> {
    let e = effort?;
    match scheme {
        EffortScheme::OpenAiReasoning => Some(("reasoning_effort", serde_json::json!(e.as_str()))),
        EffortScheme::GeminiThinking => {
            // Coarse low/medium/high → thinking-token budget (adjustable).
            let budget = match e {
                Effort::Low => 512,
                Effort::Medium => 4096,
                Effort::High => 24576,
            };
            Some((
                "generationConfig",
                serde_json::json!({ "thinkingConfig": { "thinkingBudget": budget } }),
            ))
        }
        EffortScheme::None => None,
    }
}

/// A non-text input modality an attachment can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Image,
    Audio,
    Pdf,
    /// An unrecognized MIME type — always degraded to a text note.
    Other,
}

/// Classify an attachment's MIME type into a modality.
pub fn mime_modality(mime: &str) -> Modality {
    let m = mime.to_ascii_lowercase();
    if m.starts_with("image/") {
        Modality::Image
    } else if m.starts_with("audio/") {
        Modality::Audio
    } else if m == "application/pdf" {
        Modality::Pdf
    } else {
        Modality::Other
    }
}

/// Which input modalities a model accepts (text is always supported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub image: bool,
    pub audio: bool,
    pub pdf: bool,
}

impl ModelCapabilities {
    /// Full multimodal support.
    pub const ALL: Self = Self {
        image: true,
        audio: true,
        pdf: true,
    };
    /// Text only — no media attachments.
    pub const TEXT_ONLY: Self = Self {
        image: false,
        audio: false,
        pdf: false,
    };

    /// Whether this model accepts the given modality. `Other` (unknown MIME) is
    /// never "supported" — it always degrades to a text note.
    pub fn supports(&self, m: Modality) -> bool {
        match m {
            Modality::Image => self.image,
            Modality::Audio => self.audio,
            Modality::Pdf => self.pdf,
            Modality::Other => false,
        }
    }
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self::ALL
    }
}

/// Parse a comma-separated modality list (e.g. `text,image`) into capabilities.
/// `text` is implicit; unknown tokens are ignored; an empty string is text-only.
pub fn parse_modalities(s: &str) -> ModelCapabilities {
    let mut caps = ModelCapabilities::TEXT_ONLY;
    for tok in s.split(',') {
        match tok.trim().to_ascii_lowercase().as_str() {
            "image" => caps.image = true,
            "audio" => caps.audio = true,
            "pdf" => caps.pdf = true,
            _ => {} // "text" / "" / unknown
        }
    }
    caps
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
mod capability_tests {
    use super::*;

    #[test]
    fn mime_modality_classifies() {
        assert_eq!(mime_modality("image/png"), Modality::Image);
        assert_eq!(mime_modality("AUDIO/MP3"), Modality::Audio);
        assert_eq!(mime_modality("application/pdf"), Modality::Pdf);
        assert_eq!(mime_modality("text/csv"), Modality::Other);
    }

    #[test]
    fn parse_modalities_table() {
        let c = parse_modalities("text,image");
        assert!(c.image && !c.audio && !c.pdf);
        let all = parse_modalities("text,image,audio,pdf");
        assert!(all.image && all.audio && all.pdf);
        let empty = parse_modalities("");
        assert!(!empty.image && !empty.audio && !empty.pdf);
        let bogus = parse_modalities("text,bogus");
        assert!(!bogus.image && !bogus.audio && !bogus.pdf);
    }

    #[test]
    fn supports_respects_flags_and_other() {
        let c = parse_modalities("text,image");
        assert!(c.supports(Modality::Image));
        assert!(!c.supports(Modality::Audio));
        assert!(!c.supports(Modality::Other));
    }

    #[test]
    fn effort_parses() {
        assert_eq!(Effort::parse("HIGH"), Some(Effort::High));
        assert_eq!(Effort::parse(" low "), Some(Effort::Low));
        assert_eq!(Effort::parse("xl"), None);
    }

    #[test]
    fn effort_field_per_scheme() {
        // OpenAI reasoning: top-level string.
        let (k, v) = effort_field(EffortScheme::OpenAiReasoning, Some(Effort::High)).unwrap();
        assert_eq!(k, "reasoning_effort");
        assert_eq!(v, serde_json::json!("high"));
        // Gemini thinking: a generationConfig.thinkingConfig budget.
        let (k, v) = effort_field(EffortScheme::GeminiThinking, Some(Effort::Medium)).unwrap();
        assert_eq!(k, "generationConfig");
        assert!(v["thinkingConfig"]["thinkingBudget"].as_i64().unwrap() > 0);
        // None scheme, or no effort → no field.
        assert!(effort_field(EffortScheme::None, Some(Effort::High)).is_none());
        assert!(effort_field(EffortScheme::OpenAiReasoning, None).is_none());
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

    /// Usage is optional and "unknown" must stay distinguishable from "zero
    /// tokens": a provider that reports nothing leaves `usage` as `None`, never
    /// as a zeroed `TokenUsage`.
    #[test]
    fn unreported_usage_is_none_not_zero() {
        let silent = ModelResponse::new(Message::assistant("hi"));
        assert!(silent.usage.is_none());
        assert_ne!(silent.usage, Some(TokenUsage::default()));

        let reported = ModelResponse::new(Message::assistant("hi")).with_usage(TokenUsage {
            input: 1000,
            output: 50,
            total: 1050,
            cached_input: Some(800),
        });
        assert_eq!(reported.usage.map(|u| u.total), Some(1050));
        assert_eq!(reported.usage.and_then(|u| u.cached_input), Some(800));
    }

    /// Summing folds only the calls that reported, and stays `None` when none did.
    #[test]
    fn usage_sum_skips_unknown_and_stays_none_when_all_unknown() {
        assert_eq!(TokenUsage::sum([None, None]), None);

        let a = TokenUsage {
            input: 1000,
            output: 50,
            total: 1050,
            cached_input: Some(200),
        };
        let c = TokenUsage {
            input: 1200,
            output: 80,
            total: 1280,
            cached_input: None,
        };
        let summed = TokenUsage::sum([Some(a), None, Some(c)]).expect("some reported");
        assert_eq!(summed.input, 2200);
        assert_eq!(summed.output, 130);
        assert_eq!(summed.total, 2330);
        assert_eq!(summed.cached_input, Some(200));
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
        let provider = MockProvider::new(vec![ModelResponse::new(Message::assistant("one"))]);

        let first = provider.complete(&[], &[]).await.expect("first response");
        assert_eq!(first.message.content.as_deref(), Some("one"));

        let err = provider
            .complete(&[], &[])
            .await
            .expect_err("empty script should error");
        assert!(err.report().message.contains("ran out"));
    }
}
