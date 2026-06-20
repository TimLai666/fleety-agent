//! The tool-calling loop.
//!
//! Drives a [`ModelProvider`] and a [`ToolRegistry`]: ask the model, run any
//! tool calls it requests, feed the results back, and repeat until the model
//! returns a final answer (or `max_steps` is hit).

use serde_json::json;

use crate::event::{Event, EventLog};
use crate::model::{Message, ModelProvider, Role};
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
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 16,
            max_tool_result_chars: 8000,
            compact_threshold_chars: 24_000,
            recent_keep_messages: 8,
        }
    }
}

/// The result of one agent turn.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub output: String,
    pub steps: usize,
}

/// Run the tool-calling loop for one user turn.
///
/// Appends the assistant and tool messages to `messages` and records `events`.
/// A failing tool is **not fatal**: its actionable error report is fed back as a
/// tool result so the model can recover, rather than aborting the turn. Large
/// tool results are budgeted before being fed back (full copy kept in `events`).
pub async fn run_turn(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    messages: &mut Vec<Message>,
    events: &mut EventLog,
    config: &LoopConfig,
) -> Result<TurnOutcome> {
    let specs = tools.specs();

    for step in 1..=config.max_steps {
        compact_if_needed(provider, messages, config).await?;
        let response = provider.complete(messages, &specs).await?;
        let assistant = response.message;
        events.push(Event::Assistant(assistant.clone()));
        messages.push(assistant.clone());

        if assistant.tool_calls.is_empty() {
            return Ok(TurnOutcome {
                output: assistant.content.unwrap_or_default(),
                steps: step,
            });
        }

        for call in &assistant.tool_calls {
            events.push(Event::ToolCall(call.clone()));
            let result = match tools.call(&call.name, call.arguments.clone()).await {
                Ok(value) => value,
                Err(err) => json!({ "error": err.report() }),
            };
            // Full result to the event log (truth); budgeted copy to the model.
            events.push(Event::ToolResult {
                id: call.id.clone(),
                result: result.clone(),
            });
            let fed = budget_text(&result.to_string(), config.max_tool_result_chars);
            messages.push(Message::tool_result(call.id.clone(), fed));
        }
    }

    Err(CoreError::Provider(format!(
        "reached max steps ({}) without a final answer; raise max_steps or simplify the task",
        config.max_steps
    )))
}

/// Truncate `text` to at most `max_chars`, appending a marker noting how much was
/// omitted. The full text lives in the event log, so this is reversible.
fn budget_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    let omitted = text.chars().count() - max_chars;
    format!("{kept}\n... [truncated {omitted} chars; full result retained in the event log]")
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
    let split = messages.len() - config.recent_keep_messages;
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

        run_turn(&provider, &tools, &mut messages, &mut events, &config)
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

    #[test]
    fn budget_text_passes_short_and_truncates_long() {
        assert_eq!(budget_text("short", 10), "short");
        let long = "x".repeat(100);
        let out = budget_text(&long, 10);
        assert!(out.contains("truncated 90 chars"));
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
        };

        let outcome = run_turn(&provider, &tools, &mut messages, &mut events, &config)
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
}
