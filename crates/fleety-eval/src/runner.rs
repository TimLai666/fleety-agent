//! Runner: drive one or more goldens against a real tool registry + a
//! [`MockProvider`](agent_core::MockProvider), capture events, assert.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use agent_core::{
    run_turn, AutoApprove, Event, LoopConfig, Message, MockProvider, ModelResponse, Policy,
    ToolCall, ToolRegistry,
};
use serde::Serialize;

use crate::golden::{Expected, Golden, ScriptedResponse};

/// The result of running one golden.
#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub name: String,
    pub passed: bool,
    /// All failure reasons (collected, not short-circuited, so the user sees
    /// every diff at once).
    pub failures: Vec<String>,
    /// Tools that actually ran, in order — handy for diagnosing diffs even on
    /// passing runs.
    pub tools_called: Vec<String>,
    /// The final assistant message content (post-loop). Empty if the loop
    /// stopped without producing a final answer.
    pub final_output: String,
    pub steps: usize,
}

impl Verdict {
    fn failed(name: String, reason: String) -> Self {
        Self {
            name,
            passed: false,
            failures: vec![reason],
            tools_called: Vec::new(),
            final_output: String::new(),
            steps: 0,
        }
    }
}

/// Run a single golden. Returns a [`Verdict`]; does not panic on assertion
/// failures (so the runner can keep going).
pub async fn run_one(golden: &Golden) -> Verdict {
    let temp = match TempWorkspace::new(&golden.name, &golden.workspace_files) {
        Ok(t) => t,
        Err(e) => return Verdict::failed(golden.name.clone(), format!("workspace setup: {e}")),
    };

    let mut tools = ToolRegistry::new();
    fleety_tools::register_workspace(&mut tools, &temp.workspace, &temp.backups);

    let responses: Vec<ModelResponse> = golden
        .scripted_responses
        .iter()
        .map(scripted_to_response)
        .collect();
    let provider = MockProvider::new(responses);

    let mut messages = Vec::new();
    if let Some(prompt) = &golden.system_prompt {
        messages.push(Message::system(prompt.clone()));
    }
    messages.push(Message::user(golden.user_input.clone()));

    let mut events = agent_core::EventLog::new();
    let mut gate = AutoApprove;
    let outcome = match run_turn(
        &provider,
        &tools,
        &mut messages,
        &mut events,
        &LoopConfig::default(),
        Policy::FullAccess,
        &mut gate,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            return Verdict::failed(
                golden.name.clone(),
                format!("agent loop error: {}", e.report().message),
            );
        }
    };

    let tools_called = walk_tool_calls(events.events());
    let mut failures = Vec::new();
    check_tools_called(&tools_called, &golden.expected, &mut failures);
    check_must_not_call(&tools_called, &golden.expected, &mut failures);
    check_final_contains(&outcome.output, &golden.expected, &mut failures);
    check_workspace_files_after(&temp.workspace, &golden.expected, &mut failures);
    check_workspace_files_absent(&temp.workspace, &golden.expected, &mut failures);

    Verdict {
        name: golden.name.clone(),
        passed: failures.is_empty(),
        failures,
        tools_called,
        final_output: outcome.output,
        steps: outcome.steps,
    }
}

/// Read a JSONL file and run every golden in it. Returns one [`Verdict`] per
/// golden, in file order; a parse failure on one line aborts the file (so
/// you find out about the broken line immediately).
pub async fn run_file(path: &Path) -> Result<Vec<Verdict>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut verdicts = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let golden: Golden = serde_json::from_str(trimmed)
            .map_err(|e| format!("parse {} line {}: {e}", path.display(), idx + 1))?;
        verdicts.push(run_one(&golden).await);
    }
    Ok(verdicts)
}

fn scripted_to_response(s: &ScriptedResponse) -> ModelResponse {
    let mut msg = Message::assistant(s.content.clone().unwrap_or_default());
    if s.content.is_none() {
        // Preserve "no content" so the model log says (tool call only).
        msg.content = None;
    }
    msg.tool_calls = s
        .tool_calls
        .iter()
        .map(|c| ToolCall {
            id: c.id.clone(),
            name: c.name.clone(),
            arguments: c.arguments.clone(),
        })
        .collect();
    ModelResponse { message: msg }
}

fn walk_tool_calls(events: &[Event]) -> Vec<String> {
    let mut out = Vec::new();
    for ev in events {
        if let Event::ToolCall(call) = ev {
            out.push(call.name.clone());
        }
    }
    out
}

fn check_tools_called(actual: &[String], expected: &Expected, failures: &mut Vec<String>) {
    // Expected tools must appear in order, allowing interleaving with others.
    let mut cursor = 0;
    for want in &expected.tools_called {
        let found = actual[cursor..].iter().position(|t| t == want);
        match found {
            Some(idx) => cursor += idx + 1,
            None => {
                failures.push(format!(
                    "expected tool '{want}' was not called in order (actual: {:?})",
                    actual
                ));
                return;
            }
        }
    }
}

fn check_must_not_call(actual: &[String], expected: &Expected, failures: &mut Vec<String>) {
    for forbidden in &expected.must_not_call {
        if actual.iter().any(|t| t == forbidden) {
            failures.push(format!(
                "forbidden tool '{forbidden}' was called (actual: {:?})",
                actual
            ));
        }
    }
}

fn check_final_contains(actual: &str, expected: &Expected, failures: &mut Vec<String>) {
    for needle in &expected.final_contains {
        if !actual.contains(needle) {
            failures.push(format!(
                "final output missing substring {:?} (got {:?})",
                needle, actual
            ));
        }
    }
}

fn check_workspace_files_after(workspace: &Path, expected: &Expected, failures: &mut Vec<String>) {
    for (rel, want) in &expected.workspace_files_after {
        let path = workspace.join(rel);
        match fs::read_to_string(&path) {
            Ok(got) if got == *want => {}
            Ok(got) => failures.push(format!(
                "workspace file {rel} content mismatch:\n  want: {:?}\n  got:  {:?}",
                want, got
            )),
            Err(e) => failures.push(format!("workspace file {rel} not readable: {e}")),
        }
    }
}

fn check_workspace_files_absent(workspace: &Path, expected: &Expected, failures: &mut Vec<String>) {
    for rel in &expected.workspace_files_absent {
        if workspace.join(rel).exists() {
            failures.push(format!("workspace file {rel} should not exist but does"));
        }
    }
}

/// A throwaway workspace + backups dir for one golden run, cleaned up on drop.
struct TempWorkspace {
    root: PathBuf,
    workspace: PathBuf,
    backups: PathBuf,
}

impl TempWorkspace {
    fn new(name: &str, seed: &BTreeMap<String, String>) -> Result<Self, String> {
        let safe: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let root =
            std::env::temp_dir().join(format!("fleety-eval-{safe}-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let backups = root.join("backups");
        fs::create_dir_all(&workspace).map_err(|e| format!("mk workspace: {e}"))?;
        fs::create_dir_all(&backups).map_err(|e| format!("mk backups: {e}"))?;
        for (rel, body) in seed {
            let path = workspace.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("mk {rel} parent: {e}"))?;
            }
            fs::write(&path, body).map_err(|e| format!("write seed {rel}: {e}"))?;
        }
        Ok(Self {
            root,
            workspace,
            backups,
        })
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden::ScriptedToolCall;

    #[tokio::test]
    async fn passing_golden_reads_file_and_summarises() {
        let golden = Golden {
            name: "read".into(),
            description: "agent reads then summarises".into(),
            workspace_files: [("a.txt".into(), "hello world".into())].into(),
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
                    content: Some("a.txt contains hello world".into()),
                    tool_calls: vec![],
                },
            ],
            expected: Expected {
                tools_called: vec!["read_file".into()],
                must_not_call: vec!["write_file".into()],
                final_contains: vec!["hello world".into()],
                workspace_files_after: Default::default(),
                workspace_files_absent: vec![],
            },
        };
        let v = run_one(&golden).await;
        assert!(v.passed, "verdict: {:?}", v);
        assert_eq!(v.tools_called, vec!["read_file".to_string()]);
    }

    #[tokio::test]
    async fn must_not_call_violation_is_caught() {
        // Script the model to call write_file even though we forbid it; the
        // verdict should fail with the forbidden-tool reason (not crash).
        let golden = Golden {
            name: "forbidden-write".into(),
            description: "".into(),
            workspace_files: Default::default(),
            user_input: "do nothing".into(),
            system_prompt: None,
            scripted_responses: vec![
                ScriptedResponse {
                    content: None,
                    tool_calls: vec![ScriptedToolCall {
                        id: "c1".into(),
                        name: "write_file".into(),
                        arguments: serde_json::json!({ "path": "x.txt", "content": "x" }),
                    }],
                },
                ScriptedResponse {
                    content: Some("done".into()),
                    tool_calls: vec![],
                },
            ],
            expected: Expected {
                tools_called: vec![],
                must_not_call: vec!["write_file".into()],
                final_contains: vec![],
                workspace_files_after: Default::default(),
                workspace_files_absent: vec![],
            },
        };
        let v = run_one(&golden).await;
        assert!(!v.passed);
        assert!(v.failures.iter().any(|f| f.contains("forbidden tool")));
    }

    #[tokio::test]
    async fn workspace_files_after_assertion_works() {
        // Script the model to write a file, then assert its content.
        let golden = Golden {
            name: "write-then-assert".into(),
            description: "".into(),
            workspace_files: Default::default(),
            user_input: "create greet.txt".into(),
            system_prompt: None,
            scripted_responses: vec![
                ScriptedResponse {
                    content: None,
                    tool_calls: vec![ScriptedToolCall {
                        id: "c1".into(),
                        name: "write_file".into(),
                        arguments: serde_json::json!({
                            "path": "greet.txt",
                            "content": "hi there",
                        }),
                    }],
                },
                ScriptedResponse {
                    content: Some("wrote greet.txt".into()),
                    tool_calls: vec![],
                },
            ],
            expected: Expected {
                tools_called: vec!["write_file".into()],
                must_not_call: vec![],
                final_contains: vec!["greet.txt".into()],
                workspace_files_after: [("greet.txt".into(), "hi there".into())].into(),
                workspace_files_absent: vec![],
            },
        };
        let v = run_one(&golden).await;
        assert!(v.passed, "verdict: {:?}", v);
    }
}
