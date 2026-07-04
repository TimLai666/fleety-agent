//! Reuse an originating device's Claude Code `PreToolUse` / `PostToolUse` hooks.
//! Pure parsing + best-effort discovery, plus a tool wrapper that runs the
//! matching hooks around a tool call. The wrapper takes an injected
//! [`HookRunner`] (executes a command — locally or cross-device) and a
//! [`HookAudit`] sink, so the same logic serves production and tests. conn wires
//! the production runner (local shell / cross-device via the bridge) and the
//! `Storage`-backed audit sink, and applies the env policy.
//!
//! Everything parsing/discovery-side is best-effort: missing or malformed input
//! yields empty or partial results, never an error. Hook *execution* is
//! best-effort too — a hook that cannot run is audited and skipped; only a
//! completed `PreToolUse` run that exits non-zero denies the tool.

use std::path::Path;
use std::sync::Arc;

use agent_core::{Result, Tool};
use async_trait::async_trait;
use serde_json::{json, Value};

/// Which point in a tool call a hook fires at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
}

impl HookEvent {
    fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
        }
    }
}

/// Where a hook came from — sets audit tagging and the env kill-switch scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookScope {
    User,
    Project,
}

impl HookScope {
    fn as_str(self) -> &'static str {
        match self {
            HookScope::User => "user",
            HookScope::Project => "project",
        }
    }
}

/// One resolved hook: an event, a tool-name matcher, a shell command, and the
/// scope it was declared in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEntry {
    pub event: HookEvent,
    pub matcher: String,
    pub command: String,
    pub scope: HookScope,
}

/// Match a hook `matcher` against a tool name. First release: `*` or empty
/// matches every tool; otherwise an exact tool-name match. Advanced matcher
/// syntax (regex, input predicates, alternation) is out of scope.
pub fn matches(matcher: &str, tool_name: &str) -> bool {
    matcher.is_empty() || matcher == "*" || matcher == tool_name
}

/// Parse the `PreToolUse` / `PostToolUse` hooks from a parsed settings JSON,
/// tagging each with `scope`. The shape is
/// `hooks.<Event>[] = { matcher, hooks: [{ type: "command", command }] }`; a
/// missing `matcher` means match-all (`*`). Best-effort: anything not matching
/// the expected shape is skipped, never an error.
pub fn parse_hooks(settings: &Value, scope: HookScope) -> Vec<HookEntry> {
    let Some(hooks) = settings.get("hooks") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for event in [HookEvent::PreToolUse, HookEvent::PostToolUse] {
        let Some(arr) = hooks.get(event.as_str()).and_then(Value::as_array) else {
            continue;
        };
        for group in arr {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .unwrap_or("*")
                .to_string();
            let Some(cmds) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for cmd in cmds {
                // Only `type: "command"` entries carry a shell command.
                if cmd.get("type").and_then(Value::as_str) != Some("command") {
                    continue;
                }
                let command = match cmd.get("command").and_then(Value::as_str) {
                    Some(c) if !c.is_empty() => c.to_string(),
                    _ => continue,
                };
                out.push(HookEntry {
                    event,
                    matcher: matcher.clone(),
                    command,
                    scope,
                });
            }
        }
    }
    out
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Read the project and user `.claude/settings.json` and collect their hooks,
/// each tagged with its scope. Best-effort: a missing or malformed file
/// contributes nothing.
pub fn collect_hooks(project_cwd: &Path, user_home: &Path) -> Vec<HookEntry> {
    let mut out = Vec::new();
    for (scope, path) in [
        (
            HookScope::Project,
            project_cwd.join(".claude").join("settings.json"),
        ),
        (
            HookScope::User,
            user_home.join(".claude").join("settings.json"),
        ),
    ] {
        if let Some(settings) = read_json(&path) {
            out.extend(parse_hooks(&settings, scope));
        }
    }
    out
}

/// Apply the opt-out env policy: when `FLEETY_DISABLE_PROJECT_HOOKS=1`, drop
/// project-scope hooks (user-scope hooks continue to run). This is the
/// supply-chain kill-switch for hooks that may originate from an untrusted repo.
pub fn apply_env_policy(hooks: Vec<HookEntry>) -> Vec<HookEntry> {
    if std::env::var("FLEETY_DISABLE_PROJECT_HOOKS").ok().as_deref() == Some("1") {
        hooks
            .into_iter()
            .filter(|h| h.scope != HookScope::Project)
            .collect()
    } else {
        hooks
    }
}

/// The result of running one hook command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// The command completed with this exit code.
    Exited(i32),
    /// The command could not be run (spawn error, timeout, disconnect). Carries
    /// a reason for the audit note. Never denies a tool (best-effort).
    Failed(String),
}

/// Runs a hook command. Production impls run locally (same-host origin) or route
/// to the origin device (cross-device); tests inject a fake.
#[async_trait]
pub trait HookRunner: Send + Sync {
    async fn run(&self, entry: &HookEntry) -> HookOutcome;
}

/// Records a hook execution to the audit trail. Production impl appends to the
/// device history; tests inject a fake to assert what was recorded.
pub trait HookAudit: Send + Sync {
    fn record(&self, entry: &HookEntry, outcome: &HookOutcome);
}

/// Build the audit payload for a hook execution. Shared by the production sink
/// so the recorded shape is stable: it names the event, scope, command, and
/// outcome, and flags project-sourced executions for supply-chain review.
pub fn audit_payload(entry: &HookEntry, outcome: &HookOutcome) -> Value {
    let (outcome_str, code, note) = match outcome {
        HookOutcome::Exited(c) => ("exited", Some(*c), None),
        HookOutcome::Failed(reason) => ("failed", None, Some(reason.clone())),
    };
    json!({
        "hook": true,
        "event": entry.event.as_str(),
        "scope": entry.scope.as_str(),
        "project_sourced": entry.scope == HookScope::Project,
        "command": entry.command,
        "outcome": outcome_str,
        "code": code,
        "note": note,
    })
}

/// A tool wrapped so its matching `PreToolUse` hooks run before it and its
/// matching `PostToolUse` hooks run after it. A `PreToolUse` hook that exits
/// non-zero denies the call (the inner tool never runs) with a result shaped
/// like the approval-gate denial. `PostToolUse` failures and hook-execution
/// failures are audited but never block.
pub struct HookedTool {
    inner: Box<dyn Tool>,
    tool_name: String,
    pre: Vec<HookEntry>,
    post: Vec<HookEntry>,
    runner: Arc<dyn HookRunner>,
    audit: Arc<dyn HookAudit>,
}

#[async_trait]
impl Tool for HookedTool {
    fn spec(&self) -> agent_core::model::ToolSpec {
        self.inner.spec()
    }

    async fn call(&self, args: Value) -> Result<Value> {
        for h in &self.pre {
            let outcome = self.runner.run(h).await;
            self.audit.record(h, &outcome);
            if let HookOutcome::Exited(code) = outcome {
                if code != 0 {
                    // Shaped like agent-core's approval denial so it flows the
                    // same audit/summary path and the agent treats it as denied.
                    return Ok(json!({
                        "denied": true,
                        "tool": self.tool_name,
                        "reason": format!(
                            "denied by a PreToolUse hook (exit {code}); do not retry it"
                        ),
                    }));
                }
            }
        }
        let result = self.inner.call(args).await;
        for h in &self.post {
            let outcome = self.runner.run(h).await;
            self.audit.record(h, &outcome);
            // PostToolUse never denies: the tool has already run.
        }
        result
    }
}

/// Wrap each tool with its matching hooks. Tools with no matching hook are left
/// unwrapped (zero overhead). `hooks` should already have the env policy
/// applied.
pub fn wrap_tools(
    tools: Vec<Box<dyn Tool>>,
    hooks: &[HookEntry],
    runner: Arc<dyn HookRunner>,
    audit: Arc<dyn HookAudit>,
) -> Vec<Box<dyn Tool>> {
    tools
        .into_iter()
        .map(|inner| {
            let name = inner.spec().name;
            let pre: Vec<HookEntry> = hooks
                .iter()
                .filter(|h| h.event == HookEvent::PreToolUse && matches(&h.matcher, &name))
                .cloned()
                .collect();
            let post: Vec<HookEntry> = hooks
                .iter()
                .filter(|h| h.event == HookEvent::PostToolUse && matches(&h.matcher, &name))
                .cloned()
                .collect();
            if pre.is_empty() && post.is_empty() {
                inner
            } else {
                Box::new(HookedTool {
                    inner,
                    tool_name: name,
                    pre,
                    post,
                    runner: Arc::clone(&runner),
                    audit: Arc::clone(&audit),
                }) as Box<dyn Tool>
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::model::{RiskLevel, ToolSpec};
    use serde_json::json;
    use std::sync::Mutex;

    #[test]
    fn parse_pretooluse_and_posttooluse() {
        let settings = json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [ { "type": "command", "command": "lint.sh" } ] }
                ],
                "PostToolUse": [
                    { "hooks": [ { "type": "command", "command": "fmt.sh" } ] }
                ]
            }
        });
        let got = parse_hooks(&settings, HookScope::User);
        assert_eq!(got.len(), 2);
        let pre = got.iter().find(|h| h.event == HookEvent::PreToolUse).unwrap();
        assert_eq!(pre.matcher, "Bash");
        assert_eq!(pre.command, "lint.sh");
        assert_eq!(pre.scope, HookScope::User);
        let post = got
            .iter()
            .find(|h| h.event == HookEvent::PostToolUse)
            .unwrap();
        assert_eq!(post.matcher, "*", "absent matcher defaults to match-all");
        assert_eq!(post.command, "fmt.sh");
    }

    #[test]
    fn parse_is_best_effort_on_bad_json() {
        // No hooks section, wrong types, non-command entries → empty, no panic.
        assert!(parse_hooks(&json!({}), HookScope::User).is_empty());
        assert!(parse_hooks(&json!({ "hooks": 3 }), HookScope::User).is_empty());
        let weird = json!({
            "hooks": {
                "PreToolUse": [ { "matcher": "X", "hooks": [ { "type": "other", "command": "z" } ] } ]
            }
        });
        assert!(
            parse_hooks(&weird, HookScope::User).is_empty(),
            "non-command hook entries are skipped"
        );
    }

    #[test]
    fn matcher_wildcard_and_exact() {
        assert!(matches("*", "Bash"));
        assert!(matches("", "Bash"));
        assert!(matches("Bash", "Bash"));
        assert!(!matches("Bash", "Read"));
    }

    #[test]
    fn collect_hooks_tags_scope() {
        let home = std::env::temp_dir().join(format!("fleety-hooks-{}", uuid::Uuid::new_v4()));
        let proj = home.join("proj");
        std::fs::create_dir_all(proj.join(".claude")).expect("mk proj/.claude");
        std::fs::create_dir_all(home.join(".claude")).expect("mk home/.claude");
        std::fs::write(
            home.join(".claude").join("settings.json"),
            r#"{"hooks":{"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":"u.sh"}]}]}}"#,
        )
        .expect("w user settings");
        std::fs::write(
            proj.join(".claude").join("settings.json"),
            r#"{"hooks":{"PostToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"p.sh"}]}]}}"#,
        )
        .expect("w proj settings");
        let got = collect_hooks(&proj, &home);
        assert_eq!(got.len(), 2);
        let user = got.iter().find(|h| h.scope == HookScope::User).unwrap();
        assert_eq!(user.command, "u.sh");
        let project = got.iter().find(|h| h.scope == HookScope::Project).unwrap();
        assert_eq!(project.command, "p.sh");
        assert_eq!(project.event, HookEvent::PostToolUse);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn collect_hooks_is_best_effort() {
        assert!(collect_hooks(Path::new("/no/such/proj"), Path::new("/no/such/home")).is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn project_hooks_disabled_by_env() {
        let hooks = vec![
            HookEntry {
                event: HookEvent::PreToolUse,
                matcher: "*".into(),
                command: "u.sh".into(),
                scope: HookScope::User,
            },
            HookEntry {
                event: HookEvent::PreToolUse,
                matcher: "*".into(),
                command: "p.sh".into(),
                scope: HookScope::Project,
            },
        ];
        std::env::set_var("FLEETY_DISABLE_PROJECT_HOOKS", "1");
        let filtered = apply_env_policy(hooks.clone());
        std::env::remove_var("FLEETY_DISABLE_PROJECT_HOOKS");
        assert_eq!(filtered.len(), 1, "project hook dropped, user hook kept");
        assert_eq!(filtered[0].scope, HookScope::User);
        // Without the env, both survive.
        assert_eq!(apply_env_policy(hooks).len(), 2);
    }

    // --- wrapper tests: fake tool + fake runner + fake audit ------------------

    struct FakeTool {
        name: &'static str,
        ran: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.to_string(),
                description: "fake".into(),
                parameters: json!({ "type": "object" }),
                risk: RiskLevel::Read,
            }
        }
        async fn call(&self, _args: Value) -> Result<Value> {
            *self.ran.lock().unwrap() = true;
            Ok(json!({ "ok": true }))
        }
    }

    /// A runner that returns a fixed outcome per hook command, keyed by command.
    struct FakeRunner(std::collections::HashMap<String, HookOutcome>);

    #[async_trait]
    impl HookRunner for FakeRunner {
        async fn run(&self, entry: &HookEntry) -> HookOutcome {
            self.0
                .get(&entry.command)
                .cloned()
                .unwrap_or(HookOutcome::Exited(0))
        }
    }

    #[derive(Default)]
    struct FakeAudit(Mutex<Vec<Value>>);

    impl HookAudit for FakeAudit {
        fn record(&self, entry: &HookEntry, outcome: &HookOutcome) {
            self.0.lock().unwrap().push(audit_payload(entry, outcome));
        }
    }

    fn wrap_one(
        tool: Box<dyn Tool>,
        hooks: Vec<HookEntry>,
        runner: Arc<dyn HookRunner>,
        audit: Arc<dyn HookAudit>,
    ) -> Box<dyn Tool> {
        wrap_tools(vec![tool], &hooks, runner, audit)
            .into_iter()
            .next()
            .unwrap()
    }

    #[tokio::test]
    async fn pretooluse_nonzero_exit_denies_tool() {
        let ran = Arc::new(Mutex::new(false));
        let tool = Box::new(FakeTool {
            name: "Bash",
            ran: Arc::clone(&ran),
        });
        let hooks = vec![HookEntry {
            event: HookEvent::PreToolUse,
            matcher: "Bash".into(),
            command: "deny.sh".into(),
            scope: HookScope::User,
        }];
        let runner = Arc::new(FakeRunner(
            [("deny.sh".to_string(), HookOutcome::Exited(2))].into(),
        ));
        let audit = Arc::new(FakeAudit::default());
        let wrapped = wrap_one(tool, hooks, runner, Arc::clone(&audit) as Arc<dyn HookAudit>);
        let out = wrapped.call(json!({})).await.unwrap();
        assert_eq!(out["denied"], json!(true));
        assert_eq!(out["tool"], json!("Bash"));
        assert!(!*ran.lock().unwrap(), "inner tool must not run when denied");
        assert_eq!(audit.0.lock().unwrap().len(), 1, "the deny hook is audited");
    }

    #[tokio::test]
    async fn posttooluse_failure_does_not_block() {
        let ran = Arc::new(Mutex::new(false));
        let tool = Box::new(FakeTool {
            name: "Read",
            ran: Arc::clone(&ran),
        });
        let hooks = vec![HookEntry {
            event: HookEvent::PostToolUse,
            matcher: "*".into(),
            command: "post.sh".into(),
            scope: HookScope::User,
        }];
        let runner = Arc::new(FakeRunner(
            [(
                "post.sh".to_string(),
                HookOutcome::Failed("boom".to_string()),
            )]
            .into(),
        ));
        let audit = Arc::new(FakeAudit::default());
        let wrapped = wrap_one(tool, hooks, runner, Arc::clone(&audit) as Arc<dyn HookAudit>);
        let out = wrapped.call(json!({})).await.unwrap();
        assert_eq!(out["ok"], json!(true), "tool result still returned");
        assert!(*ran.lock().unwrap(), "inner tool ran");
        let recorded = audit.0.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["outcome"], json!("failed"));
    }

    #[tokio::test]
    async fn hook_execution_is_audited_with_scope() {
        let tool = Box::new(FakeTool {
            name: "Bash",
            ran: Arc::new(Mutex::new(false)),
        });
        let hooks = vec![
            HookEntry {
                event: HookEvent::PreToolUse,
                matcher: "*".into(),
                command: "user.sh".into(),
                scope: HookScope::User,
            },
            HookEntry {
                event: HookEvent::PreToolUse,
                matcher: "*".into(),
                command: "proj.sh".into(),
                scope: HookScope::Project,
            },
        ];
        let runner = Arc::new(FakeRunner(Default::default())); // all Exited(0)
        let audit = Arc::new(FakeAudit::default());
        let wrapped = wrap_one(tool, hooks, runner, Arc::clone(&audit) as Arc<dyn HookAudit>);
        wrapped.call(json!({})).await.unwrap();
        let recorded = audit.0.lock().unwrap();
        assert_eq!(recorded.len(), 2, "both hooks audited");
        let proj = recorded
            .iter()
            .find(|r| r["scope"] == json!("project"))
            .unwrap();
        assert_eq!(proj["project_sourced"], json!(true));
        let user = recorded
            .iter()
            .find(|r| r["scope"] == json!("user"))
            .unwrap();
        assert_eq!(user["project_sourced"], json!(false));
    }
}
