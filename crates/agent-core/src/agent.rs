//! The tool-calling loop.
//!
//! Drives a [`ModelProvider`] and a [`ToolRegistry`]: ask the model, run any
//! tool calls it requests, feed the results back, and repeat until the model
//! returns a final answer (or `max_steps` is hit).

use serde_json::json;

use crate::approval::{ApprovalDecision, ApprovalGate, Policy};
use crate::event::{Event, EventLog};
use crate::model::{Message, ModelProvider, RiskLevel, Role, ToolSpec};
use crate::tools::ToolRegistry;
use crate::{CoreError, Result};

/// Tuning for the loop.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// Maximum provider calls before giving up.
    pub max_steps: usize,
    /// Max characters of a tool result fed back to the model. Larger results are
    /// truncated for the model (spec §10.1 tool-output budgeting); the **full**
    /// result is always kept in the event log, so truncation is reversible.
    pub max_tool_result_chars: usize,
    /// When the in-context messages exceed this many characters, older turns are
    /// summarized (compaction); the full history stays in the event log.
    pub compact_threshold_chars: usize,
    /// Number of most-recent messages kept verbatim during compaction.
    pub recent_keep_messages: usize,
    /// Voice mode: when on, the turn produces a second spoken channel. The loop
    /// injects the dual-channel contract into the system prompt and splits the
    /// final reply at [`SPEECH_SENTINEL`] into display + speech. When off (the
    /// default) nothing is injected and no speech is produced, so it costs no
    /// extra tokens.
    pub voice: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 16,
            max_tool_result_chars: 8000,
            compact_threshold_chars: 24_000,
            recent_keep_messages: 8,
            voice: false,
        }
    }
}

/// The result of one agent turn.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub output: String,
    /// The spoken (voice) version of the reply, set only when voice mode is on
    /// and the model emitted a [`SPEECH_SENTINEL`]-delimited spoken part.
    pub speech: Option<String>,
    /// A device-deixis attention hint, set only when voice mode is on and the
    /// model emitted an [`ATTENTION_SENTINEL`]-delimited block.
    pub attention: Option<AttentionHint>,
    pub steps: usize,
}

/// A device-deixis attention hint parsed from a voice-mode reply: which device
/// to look at, what to look at, and an optional url/path. Host-free — the server
/// maps this to its own wire type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttentionHint {
    pub device: String,
    pub look_at: String,
    pub url: Option<String>,
}

/// Marker the model emits in a voice-mode turn to separate the display reply
/// from the spoken version: text before it is display, text after it is speech.
/// Chosen as an unusual string so ordinary replies never collide with it.
pub const SPEECH_SENTINEL: &str = "⟦SPEECH⟧";

/// Marker the model emits in a voice-mode turn before an optional attention
/// block (`device=…; look=…; url=…`). See [`AttentionHint`].
pub const ATTENTION_SENTINEL: &str = "⟦ATTENTION⟧";

/// The system addendum injected when voice mode is on, teaching the model the
/// dual-channel output contract.
fn voice_instruction() -> String {
    format!(
        "Voice mode is ON for this turn. In your FINAL reply (the message with no \
         tool calls), output two channels: first your normal display reply, then on \
         its own line the exact marker {SPEECH_SENTINEL} followed by a short, plain \
         spoken version of the same reply — no markdown, natural spoken sentences, \
         the way you would say it aloud. Use the marker only in the final reply, \
         never in intermediate tool-call turns. If there is nothing to say aloud, \
         omit the marker entirely. Optionally, to point the user at something on a \
         device, after the spoken version add a line with the exact marker \
         {ATTENTION_SENTINEL} then on the next line \
         `device=<id>; look=<what to look at>; url=<optional url or path>`. Omit \
         this marker when you are not directing attention anywhere."
    )
}

/// Ensure the leading system message carries the voice contract, so it survives
/// compaction (which always keeps the head system message). Inserts a system
/// message if there is no leading one. Ephemeral: this mutates the in-context
/// `messages`, never persisted history.
fn apply_voice_prompt(messages: &mut Vec<Message>) {
    let addendum = voice_instruction();
    match messages.first_mut() {
        Some(first) if first.role == Role::System => {
            let base = first.content.take().unwrap_or_default();
            first.content = Some(format!("{base}\n\n{addendum}"));
        }
        _ => messages.insert(0, Message::system(addendum)),
    }
}

/// Split a final reply into (display, speech) at [`SPEECH_SENTINEL`]. No marker
/// → the whole text is display and speech is `None`; an empty spoken part also
/// yields `None`.
fn split_speech(output: &str) -> (String, Option<String>) {
    match output.find(SPEECH_SENTINEL) {
        Some(idx) => {
            let display = output[..idx].trim_end().to_string();
            let spoken = output[idx + SPEECH_SENTINEL.len()..].trim();
            let speech = if spoken.is_empty() {
                None
            } else {
                Some(spoken.to_string())
            };
            (display, speech)
        }
        None => (output.to_string(), None),
    }
}

/// Parse an attention block (`device=…; look=…; url=…`) into an [`AttentionHint`].
/// Returns `None` unless both `device` and `look` are present and non-empty.
fn parse_attention(block: &str) -> Option<AttentionHint> {
    let (mut device, mut look_at, mut url) = (None, None, None);
    for part in block.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            let v = v.trim().to_string();
            match k.trim() {
                "device" => device = Some(v),
                "look" => look_at = Some(v),
                "url" if !v.is_empty() => url = Some(v),
                _ => {}
            }
        }
    }
    match (device, look_at) {
        (Some(d), Some(l)) if !d.is_empty() && !l.is_empty() => Some(AttentionHint {
            device: d,
            look_at: l,
            url,
        }),
        _ => None,
    }
}

/// Split a voice-mode final reply into (display, speech, attention): peel off an
/// optional `⟦ATTENTION⟧` block first, then split the remainder at the speech
/// marker. Missing markers / malformed attention yield `None` for that channel
/// without affecting the others.
fn split_channels(output: &str) -> (String, Option<String>, Option<AttentionHint>) {
    let (rest, attention) = match output.find(ATTENTION_SENTINEL) {
        Some(idx) => {
            let block = output[idx + ATTENTION_SENTINEL.len()..].trim();
            (&output[..idx], parse_attention(block))
        }
        None => (output, None),
    };
    let (display, speech) = split_speech(rest);
    (display, speech, attention)
}

/// Run the tool-calling loop for one user turn.
///
/// Appends the assistant and tool messages to `messages` and records `events`.
/// A failing tool is **not fatal**: its actionable error report is fed back as a
/// tool result so the model can recover, rather than aborting the turn. Large
/// tool results are budgeted before being fed back (full copy kept in `events`).
#[allow(clippy::too_many_arguments)]
pub async fn run_turn(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    messages: &mut Vec<Message>,
    events: &mut EventLog,
    config: &LoopConfig,
    policy: Policy,
    gate: &mut dyn ApprovalGate,
) -> Result<TurnOutcome> {
    let mut noop: Box<dyn FnMut(&str) + Send> = Box::new(|_| {});
    run_turn_streaming(
        provider,
        tools,
        messages,
        events,
        config,
        policy,
        gate,
        noop.as_mut(),
    )
    .await
}

/// Like [`run_turn`], but streams assistant content chunks to `on_delta` as the
/// model produces them (token-by-token display). Tool calls and the final
/// message are unaffected.
#[allow(clippy::too_many_arguments)]
pub async fn run_turn_streaming(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    messages: &mut Vec<Message>,
    events: &mut EventLog,
    config: &LoopConfig,
    policy: Policy,
    gate: &mut dyn ApprovalGate,
    on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
) -> Result<TurnOutcome> {
    let specs = tools.specs();

    // Voice mode: teach the model the dual-channel contract via the system
    // prompt before the loop. Off → nothing injected, no extra tokens.
    if config.voice {
        apply_voice_prompt(messages);
    }

    for step in 1..=config.max_steps {
        compact_if_needed(provider, messages, config).await?;
        let response = provider
            .complete_streaming(messages, &specs, on_delta)
            .await?;
        let assistant = response.message;
        events.push(Event::Assistant(assistant.clone()));
        messages.push(assistant.clone());

        if assistant.tool_calls.is_empty() {
            let raw = assistant.content.unwrap_or_default();
            // Voice on: split the final reply into display + spoken + attention.
            let (output, speech, attention) = if config.voice {
                split_channels(&raw)
            } else {
                (raw, None, None)
            };
            return Ok(TurnOutcome {
                output,
                speech,
                attention,
                steps: step,
            });
        }

        for call in &assistant.tool_calls {
            events.push(Event::ToolCall(call.clone()));

            // Gate the call by policy/risk; a denial is fed back, not executed.
            let risk = risk_of(&specs, &call.name);
            if policy.needs_approval(risk) {
                if let ApprovalDecision::Deny =
                    gate.request(&call.name, &call.arguments, risk).await?
                {
                    let denied = json!({
                        "denied": true,
                        "tool": call.name,
                        "reason": "the user denied this action; do not retry it"
                    });
                    events.push(Event::ToolResult {
                        id: call.id.clone(),
                        result: denied.clone(),
                    });
                    messages.push(Message::tool_result(call.id.clone(), denied.to_string()));
                    continue;
                }
            }

            let result = match tools.call(&call.name, call.arguments.clone()).await {
                Ok(value) => value,
                Err(err) => json!({ "error": err.report() }),
            };
            // Full result to the event log (truth); budgeted copy to the model.
            events.push(Event::ToolResult {
                id: call.id.clone(),
                result: result.clone(),
            });
            let fed = crate::compress::compress_tool_result(
                &result,
                config.max_tool_result_chars,
                &call.id,
            );
            messages.push(Message::tool_result(call.id.clone(), fed));
        }
    }

    Err(CoreError::Provider(format!(
        "reached max steps ({}) without a final answer; raise max_steps or simplify the task",
        config.max_steps
    )))
}

/// Look up a tool's declared risk by name (defaults to read if unknown).
fn risk_of(specs: &[ToolSpec], name: &str) -> RiskLevel {
    specs
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.risk)
        .unwrap_or(RiskLevel::Read)
}

/// Estimate the in-context size of `messages` (chars across content + tool args).
fn estimate_chars(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            m.content.as_ref().map(String::len).unwrap_or(0)
                + m.tool_calls
                    .iter()
                    .map(|c| c.arguments.to_string().len())
                    .sum::<usize>()
        })
        .sum()
}

fn render_message(m: &Message) -> String {
    let role = match m.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    format!("{role}: {}", m.content.clone().unwrap_or_default())
}

/// If the context is over budget, summarize the older middle messages via the
/// provider, keeping a leading system message and the most recent ones verbatim.
/// Reversible: the full history lives in the event log; only the in-context view
/// shrinks.
async fn compact_if_needed(
    provider: &dyn ModelProvider,
    messages: &mut Vec<Message>,
    config: &LoopConfig,
) -> Result<()> {
    if estimate_chars(messages) <= config.compact_threshold_chars {
        return Ok(());
    }
    let keep_head = usize::from(
        messages
            .first()
            .map(|m| m.role == Role::System)
            .unwrap_or(false),
    );
    if messages.len() <= keep_head + config.recent_keep_messages + 1 {
        return Ok(());
    }
    let mut split = messages.len() - config.recent_keep_messages;
    // Don't let the kept tail begin with an orphaned `tool` message (its
    // assistant tool-call would be summarized away — providers reject that). Back
    // up to the owning assistant so its tool_call and results stay together; this
    // also guarantees we never keep zero recent messages.
    while split > keep_head && messages[split].role == Role::Tool {
        split -= 1;
    }
    let middle_text = messages[keep_head..split]
        .iter()
        .map(render_message)
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "Summarize the following earlier conversation concisely, preserving user intent, decisions, file changes, errors, and pending tasks:\n\n{middle_text}"
    );
    let response = provider.complete(&[Message::user(prompt)], &[]).await?;
    let summary = response.message.content.unwrap_or_default();

    let mut rebuilt = Vec::with_capacity(keep_head + 1 + config.recent_keep_messages);
    rebuilt.extend_from_slice(&messages[..keep_head]);
    rebuilt.push(Message::system(format!(
        "[Summary of earlier conversation]\n{summary}"
    )));
    rebuilt.extend_from_slice(&messages[split..]);
    *messages = rebuilt;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::AutoApprove;
    use crate::model::{MockProvider, ModelResponse, RiskLevel, Role, ToolCall, ToolSpec};
    use crate::tools::Tool;
    use serde_json::Value;

    struct EchoTool;

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".to_string(),
                description: "echoes the given text".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"]
                }),
                risk: RiskLevel::Read,
            }
        }

        async fn call(&self, args: Value) -> Result<Value> {
            Ok(json!({ "echoed": args.get("text").cloned().unwrap_or(Value::Null) }))
        }
    }

    fn tool_call_response(name: &str) -> ModelResponse {
        ModelResponse {
            message: Message {
                role: Role::Assistant,
                content: None,
                tool_calls: vec![ToolCall {
                    id: "c1".to_string(),
                    name: name.to_string(),
                    arguments: json!({ "text": "hi" }),
                }],
                tool_call_id: None,
                attachments: Vec::new(),
            },
        }
    }

    fn final_response() -> ModelResponse {
        ModelResponse {
            message: Message::assistant("done"),
        }
    }

    #[tokio::test]
    async fn loop_calls_tool_then_finishes() {
        let provider = MockProvider::new(vec![tool_call_response("echo"), final_response()]);
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));

        let mut messages = vec![Message::user("please echo hi")];
        let mut events = EventLog::new();

        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");

        assert_eq!(outcome.output, "done");
        assert_eq!(outcome.steps, 2);
        // Assistant(tool_call) + ToolCall + ToolResult + Assistant(final) = 4
        assert_eq!(events.len(), 4);
    }

    #[tokio::test]
    async fn unknown_tool_is_fed_back_not_fatal() {
        let provider = MockProvider::new(vec![tool_call_response("missing"), final_response()]);
        let tools = ToolRegistry::new(); // no tools registered

        let mut messages = vec![Message::user("x")];
        let mut events = EventLog::new();

        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");

        assert_eq!(outcome.output, "done");
        // The error was fed back as a tool result message, not raised.
        let fed_back = messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("c1"));
        assert!(fed_back);
    }

    #[tokio::test]
    async fn max_steps_is_actionable_error() {
        // Always returns a tool call -> never finishes.
        let provider =
            MockProvider::new(vec![tool_call_response("echo"), tool_call_response("echo")]);
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));

        let mut messages = vec![Message::user("loop forever")];
        let mut events = EventLog::new();

        let err = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig {
                max_steps: 1,
                ..LoopConfig::default()
            },
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect_err("should hit max steps");
        assert!(err.report().remediation.is_some());
    }

    struct BigTool;

    #[async_trait::async_trait]
    impl Tool for BigTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "big".to_string(),
                description: "returns a large payload".to_string(),
                parameters: json!({ "type": "object", "properties": {} }),
                risk: RiskLevel::Read,
            }
        }

        async fn call(&self, _args: Value) -> Result<Value> {
            Ok(json!({ "data": "y".repeat(50_000) }))
        }
    }

    #[tokio::test]
    async fn large_tool_result_is_budgeted_but_logged_full() {
        let provider = MockProvider::new(vec![tool_call_response("big"), final_response()]);
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(BigTool));

        let mut messages = vec![Message::user("x")];
        let mut events = EventLog::new();
        let config = LoopConfig {
            max_steps: 8,
            max_tool_result_chars: 1000,
            ..LoopConfig::default()
        };

        run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &config,
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");

        // What the model sees is truncated...
        let tool_msg = messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("c1"))
            .expect("tool result message");
        let fed = tool_msg.content.as_ref().expect("content");
        assert!(fed.contains("truncated"));
        assert!(fed.chars().count() < 2000);

        // ...but the event log retains the full result.
        let full_logged = events.events().iter().any(
            |e| matches!(e, Event::ToolResult { result, .. } if result.to_string().len() > 40_000),
        );
        assert!(full_logged);
    }

    #[tokio::test]
    async fn compaction_summarizes_old_messages() {
        // Provider returns a summary (for compaction), then a final answer.
        let provider = MockProvider::new(vec![
            ModelResponse {
                message: Message::assistant("SUMMARY"),
            },
            final_response(),
        ]);
        let tools = ToolRegistry::new();

        let mut messages = vec![Message::system("sys")];
        for i in 0..20 {
            messages.push(Message::user(format!(
                "message number {i} with padding text"
            )));
            messages.push(Message::assistant(format!("reply {i} with padding text")));
        }
        let mut events = EventLog::new();
        let config = LoopConfig {
            max_steps: 4,
            max_tool_result_chars: 8000,
            compact_threshold_chars: 50,
            recent_keep_messages: 4,
            voice: false,
        };

        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &config,
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");

        assert_eq!(outcome.output, "done");
        assert!(messages.len() < 20);
        assert!(messages.iter().any(|m| {
            m.content
                .as_deref()
                .map(|c| c.contains("Summary of earlier conversation"))
                .unwrap_or(false)
        }));
    }

    #[tokio::test]
    async fn compaction_does_not_orphan_tool_messages() {
        // split = len(7) - recent_keep(2) = 5 -> messages[5] is a tool result
        // whose assistant tool-call would land in the summary. The guard must
        // advance the split so no orphaned tool message survives.
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant("SUMMARY"),
        }]);
        let assistant_tc = |id: &str| Message {
            role: Role::Assistant,
            content: Some("calling".to_string()),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: "echo".to_string(),
                arguments: json!({}),
            }],
            tool_call_id: None,
            attachments: Vec::new(),
        };
        let mut messages = vec![
            Message::system("sys padding to exceed the tiny threshold"),
            Message::user("hello there padding padding"),
            assistant_tc("c1"),
            Message::tool_result("c1", "result one padding"),
            assistant_tc("c2"),
            Message::tool_result("c2", "result two padding"),
            Message::assistant("final answer padding"),
        ];
        let config = LoopConfig {
            compact_threshold_chars: 10,
            recent_keep_messages: 2,
            ..LoopConfig::default()
        };

        compact_if_needed(&provider, &mut messages, &config)
            .await
            .expect("compact");

        // Every surviving `tool` message must still be preceded by its
        // assistant tool-call (no orphans).
        for (i, m) in messages.iter().enumerate() {
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().expect("tool has id");
                let has_owner = messages[..i]
                    .iter()
                    .any(|p| p.role == Role::Assistant && p.tool_calls.iter().any(|c| c.id == id));
                assert!(has_owner, "orphaned tool message survived compaction");
            }
        }
        assert_eq!(
            messages.last().and_then(|m| m.content.as_deref()),
            Some("final answer padding")
        );
    }

    #[tokio::test]
    async fn compaction_keeps_recent_when_tail_is_all_tools() {
        // recent_keep=2 and the tail is [tool, tool]; backing up to the owning
        // assistant must keep that block verbatim (not summarize everything).
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant("SUMMARY"),
        }]);
        let mut messages = vec![
            Message::system("sys padding to exceed the tiny threshold"),
            Message::user("hello there padding padding"),
            Message {
                role: Role::Assistant,
                content: Some("calling".to_string()),
                tool_calls: vec![
                    ToolCall {
                        id: "c1".into(),
                        name: "echo".into(),
                        arguments: json!({}),
                    },
                    ToolCall {
                        id: "c2".into(),
                        name: "echo".into(),
                        arguments: json!({}),
                    },
                ],
                tool_call_id: None,
                attachments: Vec::new(),
            },
            Message::tool_result("c1", "result one"),
            Message::tool_result("c2", "result two"),
        ];
        let config = LoopConfig {
            compact_threshold_chars: 10,
            recent_keep_messages: 2,
            ..LoopConfig::default()
        };

        compact_if_needed(&provider, &mut messages, &config)
            .await
            .expect("compact");

        // The owning assistant block survived verbatim (recent not all summarized).
        assert!(messages
            .iter()
            .any(|m| m.content.as_deref() == Some("calling")));
        // No orphaned tool messages.
        for (i, m) in messages.iter().enumerate() {
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().expect("tool id");
                assert!(messages[..i]
                    .iter()
                    .any(|p| p.role == Role::Assistant && p.tool_calls.iter().any(|c| c.id == id)));
            }
        }
    }

    struct StreamingMock;

    #[async_trait::async_trait]
    impl crate::model::ModelProvider for StreamingMock {
        async fn complete(
            &self,
            _m: &[Message],
            _t: &[crate::model::ToolSpec],
        ) -> Result<crate::model::ModelResponse> {
            Ok(crate::model::ModelResponse {
                message: Message::assistant("hello world"),
            })
        }
        async fn complete_streaming(
            &self,
            _m: &[Message],
            _t: &[crate::model::ToolSpec],
            on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
        ) -> Result<crate::model::ModelResponse> {
            on_delta("hello ");
            on_delta("world");
            Ok(crate::model::ModelResponse {
                message: Message::assistant("hello world"),
            })
        }
    }

    #[tokio::test]
    async fn run_turn_streaming_forwards_deltas() {
        use std::sync::{Arc, Mutex};
        let provider = StreamingMock;
        let tools = ToolRegistry::new();
        let mut messages = vec![Message::user("hi")];
        let mut events = EventLog::new();
        let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured = Arc::clone(&chunks);
        let mut sink: Box<dyn FnMut(&str) + Send> =
            Box::new(move |c: &str| captured.lock().expect("lock").push(c.to_string()));
        let outcome = run_turn_streaming(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::FullAccess,
            &mut AutoApprove,
            sink.as_mut(),
        )
        .await
        .expect("turn");
        assert_eq!(outcome.output, "hello world");
        assert_eq!(
            *chunks.lock().expect("lock"),
            vec!["hello ".to_string(), "world".to_string()]
        );
    }

    struct DenyGate;

    #[async_trait::async_trait]
    impl ApprovalGate for DenyGate {
        async fn request(
            &mut self,
            _tool: &str,
            _args: &Value,
            _risk: RiskLevel,
        ) -> Result<ApprovalDecision> {
            Ok(ApprovalDecision::Deny)
        }
    }

    struct MutateTool;

    #[async_trait::async_trait]
    impl Tool for MutateTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "danger".to_string(),
                description: "mutates something".to_string(),
                parameters: json!({ "type": "object", "properties": {} }),
                risk: RiskLevel::Mutate,
            }
        }

        async fn call(&self, _args: Value) -> Result<Value> {
            Ok(json!({ "executed": true }))
        }
    }

    #[tokio::test]
    async fn require_approval_denies_mutate_tool() {
        let provider = MockProvider::new(vec![tool_call_response("danger"), final_response()]);
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(MutateTool));
        let mut messages = vec![Message::user("do danger")];
        let mut events = EventLog::new();
        let mut gate = DenyGate;

        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::RequireApproval,
            &mut gate,
        )
        .await
        .expect("turn ok");

        assert_eq!(outcome.output, "done");
        // A denial was fed back, and the tool did NOT execute.
        assert!(messages.iter().any(|m| m
            .content
            .as_deref()
            .map(|c| c.contains("denied"))
            .unwrap_or(false)));
        assert!(!messages.iter().any(|m| m
            .content
            .as_deref()
            .map(|c| c.contains("\"executed\""))
            .unwrap_or(false)));
    }

    #[tokio::test]
    async fn voice_off_produces_no_speech_and_no_injection() {
        // voice=false: speech is None and the dual-channel contract is never
        // injected into the system prompt (zero extra tokens).
        let provider = MockProvider::new(vec![final_response()]);
        let tools = ToolRegistry::new();
        let mut messages = vec![Message::system("base"), Message::user("hi")];
        let mut events = EventLog::new();
        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");
        assert_eq!(outcome.output, "done");
        assert_eq!(outcome.speech, None);
        assert!(!messages.iter().any(|m| m
            .content
            .as_deref()
            .map(|c| c.contains(SPEECH_SENTINEL))
            .unwrap_or(false)));
    }

    #[tokio::test]
    async fn voice_on_splits_display_and_speech() {
        // Spec Example row 1: text before the sentinel is display, after is speech.
        let raw = format!("Here is the diff.\n{SPEECH_SENTINEL}\nDone — check the diff.");
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant(raw),
        }]);
        let tools = ToolRegistry::new();
        let mut messages = vec![Message::system("base"), Message::user("show diff")];
        let mut events = EventLog::new();
        let cfg = LoopConfig {
            voice: true,
            ..LoopConfig::default()
        };
        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &cfg,
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");
        assert_eq!(outcome.output, "Here is the diff.");
        assert_eq!(outcome.speech.as_deref(), Some("Done — check the diff."));
        // The voice contract was merged into the leading system message.
        assert!(messages[0]
            .content
            .as_deref()
            .map(|c| c.contains(SPEECH_SENTINEL))
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn voice_on_without_sentinel_yields_no_speech() {
        // Spec Example row 2: no sentinel → whole text is display, speech None.
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant("All set, no sentinel here."),
        }]);
        let tools = ToolRegistry::new();
        let mut messages = vec![Message::system("base"), Message::user("status")];
        let mut events = EventLog::new();
        let cfg = LoopConfig {
            voice: true,
            ..LoopConfig::default()
        };
        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &cfg,
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");
        assert_eq!(outcome.output, "All set, no sentinel here.");
        assert_eq!(outcome.speech, None);
    }

    #[test]
    fn split_channels_parses_speech_and_attention() {
        // Spec Example row 1: device + look + url.
        let raw = format!(
            "Here is the diff.\n{SPEECH_SENTINEL}\nDone — check it.\n{ATTENTION_SENTINEL}\ndevice=lab-pi-a; look=the dashboard; url=http://pi-a/grafana"
        );
        let (display, speech, attention) = split_channels(&raw);
        assert_eq!(display, "Here is the diff.");
        assert_eq!(speech.as_deref(), Some("Done — check it."));
        let a = attention.expect("attention");
        assert_eq!(a.device, "lab-pi-a");
        assert_eq!(a.look_at, "the dashboard");
        assert_eq!(a.url.as_deref(), Some("http://pi-a/grafana"));
    }

    #[test]
    fn split_channels_attention_without_url() {
        // Spec Example row 2: device + look, no url.
        let raw = format!(
            "ok\n{SPEECH_SENTINEL}\nok aloud\n{ATTENTION_SENTINEL}\ndevice=nas; look=the plex transcode log"
        );
        let (_d, _s, attention) = split_channels(&raw);
        let a = attention.expect("attention");
        assert_eq!(a.device, "nas");
        assert_eq!(a.look_at, "the plex transcode log");
        assert_eq!(a.url, None);
    }

    #[test]
    fn split_channels_no_attention_block() {
        // Spec Example row 3: no attention block → none.
        let raw = format!("hi\n{SPEECH_SENTINEL}\nhi aloud");
        let (_d, speech, attention) = split_channels(&raw);
        assert_eq!(speech.as_deref(), Some("hi aloud"));
        assert_eq!(attention, None);
    }

    #[tokio::test]
    async fn voice_on_parses_attention_from_final_reply() {
        let raw = format!(
            "All set.\n{SPEECH_SENTINEL}\nAll set, take a look.\n{ATTENTION_SENTINEL}\ndevice=nas; look=the plex log"
        );
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant(raw),
        }]);
        let tools = ToolRegistry::new();
        let mut messages = vec![Message::system("base"), Message::user("status")];
        let mut events = EventLog::new();
        let cfg = LoopConfig {
            voice: true,
            ..LoopConfig::default()
        };
        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &cfg,
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");
        assert_eq!(outcome.output, "All set.");
        assert_eq!(outcome.speech.as_deref(), Some("All set, take a look."));
        let a = outcome.attention.expect("attention");
        assert_eq!(a.device, "nas");
        assert_eq!(a.look_at, "the plex log");
    }

    #[tokio::test]
    async fn voice_off_yields_no_attention() {
        // voice off: no parsing — attention is None and the text is untouched.
        let raw = format!("hi\n{ATTENTION_SENTINEL}\ndevice=x; look=y");
        let provider = MockProvider::new(vec![ModelResponse {
            message: Message::assistant(raw.clone()),
        }]);
        let tools = ToolRegistry::new();
        let mut messages = vec![Message::user("x")];
        let mut events = EventLog::new();
        let outcome = run_turn(
            &provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::FullAccess,
            &mut AutoApprove,
        )
        .await
        .expect("turn ok");
        assert_eq!(outcome.attention, None);
        assert_eq!(outcome.output, raw);
    }
}
