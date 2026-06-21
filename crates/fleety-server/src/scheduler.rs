//! Schedule fire loop: periodically run due schedules unattended.
//!
//! Due `at:`/`every:` schedules run through the agent with an **unattended
//! policy** (`RequireApproval` + `AutoDeny`): reads and reporting proceed, but
//! mutate/critical tools are denied — there is no human to confirm and mandate
//! enforcement is a later milestone. Each run is persisted to a `schedule-<id>`
//! conversation and audited, and the schedule's `last_run` is set.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{
    run_turn, AutoDeny, EventLog, LoopConfig, Message, ModelProvider, Policy, Result,
};

use crate::schedules;
use crate::storage::Storage;

const SCHED_DEVICE: &str = "scheduler";

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
    storage: &Storage,
    provider: &dyn ModelProvider,
    workspace: &Path,
    now: u64,
) -> Result<usize> {
    let due = schedules::due_schedules(&storage.schedules_dir(), now)?;
    if due.is_empty() {
        return Ok(0);
    }
    let mut tools = crate::tools::build_registry(
        workspace,
        &storage.backups_dir(),
        &storage.memory_dir(),
        &storage.history_path(SCHED_DEVICE),
        &storage.devices_dir(),
        &storage.schedules_dir(),
    );
    crate::skills::register(
        &mut tools,
        &storage.skills_builtin_dir(),
        &storage.skills_installed_dir(),
    );
    crate::web::register(&mut tools);
    crate::mcp::register(&mut tools, &storage.mcp_config_path());
    let mut fired = 0;
    for item in due {
        let conversation = format!("schedule-{}", item.id);
        tracing::info!(schedule = %item.id, "firing schedule");
        storage.append(SCHED_DEVICE, &conversation, &Message::user(item.prompt))?;
        let mut messages = vec![Message::system(storage.core_memory()?)];
        messages.extend(storage.load(SCHED_DEVICE, &conversation)?);
        let mut events = EventLog::new();
        let mut gate = AutoDeny;
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
        schedules::mark_fired(&storage.schedules_dir(), &item.id, now)?;
        fired += 1;
    }
    Ok(fired)
}

/// Spawn the periodic fire loop. A failing tick is logged and isolated; it never
/// brings the server down.
pub fn spawn(
    storage: Arc<Storage>,
    provider: Arc<dyn ModelProvider>,
    workspace: Arc<PathBuf>,
    tick_secs: u64,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(tick_secs.max(1)));
        loop {
            interval.tick().await;
            match tick(&storage, provider.as_ref(), &workspace, now_secs()).await {
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

    #[tokio::test]
    async fn tick_fires_due_at_schedule_once() {
        let home = std::env::temp_dir().join(format!("fleety-tick-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let storage = Storage::new(home.clone());
        let workspace = home.clone(); // any real dir serves as the tool root

        let sdir = storage.schedules_dir();
        std::fs::create_dir_all(&sdir).expect("mk sdir");
        std::fs::write(
            sdir.join("s1.json"),
            r#"{"id":"s1","trigger":"at:1","prompt":"status report","enabled":true}"#,
        )
        .expect("write schedule");

        let provider = EchoProvider;
        let fired = tick(&storage, &provider, &workspace, 1000)
            .await
            .expect("tick");
        assert_eq!(fired, 1);

        let msgs = storage.load(SCHED_DEVICE, "schedule-s1").expect("load");
        assert!(msgs.len() >= 2); // user prompt + assistant reply

        // `at:` already fired -> not due again
        let again = tick(&storage, &provider, &workspace, 2000)
            .await
            .expect("tick2");
        assert_eq!(again, 0);

        let _ = std::fs::remove_dir_all(&home);
    }
}
