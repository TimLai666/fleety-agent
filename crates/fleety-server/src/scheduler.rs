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
    reconstruct_messages, run_turn, LoopConfig, MandateGate, Message, ModelProvider, Policy,
    Result, ToolRegistry,
};

use crate::schedules;
use crate::storage::Storage;

pub(crate) const SCHED_DEVICE: &str = "scheduler";

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
        let conversation = format!("schedule-{}", item.id);
        tracing::info!(schedule = %item.id, "firing schedule");
        let user_msg = Message::user(item.prompt);
        storage.append(SCHED_DEVICE, &conversation, &user_msg)?;
        storage.journal_begin(SCHED_DEVICE, &conversation, &user_msg)?;
        let mut messages = vec![Message::system(
            storage.system_prompt_for(&storage.acting_for_device(SCHED_DEVICE))?,
        )];
        messages.extend(storage.load(SCHED_DEVICE, &conversation)?);
        // Journal each event so a crash mid-run is recoverable on the next tick.
        let mut events = storage.journaling_log(SCHED_DEVICE, &conversation);
        // Mandate enforcement: only the schedule's allowed_tools may mutate.
        let mut gate = MandateGate::new(item.allowed_tools);
        let outcome = run_turn(
            provider,
            &tools,
            &mut messages,
            &mut events,
            &LoopConfig::default(),
            Policy::RequireApproval,
            &mut gate,
        )
        .await?;
        for event in events.events() {
            storage.append_history(SCHED_DEVICE, event)?;
        }
        storage.append(
            SCHED_DEVICE,
            &conversation,
            &Message::assistant(outcome.output),
        )?;
        storage.journal_end(SCHED_DEVICE, &conversation)?;
        schedules::mark_fired(&storage.schedules_dir(), &item.id, now)?;
        fired += 1;
    }
    Ok(fired)
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
    let mut gate = MandateGate::new(schedule_allowed_tools(storage, conversation));
    let outcome = run_turn(
        provider,
        tools,
        &mut messages,
        &mut log,
        &config,
        Policy::RequireApproval,
        &mut gate,
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

        // `at:` already fired -> not due again
        let again = tick(&storage, &provider, &workspace, dt, 2000)
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
