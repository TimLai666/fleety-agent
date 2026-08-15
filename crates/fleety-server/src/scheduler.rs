//! Schedule fire loop: periodically run due schedules unattended.
//!
//! Due `at:`/`every:` schedules run through the agent with an **unattended
//! policy** (`RequireApproval` + `MandateGate`): reads/reporting proceed, and a
//! mutate/critical tool runs only if its name is in the schedule's
//! `allowed_tools` mandate (others are denied and fed back). Each run is
//! persisted to a `schedule-<id>` conversation and audited, and `last_run` set.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{
    reconstruct_messages, run_turn, ApprovalGate, LoopConfig, MandateGate, Message, ModelProvider,
    Policy, Result, ToolRegistry,
};

use crate::auto_review::{timeout_from_env, AutoReviewGate};
use crate::schedules;
use crate::storage::Storage;

pub(crate) const SCHED_DEVICE: &str = "scheduler";

fn schedule_gate(allowed_tools: Vec<String>) -> (Policy, Box<dyn ApprovalGate + Send>) {
    if std::env::var("FLEETY_POLICY").as_deref() == Ok("auto_review") {
        let tiers = crate::providers::ProviderTiers::from_env();
        return (
            Policy::AutoReview,
            Box::new(AutoReviewGate::with_allowed_tools(
                tiers.resolve("cheap"),
                timeout_from_env(),
                allowed_tools,
            )),
        );
    }
    (
        Policy::RequireApproval,
        Box::new(MandateGate::new(allowed_tools)),
    )
}

/// Current unix time in seconds.
pub fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Run all schedules due at `now`. Returns how many fired.
pub async fn tick(
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    workspace: &Path,
    device_tools: crate::bridge::DeviceTools,
    now: u64,
) -> Result<usize> {
    let due = schedules::due_schedules(&storage.schedules_dir(), now)?;
    // Schedule turns interrupted by a crash, to finish before firing new ones.
    let incomplete: Vec<String> = storage
        .list_incomplete_turns()?
        .into_iter()
        .filter(|(device, _)| device == SCHED_DEVICE)
        .map(|(_, conversation)| conversation)
        .collect();
    if due.is_empty() && incomplete.is_empty() {
        return Ok(0);
    }
    // This tick does turn work (recover interrupted schedule turns and/or fire
    // due ones): count it as an in-flight turn so a deferred `restart` waits for
    // schedule-fired turns too. Dropped when the tick returns (any path).
    let _inflight = crate::restart_watch::turn_guard();
    let mut tools = crate::tools::build_registry(
        workspace,
        &storage.backups_dir(),
        &storage.memory_dir(),
        &storage.history_path(SCHED_DEVICE),
        &storage.devices_dir(),
        &storage.schedules_dir(),
        device_tools,
    );
    crate::skills::register(
        &mut tools,
        &storage.skills_builtin_dir(),
        &storage.skills_authored_dir(),
        &storage.skills_installed_dir(),
        &storage.skills_synced_dir(),
        &[],
    );
    crate::web::register(&mut tools, &storage.cookies_dir(), workspace);
    crate::mcp::register(
        &mut tools,
        &storage.mcp_builtin_config_path(),
        &storage.mcp_installed_config_path(),
        Vec::new(),
    );
    crate::wiki::register(
        &mut tools,
        &storage.wiki_dir(),
        &storage.models_dir(),
        &storage.backups_dir(),
    );
    crate::ssh::register(&mut tools);
    fleety_tools::register_browser(&mut tools);
    fleety_tools::register_computer(&mut tools);
    crate::sites::register(&mut tools, &storage.sites_dir(), &storage.devices_dir());
    crate::presence::register_presence(&mut tools, storage.home(), storage.sites_dir());
    // Finish interrupted scheduled turns first (best-effort, each isolated).
    for conversation in incomplete {
        if let Err(e) = recover_schedule_turn(storage, provider, &tools, &conversation).await {
            tracing::warn!(conversation = %conversation, report = ?e.report(), "could not recover scheduled turn");
        }
    }

    let mut fired = 0;
    for item in due {
        let id = item.id.clone();
        // Per-schedule isolation: a failing schedule is recorded (as an `error`
        // outcome) and marked fired inside `fire_one`, so it neither aborts the
        // tick nor silently retries. A persistence failure is logged and skipped.
        match fire_one(storage, provider, &tools, item, now).await {
            Ok(()) => fired += 1,
            Err(e) => {
                tracing::warn!(schedule = %id, report = ?e.report(), "could not fire schedule (isolated)")
            }
        }
    }
    Ok(fired)
}

/// Fire one due schedule end-to-end: run it unattended, persist the assistant
/// reply (or a failure notice) to its `schedule-<id>` conversation, record the
/// run outcome (`ok`/`error`), and mark it fired.
///
/// A `run_turn` error is isolated here — recorded as an `error` outcome and
/// still marked fired — so a single failure neither aborts the surrounding tick
/// nor causes the schedule to be retried on every subsequent tick (`at:` won't
/// re-fire, `every:`/cron move to their next period). Both the success and
/// failure paths count as a fire. `Err` is returned only for a persistence
/// failure, which the caller logs and skips.
async fn fire_one(
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    item: schedules::DueSchedule,
    now: u64,
) -> Result<()> {
    let schedules::DueSchedule {
        id,
        prompt,
        allowed_tools,
    } = item;
    let conversation = format!("schedule-{id}");
    tracing::info!(schedule = %id, "firing schedule");
    let user_msg = Message::user(prompt);
    storage.append(SCHED_DEVICE, &conversation, &user_msg)?;
    storage.journal_begin(SCHED_DEVICE, &conversation, &user_msg)?;
    let mut messages = vec![Message::system(
        storage.system_prompt_for(&storage.acting_for_device(SCHED_DEVICE))?,
    )];
    messages.extend(storage.load(SCHED_DEVICE, &conversation)?);
    // Journal each event so a crash mid-run is recoverable on the next tick.
    let mut events = storage.journaling_log(SCHED_DEVICE, &conversation);
    // Mandate enforcement: only the schedule's allowed_tools may mutate.
    let (policy, mut gate) = schedule_gate(allowed_tools);
    let run = run_turn(
        provider,
        tools,
        &mut messages,
        &mut events,
        &LoopConfig::default(),
        policy,
        gate.as_mut(),
    )
    .await;
    // Persist whatever was journalled to history regardless of run outcome.
    for event in events.events() {
        storage.append_history(SCHED_DEVICE, event)?;
    }
    let (status, reply, summary) = match run {
        Ok(outcome) => {
            let summary = schedules::truncate_summary(&outcome.output);
            ("ok", outcome.output, summary)
        }
        Err(e) => {
            let report = e.report();
            let summary = schedules::truncate_summary(&report.message);
            tracing::warn!(schedule = %id, report = ?report, "scheduled run failed (isolated)");
            // Leave a legible failure record in the schedule's own conversation.
            let reply = match &report.remediation {
                Some(r) => format!("⚠ Scheduled run FAILED: {}\n{r}", report.message),
                None => format!("⚠ Scheduled run FAILED: {}", report.message),
            };
            ("error", reply, summary)
        }
    };
    storage.append(SCHED_DEVICE, &conversation, &Message::assistant(reply))?;
    storage.journal_end(SCHED_DEVICE, &conversation)?;
    // Outcome + fire mark. Recorded for both success and failure so the run is
    // always discoverable and never silently retried.
    schedules::record_outcome(&storage.schedules_dir(), &id, status, &summary, now)?;
    schedules::mark_fired(&storage.schedules_dir(), &id, now)?;
    Ok(())
}

/// Finish a scheduled turn interrupted by a crash: reconstruct from the journal
/// (the in-flight tool is flagged interrupted, never re-run), re-apply the
/// schedule's mandate, run to completion, and clear the journal.
async fn recover_schedule_turn(
    storage: &Arc<Storage>,
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    conversation: &str,
) -> Result<()> {
    let events = storage.journal_events(SCHED_DEVICE, conversation)?;
    if events.is_empty() {
        storage.journal_end(SCHED_DEVICE, conversation)?;
        return Ok(());
    }
    tracing::info!(%conversation, events = events.len(), "recovering interrupted scheduled turn");
    let config = LoopConfig::default();
    let mut messages = vec![Message::system(
        storage.system_prompt_for(&storage.acting_for_device(SCHED_DEVICE))?,
    )];
    messages.extend(storage.load(SCHED_DEVICE, conversation)?);
    messages.extend(reconstruct_messages(&events, config.max_tool_result_chars));
    let mut log = storage.journaling_log(SCHED_DEVICE, conversation);
    let (policy, mut gate) = schedule_gate(schedule_allowed_tools(storage, conversation));
    let outcome = run_turn(
        provider,
        tools,
        &mut messages,
        &mut log,
        &config,
        policy,
        gate.as_mut(),
    )
    .await?;
    for event in log.events() {
        storage.append_history(SCHED_DEVICE, event)?;
    }
    storage.append(
        SCHED_DEVICE,
        conversation,
        &Message::assistant(outcome.output),
    )?;
    storage.journal_end(SCHED_DEVICE, conversation)?;
    Ok(())
}

/// Restore a schedule's mandate (`allowed_tools`) by id from a `schedule-<id>`
/// conversation. Empty (no mutates allowed) if the schedule is gone — the safe
/// default for recovering an unattended turn.
fn schedule_allowed_tools(storage: &Storage, conversation: &str) -> Vec<String> {
    let Some(id) = conversation.strip_prefix("schedule-") else {
        return Vec::new();
    };
    let path = storage.schedules_dir().join(format!("{id}.json"));
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("allowed_tools")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Spawn the periodic fire loop. A failing tick is logged and isolated; it never
/// brings the server down.
pub fn spawn(
    storage: Arc<Storage>,
    provider: Arc<dyn ModelProvider>,
    workspace: Arc<PathBuf>,
    device_tools: crate::bridge::DeviceTools,
    tick_secs: u64,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(tick_secs.max(1)));
        loop {
            interval.tick().await;
            let dt = Arc::clone(&device_tools);
            match tick(&storage, provider.as_ref(), &workspace, dt, now_secs()).await {
                Ok(n) if n > 0 => tracing::info!(fired = n, "scheduler fired due schedules"),
                Ok(_) => {}
                Err(e) => tracing::warn!(report = ?e.report(), "scheduler tick error (isolated)"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::echo::EchoProvider;
    use agent_core::{Event, Role, ToolCall};
    use serde_json::json;

    #[tokio::test]
    async fn tick_recovers_interrupted_scheduled_turn() {
        let home = std::env::temp_dir().join(format!("fleety-recover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = home.clone();

        // Simulate a crash mid-turn: a user message persisted, a journal opened,
        // an assistant step with a tool call recorded — but no tool result.
        let conv = "schedule-s1";
        let user = Message::user("status report");
        storage
            .append(SCHED_DEVICE, conv, &user)
            .expect("append user");
        storage
            .journal_begin(SCHED_DEVICE, conv, &user)
            .expect("begin");
        let mut assistant = Message::assistant("");
        assistant.tool_calls = vec![ToolCall {
            id: "t1".into(),
            name: "run_command".into(),
            arguments: json!({ "command": "echo hi" }),
        }];
        storage
            .journal_event(SCHED_DEVICE, conv, &Event::Assistant(assistant))
            .expect("journal assistant");
        storage
            .journal_event(
                SCHED_DEVICE,
                conv,
                &Event::ToolCall(ToolCall {
                    id: "t1".into(),
                    name: "run_command".into(),
                    arguments: json!({ "command": "echo hi" }),
                }),
            )
            .expect("journal toolcall");
        // crash here: t1 has no result.

        // A tick with nothing due still recovers the interrupted turn.
        let fired = tick(
            &storage,
            &EchoProvider,
            &workspace,
            crate::bridge::new_device_tools(),
            0,
        )
        .await
        .expect("tick");
        assert_eq!(fired, 0);

        // The journal is cleared and a final assistant reply was persisted —
        // without re-running the interrupted `run_command`.
        assert!(storage.list_incomplete_turns().expect("list").is_empty());
        let msgs = storage.load(SCHED_DEVICE, conv).expect("load");
        assert!(msgs.iter().any(|m| m.role == Role::Assistant));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn tick_fires_due_at_schedule_once() {
        let home = std::env::temp_dir().join(format!("fleety-tick-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = home.clone(); // any real dir serves as the tool root

        let sdir = storage.schedules_dir();
        std::fs::create_dir_all(&sdir).expect("mk sdir");
        std::fs::write(
            sdir.join("s1.json"),
            r#"{"id":"s1","trigger":"at:1","prompt":"status report","enabled":true}"#,
        )
        .expect("write schedule");

        let provider = EchoProvider;
        let dt = crate::bridge::new_device_tools();
        let fired = tick(&storage, &provider, &workspace, Arc::clone(&dt), 1000)
            .await
            .expect("tick");
        assert_eq!(fired, 1);

        let msgs = storage.load(SCHED_DEVICE, "schedule-s1").expect("load");
        assert!(msgs.len() >= 2); // user prompt + assistant reply

        // A successful run records an `ok` outcome on the schedule.
        let record = std::fs::read_to_string(sdir.join("s1.json")).expect("read schedule");
        let value: serde_json::Value = serde_json::from_str(&record).expect("parse");
        assert_eq!(
            value
                .get("last_outcome")
                .and_then(|o| o.get("status"))
                .and_then(|s| s.as_str()),
            Some("ok")
        );

        // `at:` already fired -> not due again
        let again = tick(&storage, &provider, &workspace, dt, 2000)
            .await
            .expect("tick2");
        assert_eq!(again, 0);

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A provider that fails the run when the prompt carries a `BOOM` marker and
    /// echoes otherwise — lets one tick fire both a failing and a succeeding
    /// schedule.
    struct SelectiveFailProvider;

    #[async_trait::async_trait]
    impl ModelProvider for SelectiveFailProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[agent_core::ToolSpec],
        ) -> Result<agent_core::ModelResponse> {
            let last_user = messages
                .iter()
                .rev()
                .find(|m| m.role == Role::User)
                .and_then(|m| m.content.clone())
                .unwrap_or_default();
            if last_user.contains("BOOM") {
                return Err(agent_core::CoreError::Provider(
                    "simulated provider failure".to_string(),
                ));
            }
            Ok(agent_core::ModelResponse::new(Message::assistant(format!(
                "echo: {last_user}"
            ))))
        }
    }

    #[tokio::test]
    async fn tick_isolates_failing_schedule() {
        let home = std::env::temp_dir().join(format!("fleety-isolate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let workspace = home.clone();

        let sdir = storage.schedules_dir();
        std::fs::create_dir_all(&sdir).expect("mk sdir");
        // Two schedules due at the same tick: one succeeds, one fails.
        std::fs::write(
            sdir.join("ok.json"),
            r#"{"id":"ok","trigger":"at:1","prompt":"status please","enabled":true}"#,
        )
        .expect("write ok schedule");
        std::fs::write(
            sdir.join("bad.json"),
            r#"{"id":"bad","trigger":"at:1","prompt":"BOOM now","enabled":true}"#,
        )
        .expect("write bad schedule");

        let dt = crate::bridge::new_device_tools();
        let fired = tick(
            &storage,
            &SelectiveFailProvider,
            &workspace,
            Arc::clone(&dt),
            1000,
        )
        .await
        .expect("tick");
        // Both fired despite one failing (failure did not abort the tick).
        assert_eq!(fired, 2);

        let read_status = |id: &str| -> String {
            let text = std::fs::read_to_string(sdir.join(format!("{id}.json"))).expect("read");
            let value: serde_json::Value = serde_json::from_str(&text).expect("parse");
            value
                .get("last_outcome")
                .and_then(|o| o.get("status"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(read_status("ok"), "ok");
        assert_eq!(read_status("bad"), "error");

        // Both were marked fired, so neither `at:` is due on the next tick.
        assert!(schedules::due_schedules(&sdir, 2000)
            .expect("due")
            .is_empty());
        let again = tick(&storage, &SelectiveFailProvider, &workspace, dt, 2000)
            .await
            .expect("tick2");
        assert_eq!(again, 0);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn schedule_allowed_tools_restores_strings_and_defaults_to_empty() {
        let home = std::env::temp_dir().join(format!("fleety-sched-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Storage::new(home.clone());

        assert!(schedule_allowed_tools(&storage, "interactive-c1").is_empty());
        assert!(schedule_allowed_tools(&storage, "schedule-missing").is_empty());

        let sdir = storage.schedules_dir();
        std::fs::create_dir_all(&sdir).expect("mk sdir");
        std::fs::write(sdir.join("bad.json"), "{not json").expect("bad schedule");
        assert!(schedule_allowed_tools(&storage, "schedule-bad").is_empty());

        std::fs::write(
            sdir.join("s1.json"),
            r#"{"id":"s1","allowed_tools":["read_file",42,"run_command",null]}"#,
        )
        .expect("schedule");
        assert_eq!(
            schedule_allowed_tools(&storage, "schedule-s1"),
            vec!["read_file".to_string(), "run_command".to_string()]
        );

        std::fs::write(sdir.join("s2.json"), r#"{"id":"s2"}"#).expect("schedule");
        assert!(schedule_allowed_tools(&storage, "schedule-s2").is_empty());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn recovery_with_empty_journal_clears_marker_without_running_agent() {
        let home =
            std::env::temp_dir().join(format!("fleety-empty-recover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Arc::new(Storage::new(home.clone()));
        let conv = "schedule-empty";
        storage
            .journal_begin(SCHED_DEVICE, conv, &Message::user("status"))
            .expect("begin");

        recover_schedule_turn(&storage, &EchoProvider, &ToolRegistry::new(), conv)
            .await
            .expect("recover");

        assert!(storage.list_incomplete_turns().expect("list").is_empty());
        assert!(storage.load(SCHED_DEVICE, conv).expect("load").is_empty());

        let _ = std::fs::remove_dir_all(&home);
    }
}
