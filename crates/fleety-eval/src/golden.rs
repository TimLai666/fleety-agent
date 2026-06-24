//! Golden record format. One JSON object per line of a `.jsonl` file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One scripted assistant response — what the model would have returned at one
/// step of the loop. `tool_calls` is empty for a terminal "final answer" step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptedResponse {
    /// Assistant text (may be `null` when the response is purely tool calls).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls the model is requesting this step. Each call's `id` must be
    /// unique within the golden (the runner uses it to thread `tool_result`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ScriptedToolCall>,
}

/// A scripted tool call inside a [`ScriptedResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Assertions the loop must satisfy after the scripted responses are consumed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Expected {
    /// Every tool name in this list must be called at least once, in order
    /// (allowing other calls between them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools_called: Vec<String>,
    /// No tool with a name in this list may be called.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub must_not_call: Vec<String>,
    /// The final assistant message's content must contain each substring.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub final_contains: Vec<String>,
    /// After the run, the listed paths (relative to the golden's workspace)
    /// must exist as files with the given exact content.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workspace_files_after: BTreeMap<String, String>,
    /// Paths that must NOT exist as files after the run (e.g. checked
    /// against an accidental write).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_files_absent: Vec<String>,
}

/// Optional attachments the user "sends" with the message. Mirrors the
/// `agent-core::Attachment` shape; for golden purposes a tiny base64 stub is
/// enough to verify the loop accepts the multimodal envelope without crashing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoldenAttachment {
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One full golden record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Golden {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Files to seed into a fresh workspace before the run. Paths are relative.
    #[serde(default)]
    pub workspace_files: BTreeMap<String, String>,
    /// The user message that drives the loop.
    pub user_input: String,
    /// Optional attachments riding along with the user message — for
    /// regression-testing the multimodal path (the MockProvider doesn't *see*
    /// them, but the loop must accept and persist them without crashing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_attachments: Vec<GoldenAttachment>,
    /// Optional system prompt prepended to the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// The model's scripted responses, one per loop step.
    pub scripted_responses: Vec<ScriptedResponse>,
    pub expected: Expected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_serde_roundtrips() {
        let g = Golden {
            name: "demo".into(),
            description: "demo golden".into(),
            workspace_files: [("a.txt".into(), "hi".into())].into(),
            user_input: "what does a.txt say?".into(),
            system_prompt: None,
            scripted_responses: vec![
                ScriptedResponse {
                    content: None,
                    tool_calls: vec![ScriptedToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({ "path": "a.txt" }),
                    }],
                },
                ScriptedResponse {
                    content: Some("a.txt says hi".into()),
                    tool_calls: vec![],
                },
            ],
            expected: Expected {
                tools_called: vec!["read_file".into()],
                must_not_call: vec!["write_file".into()],
                final_contains: vec!["hi".into()],
                workspace_files_after: BTreeMap::new(),
                workspace_files_absent: vec![],
            },
        };
        let text = serde_json::to_string(&g).expect("ser");
        let back: Golden = serde_json::from_str(&text).expect("de");
        assert_eq!(back.name, "demo");
        assert_eq!(back.scripted_responses.len(), 2);
        assert_eq!(back.expected.tools_called, vec!["read_file".to_string()]);
    }
}
