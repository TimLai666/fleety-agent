//! fleetyd — the Fleety device background service.
//!
//! Connects to the Agent on startup (registering this device) and stays
//! connected across drops and device sleep via an exponential-backoff reconnect
//! loop. Runs as a background OS service (systemd `--user` / launchd / Windows
//! SCM) controlled by the `install`/`start`/`stop`/`restart`/`enable`/`disable`/
//! `status` subcommands; single-instance via a pidfile in service mode. A clean
//! stop (Ctrl+C, SIGTERM, or SCM Stop) shuts down gracefully between frames, and
//! a self-update restarts the service when idle.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backoff;
mod colocation;
mod ondevice;
mod poll_updates;
mod provision;
mod service;
#[cfg(windows)]
mod winsvc;

use agent_core::{obs, Result};
use clap::{Arg, ArgAction, Command};

use fleety_protocol::{
    ChangeOp, ClientMsg, ConfigChange, ConfigEntry, Effect, ServerMsg, WireError, PROTOCOL_VERSION,
};
use fleety_tools::connection::{self, Resolved, Source, Target};

const RECONNECT_ACK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const RECONNECT_POLL: std::time::Duration = std::time::Duration::from_millis(50);
const RECONNECT_HANDSHAKE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

fn control_nonce() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}-{sequence}", std::process::id())
}

#[derive(Debug)]
struct ControlReady {
    pid: u32,
    instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectRequest {
    instance: String,
    nonce: String,
    expected_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectAck {
    nonce: String,
    accepted: bool,
    message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReconnectJournalEvent {
    Submitted { request: ReconnectRequest },
    Claimed { nonce: String },
    Settled { ack: ReconnectAck },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReconnectPhase {
    Submitted,
    Claimed,
    Settled(ReconnectAck),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectJournalState {
    request: ReconnectRequest,
    phase: ReconnectPhase,
}

#[derive(Debug, Clone)]
struct PendingReconnect {
    request: ReconnectRequest,
    decision: Option<ReconnectAck>,
}

impl PendingReconnect {
    fn new(request: ReconnectRequest) -> Self {
        Self {
            request,
            decision: None,
        }
    }
}

fn encode_ready(ready: &ControlReady) -> Vec<u8> {
    serde_json::json!({ "pid": ready.pid, "instance": ready.instance })
        .to_string()
        .into_bytes()
}

fn parse_ready(bytes: &[u8]) -> Option<ControlReady> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    Some(ControlReady {
        pid: u32::try_from(value.get("pid")?.as_u64()?).ok()?,
        instance: value.get("instance")?.as_str()?.to_string(),
    })
}

fn control_path(name: &str) -> std::path::PathBuf {
    connection::connections_path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(name)
}

fn ready_path() -> std::path::PathBuf {
    control_path("fleetyd.control-ready.json")
}

fn reconnect_journal_path() -> std::path::PathBuf {
    control_path("fleetyd.reconnect-journal.jsonl")
}

fn reconnect_lock_path() -> std::path::PathBuf {
    control_path("fleetyd.reconnect.lock")
}

fn reconnect_journal_error(message: impl Into<String>) -> agent_core::CoreError {
    agent_core::CoreError::Message(format!(
        "fleetyd reconnect journal is invalid: {}",
        message.into()
    ))
}

fn reconnect_event_from_value(value: &serde_json::Value) -> Result<ReconnectJournalEvent> {
    let event = value
        .get("event")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| reconnect_journal_error("event kind is missing"))?;
    let nonce = || {
        value
            .get("nonce")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| reconnect_journal_error("event nonce is missing"))
    };
    match event {
        "submitted" => Ok(ReconnectJournalEvent::Submitted {
            request: ReconnectRequest {
                instance: value
                    .get("instance")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| reconnect_journal_error("request instance is missing"))?,
                nonce: nonce()?,
                expected_profile: value
                    .get("expected_profile")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| {
                        reconnect_journal_error("request expected_profile is missing")
                    })?,
            },
        }),
        "claimed" => Ok(ReconnectJournalEvent::Claimed { nonce: nonce()? }),
        "settled" => Ok(ReconnectJournalEvent::Settled {
            ack: ReconnectAck {
                nonce: nonce()?,
                accepted: value
                    .get("accepted")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or_else(|| {
                        reconnect_journal_error("settlement accepted flag is missing")
                    })?,
                message: value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| reconnect_journal_error("settlement message is missing"))?,
            },
        }),
        other => Err(reconnect_journal_error(format!(
            "unknown event kind '{other}'"
        ))),
    }
}

fn reconnect_event_value(event: &ReconnectJournalEvent) -> serde_json::Value {
    match event {
        ReconnectJournalEvent::Submitted { request } => serde_json::json!({
            "event": "submitted",
            "instance": request.instance,
            "nonce": request.nonce,
            "expected_profile": request.expected_profile,
        }),
        ReconnectJournalEvent::Claimed { nonce } => serde_json::json!({
            "event": "claimed",
            "nonce": nonce,
        }),
        ReconnectJournalEvent::Settled { ack } => serde_json::json!({
            "event": "settled",
            "nonce": ack.nonce,
            "accepted": ack.accepted,
            "message": ack.message,
        }),
    }
}

fn load_reconnect_journal_at(path: &std::path::Path) -> Result<Option<ReconnectJournalState>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot read fleetyd reconnect journal {}: {error}",
                path.display()
            )))
        }
    };
    // A crash can leave only the final append torn. Ignore that uncommitted
    // tail while reading; append_reconnect_event_at truncates it under the
    // reconnect lease before writing the next complete record.
    let complete_len = if bytes.last() == Some(&b'\n') {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    };
    let text = std::str::from_utf8(&bytes[..complete_len])
        .map_err(|error| reconnect_journal_error(format!("not UTF-8: {error}")))?;
    let mut state: Option<ReconnectJournalState> = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            reconnect_journal_error(format!("line {} is not valid JSON: {error}", index + 1))
        })?;
        let event = reconnect_event_from_value(&value)?;
        match event {
            ReconnectJournalEvent::Submitted { request } => match &state {
                None => {
                    state = Some(ReconnectJournalState {
                        request,
                        phase: ReconnectPhase::Submitted,
                    });
                }
                Some(existing)
                    if existing.request == request
                        && matches!(existing.phase, ReconnectPhase::Submitted) => {}
                Some(_) => {
                    return Err(reconnect_journal_error(
                        "a second request was appended before the first was reaped",
                    ))
                }
            },
            ReconnectJournalEvent::Claimed { nonce } => {
                let current = state
                    .as_mut()
                    .ok_or_else(|| reconnect_journal_error("claim precedes submission"))?;
                if current.request.nonce != nonce {
                    return Err(reconnect_journal_error(
                        "claim nonce does not match request",
                    ));
                }
                match current.phase {
                    ReconnectPhase::Submitted => current.phase = ReconnectPhase::Claimed,
                    ReconnectPhase::Claimed => {}
                    ReconnectPhase::Settled(_) => {
                        return Err(reconnect_journal_error("claim follows terminal settlement"))
                    }
                }
            }
            ReconnectJournalEvent::Settled { ack } => {
                let current = state
                    .as_mut()
                    .ok_or_else(|| reconnect_journal_error("settlement precedes submission"))?;
                if current.request.nonce != ack.nonce {
                    return Err(reconnect_journal_error(
                        "settlement nonce does not match request",
                    ));
                }
                match &current.phase {
                    ReconnectPhase::Submitted | ReconnectPhase::Claimed => {
                        current.phase = ReconnectPhase::Settled(ack);
                    }
                    ReconnectPhase::Settled(existing) if existing == &ack => {}
                    ReconnectPhase::Settled(_) => {
                        return Err(reconnect_journal_error(
                            "nonce has conflicting terminal settlements",
                        ))
                    }
                }
            }
        }
    }
    Ok(state)
}

fn append_reconnect_event_at(path: &std::path::Path, event: &ReconnectJournalEvent) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            agent_core::CoreError::Message(format!(
                "cannot create daemon control directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    if let Ok(existing) = std::fs::read(path) {
        if existing.last().is_some_and(|byte| *byte != b'\n') {
            let complete_len = existing
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |index| index + 1);
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(|error| {
                    agent_core::CoreError::Message(format!(
                        "cannot repair fleetyd reconnect journal {}: {error}",
                        path.display()
                    ))
                })?;
            file.set_len(complete_len as u64)
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    agent_core::CoreError::Message(format!(
                        "cannot make fleetyd reconnect journal repair durable {}: {error}",
                        path.display()
                    ))
                })?;
        }
    }
    let mut bytes = reconnect_event_value(event).to_string().into_bytes();
    bytes.push(b'\n');
    let created = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| {
            agent_core::CoreError::Message(format!(
                "cannot open fleetyd reconnect journal {}: {error}",
                path.display()
            ))
        })?;
    use std::io::Write;
    let original_len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
    if let Err(error) = result {
        let _ = file.set_len(original_len);
        let _ = file.sync_all();
        return Err(agent_core::CoreError::Message(format!(
            "cannot make fleetyd reconnect journal event durable {}: {error}",
            path.display()
        )));
    }
    #[cfg(unix)]
    if created {
        let parent = path.parent().ok_or_else(|| {
            agent_core::CoreError::Message(
                "fleetyd reconnect journal has no parent directory".to_string(),
            )
        })?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                agent_core::CoreError::Message(format!(
                    "cannot make fleetyd reconnect journal directory durable {}: {error}",
                    parent.display()
                ))
            })?;
    }
    Ok(())
}

fn submit_reconnect_at(path: &std::path::Path, request: &ReconnectRequest) -> Result<()> {
    if let Some(existing) = load_reconnect_journal_at(path)? {
        let state = match &existing.phase {
            ReconnectPhase::Submitted => "queued",
            ReconnectPhase::Claimed => "being processed",
            ReconnectPhase::Settled(_) => "settled but not yet observed",
        };
        return Err(agent_core::CoreError::Message(format!(
            "fleetyd reconnect request '{}' is {state}; retry after its result is observed",
            existing.request.nonce
        )));
    }
    append_reconnect_event_at(
        path,
        &ReconnectJournalEvent::Submitted {
            request: request.clone(),
        },
    )
}

fn claim_reconnect_at(path: &std::path::Path, instance: &str) -> Result<Option<ReconnectRequest>> {
    let Some(state) = load_reconnect_journal_at(path)? else {
        return Ok(None);
    };
    if state.request.instance != instance {
        return Ok(None);
    }
    match state.phase {
        ReconnectPhase::Submitted => {
            append_reconnect_event_at(
                path,
                &ReconnectJournalEvent::Claimed {
                    nonce: state.request.nonce.clone(),
                },
            )?;
            Ok(Some(state.request))
        }
        ReconnectPhase::Claimed | ReconnectPhase::Settled(_) => Ok(None),
    }
}

fn claim_reconnect(instance: &str) -> Result<Option<ReconnectRequest>> {
    let journal = reconnect_journal_path();
    if !journal.exists() {
        return Ok(None);
    }
    let _lease = acquire_reconnect_lease()?;
    claim_reconnect_at(&journal, instance)
}

fn decide_pending_reconnect(
    pending: &mut Option<PendingReconnect>,
    accepted: bool,
    message: String,
) {
    let Some(pending) = pending.as_mut() else {
        return;
    };
    if pending.decision.is_none() {
        pending.decision = Some(ReconnectAck {
            nonce: pending.request.nonce.clone(),
            accepted,
            message,
        });
    }
}

fn settle_pending_reconnect_at_with<F>(
    path: &std::path::Path,
    pending: &mut Option<PendingReconnect>,
    append: F,
) -> Result<bool>
where
    F: FnOnce(&std::path::Path, &ReconnectJournalEvent) -> Result<()>,
{
    let Some(decision) = pending
        .as_ref()
        .and_then(|pending| pending.decision.clone())
    else {
        return Ok(false);
    };
    append(
        path,
        &ReconnectJournalEvent::Settled {
            ack: decision.clone(),
        },
    )?;
    // A waiting caller may observe and reap the durable event immediately after
    // the append returns. Re-reading here races that legitimate cleanup and can
    // revive an already-observed decision into a journal without its Submitted
    // event. A successful durable append is the settlement commit point.
    pending.take();
    Ok(true)
}

fn settle_pending_reconnect(pending: &mut Option<PendingReconnect>) -> Result<bool> {
    let _lease = acquire_reconnect_lease()?;
    settle_pending_reconnect_at_with(
        &reconnect_journal_path(),
        pending,
        append_reconnect_event_at,
    )
}

async fn settle_pending_reconnect_before_exit_with<F>(
    pending: &mut Option<PendingReconnect>,
    mut settle: F,
) where
    F: FnMut(&mut Option<PendingReconnect>) -> Result<bool>,
{
    while pending
        .as_ref()
        .is_some_and(|pending| pending.decision.is_some())
    {
        match settle(pending) {
            Ok(_) => return,
            Err(error) => {
                tracing::warn!(
                    report = ?error.report(),
                    "cannot persist reconnect settlement during shutdown; will retry"
                );
                tokio::time::sleep(RECONNECT_POLL).await;
            }
        }
    }
}

async fn settle_pending_reconnect_before_exit(pending: &mut Option<PendingReconnect>) {
    settle_pending_reconnect_before_exit_with(pending, settle_pending_reconnect).await;
}

fn settle_authenticated_reconnect_with<F>(
    target: &Resolved,
    server_fingerprint: Option<&str>,
    pending: &mut Option<PendingReconnect>,
    append: F,
) -> Result<bool>
where
    F: FnOnce(&std::path::Path, &ReconnectJournalEvent) -> Result<()>,
{
    let Some(expected_profile) = pending
        .as_ref()
        .map(|pending| pending.request.expected_profile.clone())
    else {
        return Ok(false);
    };
    connection::inspect_locked(|conns| {
        let fingerprint = reconnect_target_fingerprint_in(conns, &expected_profile, target)?;
        if fingerprint.as_deref() != server_fingerprint {
            return Err(agent_core::CoreError::Message(format!(
                "fleetyd authenticated profile '{expected_profile}', but its persisted owner snapshot changed before settlement"
            )));
        }
        let decision = ReconnectAck {
            nonce: pending
                .as_ref()
                .map(|pending| pending.request.nonce.clone())
                .ok_or_else(|| reconnect_journal_error("authenticated request disappeared"))?,
            accepted: true,
            message: "fleetyd authenticated the selected profile and loaded its Server identity"
                .to_string(),
        };
        let _lease = acquire_reconnect_lease()?;
        match append(
            &reconnect_journal_path(),
            &ReconnectJournalEvent::Settled {
                ack: decision.clone(),
            },
        ) {
            Ok(()) => {
                pending.take();
                Ok(true)
            }
            Err(error) => {
                let committed = load_reconnect_journal_at(&reconnect_journal_path())?
                    .is_some_and(|state| {
                        matches!(state.phase, ReconnectPhase::Settled(ref ack) if ack == &decision)
                    });
                if committed {
                    pending.take();
                    Ok(true)
                } else {
                    Err(error)
                }
            }
        }
    })
}

fn settle_authenticated_reconnect(
    target: &Resolved,
    server_fingerprint: Option<&str>,
    pending: &mut Option<PendingReconnect>,
) -> Result<bool> {
    settle_authenticated_reconnect_with(
        target,
        server_fingerprint,
        pending,
        append_reconnect_event_at,
    )
}

fn recover_reconnect_for_instance_at(path: &std::path::Path, instance: &str) -> Result<()> {
    let Some(state) = load_reconnect_journal_at(path)? else {
        return Ok(());
    };
    if state.request.instance == instance || matches!(state.phase, ReconnectPhase::Settled(_)) {
        return Ok(());
    }
    append_reconnect_event_at(
        path,
        &ReconnectJournalEvent::Settled {
            ack: ReconnectAck {
                nonce: state.request.nonce,
                accepted: false,
                message: "fleetyd restarted before the reconnect completed".to_string(),
            },
        },
    )
}

fn reap_reconnect_journal_at(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            #[cfg(unix)]
            if let Some(parent) = path.parent() {
                std::fs::File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| {
                        agent_core::CoreError::Message(format!(
                            "reconnect result was reaped, but its directory was not durable {}: {error}",
                            parent.display()
                        ))
                    })?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(agent_core::CoreError::Message(format!(
            "reconnect result was observed, but its journal could not be reaped {}: {error}",
            path.display()
        ))),
    }
}

struct ControlGuard {
    ready: ControlReady,
}

impl ControlGuard {
    fn claim() -> Result<Self> {
        let ready = ControlReady {
            pid: std::process::id(),
            instance: control_nonce(),
        };
        // Ready ownership, old-generation recovery, and publication are one
        // generation handoff. Two starters must never inspect/remove ready
        // outside the same reconnect lease.
        let _reconnect_lease = acquire_reconnect_lease()?;
        let path = ready_path();
        if let Ok(bytes) = std::fs::read(&path) {
            if let Some(existing) = parse_ready(&bytes) {
                match fleety_tools::service::probe_pid(existing.pid) {
                    fleety_tools::service::PidState::Dead => {
                        let _ = std::fs::remove_file(&path);
                    }
                    fleety_tools::service::PidState::Alive
                    | fleety_tools::service::PidState::Unknown => {
                        return Err(agent_core::CoreError::Message(format!(
                            "another fleetyd process owns local reconnect control (pid {})",
                            existing.pid
                        )));
                    }
                }
            } else {
                return Err(agent_core::CoreError::Message(
                    "fleetyd reconnect control identity is unreadable; remove it only after confirming no daemon is running"
                        .to_string(),
                ));
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                agent_core::CoreError::Message(format!(
                    "cannot create daemon control directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        // Serialize generation handoff with publishers: a request can bind to
        // either the old owner (then recovery settles it) or this new owner,
        // never to an unpublished generation between the two.
        recover_reconnect_for_instance_at(&reconnect_journal_path(), &ready.instance)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                agent_core::CoreError::Message(format!(
                    "cannot claim daemon reconnect control {}: {error}",
                    path.display()
                ))
            })?;
        use std::io::Write;
        if let Err(error) = file.write_all(&encode_ready(&ready)) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(agent_core::CoreError::Message(format!(
                "cannot publish daemon reconnect identity: {error}"
            )));
        }
        Ok(Self { ready })
    }
}

struct ReconnectLease {
    path: std::path::PathBuf,
    owner: String,
}

impl Drop for ReconnectLease {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|owner| owner == self.owner) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn acquire_reconnect_lease() -> Result<ReconnectLease> {
    let path = reconnect_lock_path();
    let owner = format!("{}:{}", std::process::id(), control_nonce());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            agent_core::CoreError::Message(format!(
                "cannot create daemon control directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let started = std::time::Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                if let Err(error) = file
                    .write_all(owner.as_bytes())
                    .and_then(|()| file.sync_all())
                {
                    let _ = std::fs::remove_file(&path);
                    return Err(agent_core::CoreError::Message(format!(
                        "cannot publish fleetyd reconnect lock owner: {error}"
                    )));
                }
                return Ok(ReconnectLease {
                    path,
                    owner: owner.clone(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if started.elapsed() >= RECONNECT_ACK_WAIT {
                    return Err(agent_core::CoreError::Message(
                        "fleetyd reconnect control is locked; retry after the active command \
                         finishes, or remove the lock only after confirming its owner process \
                         has exited"
                            .to_string(),
                    ));
                }
                std::thread::sleep(RECONNECT_POLL);
            }
            Err(error) => {
                return Err(agent_core::CoreError::Message(format!(
                    "cannot lock fleetyd reconnect control: {error}"
                )))
            }
        }
    }
}

impl Drop for ControlGuard {
    fn drop(&mut self) {
        let still_ours = std::fs::read(ready_path())
            .ok()
            .and_then(|bytes| parse_ready(&bytes))
            .is_some_and(|ready| ready.instance == self.ready.instance);
        if still_ours {
            let _ = std::fs::remove_file(ready_path());
        }
    }
}

async fn wait_reconnect_request(control: Option<&ControlGuard>) -> ReconnectRequest {
    let Some(control) = control else {
        return std::future::pending::<ReconnectRequest>().await;
    };
    loop {
        match claim_reconnect(&control.ready.instance) {
            Ok(Some(request)) => {
                if std::env::var("FLEETY_AGENT_URL").is_ok_and(|url| !url.is_empty()) {
                    let mut pending = Some(PendingReconnect::new(request));
                    decide_pending_reconnect(
                        &mut pending,
                    false,
                    "fleetyd is pinned by FLEETY_AGENT_URL and cannot follow a profile switch; unset the Daemon owner override, restart fleetyd, then retry"
                        .to_string(),
                );
                    while pending.is_some() {
                        if let Err(error) = settle_pending_reconnect(&mut pending) {
                            tracing::warn!(report = ?error.report(), "cannot persist reconnect rejection; will retry");
                            tokio::time::sleep(RECONNECT_POLL).await;
                        }
                    }
                    continue;
                }
                let current = connection::load().ok().and_then(|conns| conns.current);
                if current.as_deref() == Some(request.expected_profile.as_str()) {
                    return request;
                }
                let expected_profile = request.expected_profile.clone();
                let mut pending = Some(PendingReconnect::new(request));
                decide_pending_reconnect(
                    &mut pending,
                    false,
                    format!(
                        "requested profile '{}' is no longer current (current: {})",
                        expected_profile,
                        current.as_deref().unwrap_or("none")
                    ),
                );
                while pending.is_some() {
                    if let Err(error) = settle_pending_reconnect(&mut pending) {
                        tracing::warn!(report = ?error.report(), "cannot persist reconnect rejection; will retry");
                        tokio::time::sleep(RECONNECT_POLL).await;
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(report = ?error.report(), "cannot claim durable reconnect request");
            }
        }
        tokio::time::sleep(RECONNECT_POLL).await;
    }
}

fn request_running_daemon_reconnect(expected_profile: &str) -> Result<String> {
    request_running_daemon_reconnect_with_wait(expected_profile, RECONNECT_ACK_WAIT)
}

fn request_running_daemon_reconnect_with_wait(
    expected_profile: &str,
    wait: std::time::Duration,
) -> Result<String> {
    // Keep this lease only through ready-generation capture and submission.
    // The Daemon needs the same lease to claim and settle the request.
    let lease = acquire_reconnect_lease()?;
    let ready = match std::fs::read(ready_path()) {
        Ok(bytes) => parse_ready(&bytes).ok_or_else(|| {
            agent_core::CoreError::Message(
                "fleetyd control state is unreadable; restart fleetyd, then retry".to_string(),
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(format!(
                "no running fleetyd owns this connection store; profile '{expected_profile}' will be used when that Daemon owner starts"
            ));
        }
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot read fleetyd control state: {error}"
            )))
        }
    };
    let request = ReconnectRequest {
        instance: ready.instance,
        nonce: control_nonce(),
        expected_profile: expected_profile.to_string(),
    };
    let journal = reconnect_journal_path();
    if let Some(state) = load_reconnect_journal_at(&journal)? {
        if let ReconnectPhase::Settled(ack) = state.phase {
            reap_reconnect_journal_at(&journal)?;
            if state.request.expected_profile != expected_profile {
                return Err(agent_core::CoreError::Message(format!(
                    "previous reconnect request '{}' for profile '{}' settled {}; profile \
                     '{expected_profile}' was not submitted — retry now",
                    state.request.nonce,
                    state.request.expected_profile,
                    if ack.accepted {
                        "successfully"
                    } else {
                        "with failure"
                    }
                )));
            }
            return if ack.accepted {
                Ok(ack.message)
            } else {
                Err(agent_core::CoreError::Message(ack.message))
            };
        }
    }
    submit_reconnect_at(&journal, &request)?;
    drop(lease);

    let deadline = std::time::Instant::now() + wait;
    while std::time::Instant::now() < deadline {
        if let Some(state) = load_reconnect_journal_at(&journal)? {
            if state.request.nonce == request.nonce {
                if let ReconnectPhase::Settled(observed_ack) = state.phase {
                    let _observe_lease = acquire_reconnect_lease()?;
                    if let Some(observed) = load_reconnect_journal_at(&journal)? {
                        if observed.request.nonce == request.nonce {
                            match observed.phase {
                                ReconnectPhase::Settled(ref ack) if ack == &observed_ack => {}
                                ReconnectPhase::Settled(_) => {
                                    return Err(reconnect_journal_error(
                                        "nonce changed terminal result while being observed",
                                    ))
                                }
                                ReconnectPhase::Submitted | ReconnectPhase::Claimed => {
                                    return Err(reconnect_journal_error(
                                        "nonce regressed after terminal result was observed",
                                    ))
                                }
                            }
                            reap_reconnect_journal_at(&journal)?;
                        }
                    }
                    // Another same-profile caller may have reaped this exact
                    // result after our durable read. The result was still
                    // observed; never turn that race into a false timeout.
                    return if observed_ack.accepted {
                        Ok(observed_ack.message)
                    } else {
                        Err(agent_core::CoreError::Message(observed_ack.message))
                    };
                }
            }
        }
        std::thread::sleep(RECONNECT_POLL);
    }
    Err(agent_core::CoreError::Message(format!(
        "profile '{expected_profile}' was saved, but running fleetyd did not finish reconnecting within {} seconds; request '{}' remains durable and a second request will be refused until it settles",
        wait.as_secs(),
        request.nonce
    )))
}

/// Return the exact persisted profile that owns this resolved target. A pure
/// env/default deployment may create `default`; an env URL that differs from an
/// existing current profile deliberately has no persisted owner.
fn target_profile_name(
    conns: &connection::Connections,
    target: &Resolved,
    allow_default: bool,
) -> Option<String> {
    match &target.source {
        Source::Profile(name) | Source::OverrideProfile(name) => conns
            .profiles
            .get(name)
            .filter(|profile| profile.url == target.url)
            .map(|_| name.clone()),
        Source::OverrideUrl => conns
            .profiles
            .iter()
            .find(|(_, profile)| profile.url == target.url)
            .map(|(name, _)| name.clone()),
        Source::Env => match conns.current.as_ref() {
            Some(name) => conns
                .profiles
                .get(name)
                .filter(|profile| profile.url == target.url)
                .map(|_| name.clone()),
            None if allow_default && conns.profiles.is_empty() => Some("default".to_string()),
            None => None,
        },
        Source::Mdns | Source::Default => match conns.current.as_ref() {
            // Unowned discovery must never be attached to an existing current
            // profile. Unsigned discovery never becomes Source::Profile.
            Some(_) => None,
            None if allow_default && conns.profiles.is_empty() => Some("default".to_string()),
            None => None,
        },
    }
}

/// Confirm that the resolved target is still the exact persisted owner snapshot
/// selected by this reconnect. Re-run this before terminal success so a later
/// profile mutation cannot make one nonce acknowledge another owner.
fn reconnect_target_fingerprint(
    expected_profile: &str,
    target: &Resolved,
) -> Result<Option<String>> {
    let conns = connection::load()?;
    reconnect_target_fingerprint_in(&conns, expected_profile, target)
}

fn reconnect_target_fingerprint_in(
    conns: &connection::Connections,
    expected_profile: &str,
    target: &Resolved,
) -> Result<Option<String>> {
    let source_profile = match &target.source {
        Source::Profile(name) | Source::OverrideProfile(name) => name,
        _ => {
            return Err(agent_core::CoreError::Message(format!(
                "reconnect profile '{expected_profile}' did not resolve to a persisted owner"
            )))
        }
    };
    if source_profile != expected_profile {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect requested profile '{expected_profile}', but resolved profile '{source_profile}'"
        )));
    }
    if conns.current.as_deref() != Some(expected_profile) {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' is no longer current"
        )));
    }
    let profile = conns.profiles.get(expected_profile).ok_or_else(|| {
        agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' no longer exists"
        ))
    })?;
    if profile.url != target.url || profile.token.as_deref() != target.token.as_deref() {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' changed while fleetyd was connecting"
        )));
    }
    Ok(profile.fingerprint.clone())
}

/// Persist a freshly-minted token only onto the profile that owned the resolved
/// connection. Returns false when the target is intentionally ephemeral.
fn persist_token(target: &Resolved, token: &str) -> Result<bool> {
    connection::mutate(|conns| {
        let Some(name) = target_profile_name(conns, target, true) else {
            return Ok(false);
        };
        let first_profile = conns.current.is_none() && conns.profiles.is_empty();
        if conns.device_id.is_empty() {
            conns.device_id = fleety_tools::device::device_id();
        }
        let profile = conns.profiles.entry(name.clone()).or_default();
        if profile.url.is_empty() {
            profile.url = target.url.clone();
        }
        profile.token = Some(token.to_string());
        if first_profile {
            conns.current = Some(name);
        }
        Ok(true)
    })
}

/// Clear only the token that was actually sent to the rejecting target.
fn clear_target_token(target: &Resolved) -> Result<bool> {
    connection::mutate(|conns| {
        let Some(name) = target_profile_name(conns, target, false) else {
            return Ok(false);
        };
        let Some(profile) = conns.profiles.get_mut(&name) else {
            return Ok(false);
        };
        // An explicit FLEETY_TOKEN may target the same URL without being the saved
        // profile credential. Only delete the exact token that was rejected.
        if profile.token.as_deref() != target.token.as_deref() || target.token.is_none() {
            return Ok(false);
        }
        profile.token = None;
        Ok(true)
    })
}

fn pin_target_fingerprint(
    target: &Resolved,
    fingerprint: &str,
) -> Result<Option<connection::PinDecision>> {
    connection::mutate(|conns| {
        let Some(name) = target_profile_name(conns, target, false) else {
            return Ok(None);
        };
        let Some(profile) = conns.profiles.get_mut(&name) else {
            return Ok(None);
        };
        let decision = connection::tofu_pin_decision(profile.fingerprint.as_deref(), fingerprint);
        if decision == connection::PinDecision::Pin {
            profile.fingerprint = Some(fingerprint.to_string());
        }
        Ok(Some(decision))
    })
}

fn command() -> Command {
    let lifecycle = |name| Command::new(name).about("Manage the installed Daemon service");
    Command::new("fleetyd")
        .version(agent_core::VERSION)
        .about("Fleety device background service")
        .after_help("With no command, fleetyd runs in the foreground.")
        .arg(
            Arg::new("legacy-version")
                .short('v')
                .action(ArgAction::Version)
                .hide(true),
        )
        .subcommands([
            lifecycle("run-service").hide(true),
            lifecycle("install"),
            lifecycle("uninstall"),
            lifecycle("start"),
            lifecycle("stop"),
            lifecycle("restart"),
            lifecycle("enable"),
            lifecycle("disable"),
            lifecycle("status"),
            Command::new("reconnect")
                .about("Reconnect the running Daemon to the selected profile")
                .hide(true)
                .arg(
                    Arg::new("profile")
                        .long("profile")
                        .required(true)
                        .value_name("NAME"),
                ),
            lifecycle("update"),
            fleety_tools::config::clap_command_for_daemon(),
            Command::new("version").about("Print the version"),
        ])
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("fleetyd".to_string());
    argv.extend(args.iter().cloned());
    if let Err(error) = command().try_get_matches_from(argv) {
        let code = error.exit_code();
        let _ = error.print();
        return std::process::ExitCode::from(code as u8);
    }
    let cmd = args.first().cloned();
    if cmd.as_deref() == Some("version") {
        println!("fleetyd {}", agent_core::VERSION);
        return std::process::ExitCode::SUCCESS;
    }

    obs::init();
    // Seed env from ~/.fleety/config.toml before reading env (env still wins;
    // only unset keys are filled). Best-effort.
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    // One-time, idempotent migration of the legacy config.json / fleetyd.token
    // into connections.toml (best-effort; a fresh device has nothing to migrate).
    let _ = connection::migrate_from_config_json();
    // `config ...` inspects/edits this host's settings, then exits — no runtime
    // needed. Same command surface as `fleety config`.
    if cmd.as_deref() == Some("config") {
        if let Err(e) =
            fleety_tools::config::run_scoped(&args[1..], Some(fleety_tools::config::DAEMON_SCOPES))
        {
            let report = e.report();
            eprintln!(
                "error: {}",
                fleety_tools::transport::terminal_safe_multiline(&report.message)
            );
            if let Some(hint) = report.remediation {
                eprintln!(
                    "hint: {}",
                    fleety_tools::transport::terminal_safe_multiline(&hint)
                );
            }
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }
    if cmd.as_deref() == Some("reconnect") {
        let profile = args
            .windows(2)
            .find(|pair| pair[0] == "--profile")
            .map(|pair| pair[1].as_str())
            .unwrap_or_default();
        return match request_running_daemon_reconnect(profile) {
            Ok(message) => {
                println!("{message}");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {}", error.report().message);
                std::process::ExitCode::FAILURE
            }
        };
    }

    // On Windows, `run-service` is the SCM entry point: it must talk the service
    // control protocol on this thread (not under a tokio runtime), so handle it
    // before building one. The dispatcher blocks until the service stops and runs
    // the daemon on its own runtime (see winsvc).
    #[cfg(windows)]
    if cmd.as_deref() == Some("run-service") {
        if let Err(e) = winsvc::dispatch() {
            tracing::error!(
                %e,
                "windows service dispatcher failed; `run-service` only works when started \
                 by the Service Control Manager (use `fleetyd start` after `fleetyd install`)"
            );
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start tokio runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    rt.block_on(async_main(cmd))
}

/// Forward-only fleet convergence. Bring this host's fleety binaries to the
/// server's exact version when the server is newer; never downgrade. If this
/// device is ahead, only warn (the operator should upgrade the server). Skips
/// silently when no update manifest is configured.
async fn converge_to_server_version(server_version: &str) {
    if server_version.is_empty() || std::env::var("FLEETY_UPDATE_MANIFEST").is_err() {
        return; // older server, or no updater configured on this host
    }
    let me = agent_core::VERSION;
    if fleety_tools::update::is_newer(me, server_version) {
        tracing::warn!(
            device = me,
            server = server_version,
            "this device is newer than the server; upgrade the server so the fleet converges \
             (devices never auto-downgrade)"
        );
        return;
    }
    if !fleety_tools::update::is_newer(server_version, me) {
        return; // already matched
    }
    tracing::info!(
        device = me,
        server = server_version,
        "server is newer; converging this host"
    );

    // The resolution chain lives in fleety_tools::update::converge_to_version:
    // an env {version} template pins directly; otherwise the binary's latest
    // manifest either already matches or names the pinned manifest via its
    // versioned_manifest template. When neither applies, the error names both
    // remedies (publish versioned_manifest, or switch to a {version} template)
    // and this host just stays put — forward-only, never a wrong install.
    let self_updated = match fleety_tools::update::converge_self_to_version(server_version).await {
        Ok(updated) => updated,
        Err(e) => {
            tracing::warn!(report = ?e.report(), "could not self-update fleetyd to the server version");
            false
        }
    };
    // Bring sibling binaries on this host to the same version. fleety-server is a
    // service: a bare `restart` (no --force) asks the running server to restart
    // once it is idle (deferred until no in-flight turn), rather than hard-killing
    // it mid-turn; the fleety CLI just needs its binary swapped.
    for bin in ["fleety", "fleety-server"] {
        let Some(exe) = fleety_tools::update::sibling_exe(bin) else {
            continue;
        };
        match fleety_tools::update::converge_to_version(bin, &exe, server_version).await {
            Ok(true) if bin == "fleety-server" => {
                // Deferred restart: never add --force here so an update never
                // interrupts an in-flight turn before the deferral deadline.
                let _ = std::process::Command::new(&exe).arg("restart").status();
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(report = ?e.report(), bin, "could not update sibling to the server version")
            }
        }
    }
    if self_updated {
        // Restart at the next idle frame so the new fleetyd takes over.
        request_self_restart("converge-to-server-version");
    }
}

async fn async_main(cmd: Option<String>) -> std::process::ExitCode {
    // Lifecycle subcommands act on the OS service manager and exit; `run-service`
    // and no subcommand fall through to actually run the daemon.
    match cmd.as_deref() {
        Some("version") | Some("--version") | Some("-v") | Some("-V") => {
            println!("fleetyd {}", agent_core::VERSION);
            return std::process::ExitCode::SUCCESS;
        }
        Some("install") => {
            if let Err(e) = service::install() {
                return log_verb("install", Err(e));
            }
            // Provision the data-analysis sidecar (best-effort — the daemon
            // works without it, but say so on the console: otherwise the user
            // first learns at an `insyra_exec` failure much later).
            if let Err(e) = provision::ensure_insyra(false).await {
                eprintln!(
                    "note: could not provision the fleety-insyra sidecar ({}); data analysis \
                     (insyra_exec) will be unavailable on this device until `fleetyd update` \
                     succeeds",
                    e.report().message
                );
            }
            return std::process::ExitCode::SUCCESS;
        }
        Some("uninstall") => return log_verb("uninstall", service::uninstall()),
        Some("start") => return log_verb("start", service::start()),
        Some("stop") => return log_verb("stop", service::stop()),
        Some("restart") => return log_verb("restart", service::restart()),
        Some("enable") => return log_verb("enable", service::enable()),
        Some("disable") => return log_verb("disable", service::disable()),
        Some("status") => return log_verb("status", service::status()),
        Some("update") => {
            let mut code = std::process::ExitCode::SUCCESS;
            match fleety_tools::update::self_update().await {
                Ok(true) => {
                    // We swapped the binary; restart the installed service (best
                    // effort) so it runs the new exe. The manager stop is graceful
                    // (SIGTERM / SCM Stop handled between frames).
                    if let Err(e) = service::restart() {
                        eprintln!(
                            "updated, but could not restart the service automatically ({}) — \
                             restart fleetyd to apply",
                            e.report().message
                        );
                    }
                }
                Ok(false) => {}
                Err(e) => code = log_verb("update", Err(e)),
            }
            // Refresh the data-analysis sidecar alongside fleetyd (best-effort).
            if let Err(e) = provision::ensure_insyra(true).await {
                eprintln!(
                    "note: could not refresh the fleety-insyra sidecar ({}); insyra_exec may \
                     stay on the old version or be unavailable",
                    e.report().message
                );
            }
            // Host-wide: bring the sibling fleety binaries on this machine
            // along (gated on a {bin} template; fleety-server restarts
            // deferred). Best-effort — a sibling failure never fails the
            // daemon's own update.
            if let Err(e) = fleety_tools::update::update_siblings_to_latest(
                &fleety_tools::update::host_siblings_of("fleetyd"),
            )
            .await
            {
                eprintln!(
                    "note: sibling updates did not complete: {}",
                    e.report().message
                );
            }
            return code;
        }
        _ => {}
    }

    // Service mode (non-Windows run-service) claims the single-instance pidfile
    // (defense-in-depth on top of the manager); foreground dev runs do not, so a
    // developer can run one alongside an installed service. On Windows the
    // service path goes through winsvc, which claims the pidfile itself.
    let service_mode = cmd.as_deref() == Some("run-service");
    let _pid_guard = if service_mode {
        match fleety_tools::service::acquire("fleetyd") {
            Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
            Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
                tracing::error!(pid, "another fleetyd is already running; exiting");
                return std::process::ExitCode::FAILURE;
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(), "pidfile check failed; continuing without it");
                None
            }
        }
    } else {
        None
    };

    tracing::info!(version = agent_core::VERSION, "fleetyd starting");
    // Best-effort background update poller (no-op when the user hasn't set
    // FLEETY_UPDATE_MANIFEST — keeps the existing dev/install posture).
    poll_updates::spawn();
    if let Err(e) = run(None).await {
        tracing::error!(report = ?e.report(), "fleetyd exited with error");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

/// Report the outcome of a one-shot lifecycle verb: a clean stderr line (plus
/// hint) on failure, and a non-zero exit code so users and scripts can tell.
fn log_verb(verb: &str, res: Result<()>) -> std::process::ExitCode {
    match res {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            let report = e.report();
            eprintln!("error: {verb} failed: {}", report.message);
            if let Some(hint) = report.remediation {
                eprintln!("hint: {hint}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

/// Ensure this device's external dependencies (best-effort, non-blocking).
/// Default subset insyra; add node/python via `FLEETY_DEPS` for device-side
/// skills/MCP. `FLEETY_AUTO_INSTALL_DEPS=0` disables installing.
async fn ensure_dependencies() {
    use fleety_tools::deps;
    let names = deps::selected_dep_names(
        std::env::var("FLEETY_DEPS").ok().as_deref(),
        deps::daemon_default_deps(),
    );
    let chosen: Vec<deps::Dependency> = names
        .iter()
        .filter_map(|n| match n.as_str() {
            "insyra" => Some(deps::insyra_dependency()),
            "node" => Some(deps::node_dependency()),
            "python" => Some(deps::python_dependency()),
            other => {
                tracing::warn!(dep = %other, "unknown dependency in FLEETY_DEPS; ignoring");
                None
            }
        })
        .collect();
    let outcomes = deps::ensure_all(&chosen).await;
    deps::log_outcomes(&outcomes);
}

/// Process-wide pending restart (set by the auto-update poller). The serve loop
/// checks it at each frame boundary — where the daemon is idle — and carries it
/// out via [`fleety_tools::restart`]'s defer-until-idle policy, so a self-update
/// never interrupts a running on-device tool.
fn pending_restart() -> &'static std::sync::Mutex<Option<fleety_tools::restart::PendingRestart>> {
    static P: std::sync::OnceLock<std::sync::Mutex<Option<fleety_tools::restart::PendingRestart>>> =
        std::sync::OnceLock::new();
    P.get_or_init(|| std::sync::Mutex::new(None))
}

/// Record a deferred self-restart (idempotent while one is already pending).
pub(crate) fn request_self_restart(reason: &str) {
    if let Ok(mut g) = pending_restart().lock() {
        if g.is_none() {
            *g = Some(fleety_tools::restart::PendingRestart::new(
                reason,
                false,
                std::time::Instant::now(),
            ));
            tracing::info!(reason, "self-restart requested; will restart when idle");
        }
    }
}

/// At an idle frame boundary, decide whether a pending restart is due. Consumes
/// it and returns true when the daemon should restart now.
fn restart_due_at_idle() -> bool {
    use fleety_tools::restart::{decide, Decision};
    let Ok(mut g) = pending_restart().lock() else {
        return false;
    };
    if let Some(p) = g.as_ref() {
        // We're between frames here, so the daemon is idle.
        if decide(p, true, None, std::time::Instant::now()) == Decision::RestartNow {
            *g = None;
            return true;
        }
    }
    false
}

/// Resolve which server (url + token) to connect to, via the shared resolver
/// over connections.toml. `FLEETY_AGENT_URL` stays a persistent unit-file source
/// for the daemon (transient env override in the resolver): env > current
/// profile > mDNS > localhost. The daemon has no per-invocation override — it
/// always follows `current`.
fn resolve_target() -> Result<connection::Resolved> {
    let conns = connection::load()?;
    let env_url = std::env::var("FLEETY_AGENT_URL").ok();
    let env_token = std::env::var("FLEETY_TOKEN").ok();
    connection::resolve(&conns, &Target::Current, env_url, env_token, || {
        let discovered =
            connection::discover_for_connections(&conns, std::time::Duration::from_secs(2));
        if let Some(server) = &discovered {
            tracing::info!(url = %server.url, "discovered fleety server via mDNS");
        }
        discovered
    })
}

fn reconnect_connect_error(target: &Resolved, cause: &str) -> String {
    let cause = fleety_tools::transport::redact_urls_in_text(cause);
    if matches!(
        target.source,
        Source::Profile(_) | Source::OverrideProfile(_)
    ) {
        format!("{cause}. {}", connection::explicit_repair_guidance())
    } else {
        cause
    }
}

fn device_id() -> String {
    connection::load()
        .map(|c| c.effective_device_id())
        .unwrap_or_else(|_| fleety_tools::device::device_id())
}

const INTERNAL_CONFIG_TOOL: &str = "__fleety_internal_config";

fn config_failure(kind: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({ "ok": false, "kind": kind, "message": message.into() })
}

fn internal_config(tool: &str, args: &serde_json::Value) -> Option<Result<serde_json::Value>> {
    if tool != INTERNAL_CONFIG_TOOL {
        return None;
    }
    let result = (|| -> Result<serde_json::Value> {
        let op = args
            .get("op")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                agent_core::CoreError::Message("internal config request is missing op".to_string())
            })?;
        let path = fleety_tools::config::config_path();
        match op {
            "exec" => {
                let command: Vec<String> = serde_json::from_value(
                    args.get("args")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .map_err(|e| {
                    agent_core::CoreError::Message(format!("invalid config arguments: {e}"))
                })?;
                if matches!(
                    command.first().map(String::as_str),
                    Some("provider" | "model")
                ) {
                    return Ok(config_failure(
                        "wrong_owner",
                        "provider and model configuration is owned by fleety-server",
                    ));
                }
                match fleety_tools::config::run_rendered_scoped(
                    &command,
                    Some(fleety_tools::config::DAEMON_SCOPES),
                ) {
                    Ok(output) => Ok(serde_json::json!({
                        "ok": true,
                        "output": output,
                        "effect": fleety_tools::config::config_effect(&command).map(|_| Effect::Restart),
                    })),
                    Err(e) => Ok(config_failure("invalid", e.report().message)),
                }
            }
            "snapshot" => match fleety_tools::config::load_strict(&path) {
                Ok(map) => {
                    let entries: Vec<ConfigEntry> = fleety_tools::config::snapshot_entries(
                        &map,
                        Some(fleety_tools::config::DAEMON_SCOPES),
                    )
                    .into_iter()
                    .map(|entry| ConfigEntry {
                        key: entry.key.to_string(),
                        scope: entry.scope.as_str().to_string(),
                        value: entry.value,
                        default: entry.default.to_string(),
                        description: entry.description.to_string(),
                        secret: entry.secret,
                        is_set: entry.is_set,
                        effect: Some(Effect::Restart),
                        choices: entry.choices.into_iter().map(str::to_string).collect(),
                    })
                    .collect();
                    Ok(serde_json::json!({
                        "ok": true,
                        "revision": fleety_tools::config::revision(&path),
                        "entries": entries,
                    }))
                }
                Err(e) => Ok(config_failure("invalid_config", e.report().message)),
            },
            "apply" => {
                let base_revision = args
                    .get("base_revision")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        agent_core::CoreError::Message(
                            "internal config apply is missing base_revision".to_string(),
                        )
                    })?;
                let changes: Vec<ConfigChange> = serde_json::from_value(
                    args.get("changes")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .map_err(|e| {
                    agent_core::CoreError::Message(format!("invalid config changes: {e}"))
                })?;
                let applied = changes
                    .iter()
                    .filter(|change| !matches!(change.op, ChangeOp::Keep))
                    .count();
                let mutation =
                    fleety_tools::config::mutate_strict_at_revision(&path, base_revision, |map| {
                        for change in &changes {
                            fleety_tools::config::ensure_scope(
                                &change.key,
                                fleety_tools::config::DAEMON_SCOPES,
                            )?;
                            let setting =
                                fleety_tools::config::find(&change.key).ok_or_else(|| {
                                    agent_core::CoreError::Message(format!(
                                        "unknown setting '{}'",
                                        change.key
                                    ))
                                })?;
                            match change.op {
                                ChangeOp::Keep => {}
                                ChangeOp::Set => {
                                    let value = change.value.as_deref().ok_or_else(|| {
                                        agent_core::CoreError::Message(format!(
                                            "{} set operation is missing a value",
                                            change.key
                                        ))
                                    })?;
                                    fleety_tools::config::validate(setting, value)?;
                                    map.insert(
                                        (setting.scope, change.key.clone()),
                                        value.to_string(),
                                    );
                                }
                                ChangeOp::Clear => {
                                    map.remove(&(setting.scope, change.key.clone()));
                                }
                            }
                        }
                        Ok(())
                    });
                match mutation {
                    Ok(()) => Ok(serde_json::json!({ "ok": true, "applied": applied })),
                    Err(e) => {
                        let message = e.report().message;
                        let kind = if message.contains("revision conflict") {
                            "conflict"
                        } else {
                            "invalid"
                        };
                        Ok(config_failure(kind, message))
                    }
                }
            }
            other => Ok(config_failure(
                "invalid",
                format!("unknown internal config operation '{other}'"),
            )),
        }
    })();
    Some(result)
}

/// What ended one connected session.
enum Outcome {
    /// A clean shutdown signal (Ctrl+C / service Stop) — exit the process.
    Shutdown,
    /// The link dropped (disconnect/sleep) — the caller should reconnect.
    Disconnected,
    /// The local owner control path requested an immediate profile re-resolve.
    Reconnect(ReconnectRequest),
}

/// Outer reconnect loop: stay connected across transient drops and device
/// sleep. Exits only on a clean shutdown signal; everything else reconnects
/// with exponential backoff (reset after a successful connect). `shutdown` is an
/// optional external stop (set by the Windows service control handler); a clean
/// stop is also delivered by Ctrl+C and, on Unix, SIGTERM (so `systemctl stop`
/// is graceful).
async fn run(shutdown: Option<tokio::sync::watch::Receiver<bool>>) -> Result<()> {
    // Ensure device dependencies in the background (best-effort, non-blocking).
    tokio::spawn(ensure_dependencies());
    let control = match ControlGuard::claim() {
        Ok(control) => Some(control),
        Err(error) => {
            tracing::warn!(report = ?error.report(), "fleetyd local reconnect control is unavailable");
            None
        }
    };
    let mut bo = backoff::Backoff::new();
    let mut pending_reconnect: Option<PendingReconnect> = None;
    loop {
        if pending_reconnect
            .as_ref()
            .is_some_and(|pending| pending.decision.is_some())
        {
            match settle_pending_reconnect(&mut pending_reconnect) {
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(report = ?error.report(), "cannot persist reconnect settlement; will retry");
                    tokio::time::sleep(RECONNECT_POLL).await;
                    continue;
                }
            }
        }
        if let Some(expected) = pending_reconnect
            .as_ref()
            .map(|pending| pending.request.expected_profile.clone())
        {
            let current = connection::load().ok().and_then(|conns| conns.current);
            if current.as_deref() != Some(expected.as_str()) {
                decide_pending_reconnect(
                    &mut pending_reconnect,
                    false,
                    format!(
                        "requested profile '{expected}' is no longer current (current: {})",
                        current.as_deref().unwrap_or("none")
                    ),
                );
                continue;
            }
        }
        // Resolve the server (url + token) via the shared resolver over
        // connections.toml, honoring a persistent FLEETY_AGENT_URL env override.
        match resolve_target() {
            Ok(target) => {
                // WebSocket first, SSE+POST fallback (unless overridden by env) —
                // so a device behind a proxy that blocks the WS upgrade connects.
                let reconnect_deadline = pending_reconnect
                    .as_ref()
                    .map(|_| tokio::time::Instant::now() + RECONNECT_HANDSHAKE_WAIT);
                let connection = if let Some(deadline) = reconnect_deadline {
                    tokio::select! {
                        result = tokio::time::timeout_at(
                            deadline,
                            fleety_tools::transport::connect(&target.url, target.token.as_deref()),
                        ) => match result {
                            Ok(result) => result,
                            Err(_) => {
                                decide_pending_reconnect(
                                    &mut pending_reconnect,
                                    false,
                                    format!(
                                        "fleetyd could not establish and authenticate the selected profile before the reconnect deadline. {}",
                                        connection::explicit_repair_guidance()
                                    ),
                                );
                                continue;
                            }
                        },
                        _ = wait_stop(shutdown.clone()) => {
                            decide_pending_reconnect(
                                &mut pending_reconnect,
                                false,
                                "fleetyd stopped before the reconnect completed".to_string(),
                            );
                            settle_pending_reconnect_before_exit(&mut pending_reconnect).await;
                            return Ok(());
                        }
                    }
                } else {
                    tokio::select! {
                        result = fleety_tools::transport::connect(
                            &target.url,
                            target.token.as_deref(),
                        ) => result,
                        _ = wait_stop(shutdown.clone()) => return Ok(()),
                    }
                };
                match connection {
                    Ok(conn) => {
                        bo.reset();
                        match serve(
                            &target,
                            conn,
                            shutdown.clone(),
                            control.as_ref(),
                            &mut pending_reconnect,
                            reconnect_deadline,
                        )
                        .await
                        {
                            Outcome::Shutdown => return Ok(()),
                            Outcome::Disconnected => {
                                tracing::info!("fleetyd disconnected; will reconnect");
                            }
                            Outcome::Reconnect(request) => {
                                tracing::info!(profile = %request.expected_profile, "profile changed; reconnecting immediately");
                                pending_reconnect = Some(PendingReconnect::new(request));
                                bo.reset();
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        let failure = reconnect_connect_error(&target, &e.report().message);
                        tracing::warn!(message = %failure, "connect failed; will retry");
                        if let Some(expected_profile) = pending_reconnect
                            .as_ref()
                            .map(|pending| pending.request.expected_profile.clone())
                        {
                            decide_pending_reconnect(
                                &mut pending_reconnect,
                                false,
                                format!(
                                    "fleetyd left the previous Server, but could not connect to profile '{}': {}",
                                    expected_profile,
                                    failure
                                ),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(),
                    "cannot resolve a server to connect to (is connections.toml valid?); will retry");
                if let Some(expected_profile) = pending_reconnect
                    .as_ref()
                    .map(|pending| pending.request.expected_profile.clone())
                {
                    decide_pending_reconnect(
                        &mut pending_reconnect,
                        false,
                        format!(
                            "fleetyd left the previous Server, but could not resolve profile '{}': {}",
                            expected_profile,
                            e.report().message
                        ),
                    );
                }
            }
        }
        let delay =
            backoff::with_jitter(bo.next_base(), backoff::JITTER_FRAC, backoff::jitter_unit());
        tracing::info!(?delay, "waiting before reconnect");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            request = wait_reconnect_request(control.as_ref()) => {
                pending_reconnect = Some(PendingReconnect::new(request));
                bo.reset();
                continue;
            }
            _ = wait_stop(shutdown.clone()) => {
                tracing::info!("stop signal received during backoff; shutting down fleetyd");
                decide_pending_reconnect(
                    &mut pending_reconnect,
                    false,
                    "fleetyd stopped before the reconnect completed".to_string(),
                );
                settle_pending_reconnect_before_exit(&mut pending_reconnect).await;
                return Ok(());
            }
        }
    }
}

/// Resolve when the process should stop cleanly: Ctrl+C on any platform, plus
/// SIGTERM on Unix (service stop) or the external `shutdown` watch on Windows
/// (SCM Stop). Used in every place the daemon waits.
async fn wait_stop(shutdown: Option<tokio::sync::watch::Receiver<bool>>) {
    #[cfg(unix)]
    {
        let _ = &shutdown;
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(t) => {
                        t.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        match shutdown {
            Some(mut rx) => {
                if *rx.borrow() {
                    return;
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    changed = rx.changed() => {
                        let _ = changed;
                    }
                }
            }
            None => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
}

/// Run one connected session: send Hello, then serve frames until the link
/// drops or a shutdown signal arrives.
async fn serve(
    target: &Resolved,
    mut conn: fleety_tools::transport::Connection,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    control: Option<&ControlGuard>,
    pending_reconnect: &mut Option<PendingReconnect>,
    reconnect_deadline: Option<tokio::time::Instant>,
) -> Outcome {
    let url = &target.url;
    let pairing_code = std::env::var("FLEETY_PAIRING_CODE")
        .ok()
        .filter(|s| !s.is_empty());
    let expected_reconnect_fingerprint = match pending_reconnect
        .as_ref()
        .map(|pending| reconnect_target_fingerprint(&pending.request.expected_profile, target))
        .transpose()
    {
        Ok(fingerprint) => fingerprint.flatten(),
        Err(error) => {
            decide_pending_reconnect(
                pending_reconnect,
                false,
                format!(
                    "fleetyd could not bind the reconnect to the selected profile: {}",
                    error.report().message
                ),
            );
            return Outcome::Disconnected;
        }
    };

    let registry = ondevice::build_local_registry(&ondevice::device_root());
    // Advertise the on-device tool set so the agent knows what device_exec can
    // invoke here — without this, the server has to guess (or hardcode).
    let local_tools_json = serde_json::to_string(&registry.specs()).ok();

    let hello = match serde_json::to_string(&ClientMsg::Hello {
        device_id: device_id(),
        protocol: PROTOCOL_VERSION,
        token: target.token.clone(),
        pairing_code,
        local_tools_json,
        hostname: fleety_tools::device::hostname(),
    }) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(%e, "could not serialize hello; will reconnect");
            decide_pending_reconnect(
                pending_reconnect,
                false,
                "fleetyd could not prepare the new Server handshake".to_string(),
            );
            return Outcome::Disconnected;
        }
    };
    let hello_result = if let Some(deadline) = reconnect_deadline {
        tokio::select! {
            result = tokio::time::timeout_at(deadline, conn.send_text(hello)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        decide_pending_reconnect(
                            pending_reconnect,
                            false,
                            "fleetyd could not send Hello before the reconnect deadline"
                                .to_string(),
                        );
                        conn.close().await;
                        return Outcome::Disconnected;
                    }
                }
            }
            _ = wait_stop(shutdown.clone()) => {
                decide_pending_reconnect(
                    pending_reconnect,
                    false,
                    "fleetyd stopped before the reconnect completed".to_string(),
                );
                settle_pending_reconnect_before_exit(pending_reconnect).await;
                conn.close().await;
                return Outcome::Shutdown;
            }
        }
    } else {
        tokio::select! {
            result = conn.send_text(hello) => result,
            _ = wait_stop(shutdown.clone()) => {
                conn.close().await;
                return Outcome::Shutdown;
            }
        }
    };
    if let Err(e) = hello_result {
        tracing::warn!(report = ?e.report(), "send hello failed; will reconnect");
        decide_pending_reconnect(
            pending_reconnect,
            false,
            format!(
                "fleetyd could not start the new Server handshake: {}",
                e.report().message
            ),
        );
        return Outcome::Disconnected;
    }
    tracing::info!(
        endpoint = %fleety_tools::transport::redact_endpoint(url),
        "connected; holding connection"
    );
    // Presence: when opted in (`FLEETY_PRESENCE=on`), periodically report this
    // device's co-location fingerprint so the server can infer its site. Absent
    // when disabled — nothing is computed or sent.
    let mut presence_tick = if colocation::presence_enabled() {
        let mut iv =
            tokio::time::interval(std::time::Duration::from_secs(colocation::interval_secs()));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Some(iv)
    } else {
        None
    };
    let mut welcome_deadline = pending_reconnect.as_ref().and(reconnect_deadline);
    loop {
        if pending_reconnect
            .as_ref()
            .is_some_and(|pending| pending.decision.is_some())
        {
            match settle_pending_reconnect(pending_reconnect) {
                Ok(true) => welcome_deadline = None,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(report = ?error.report(), "cannot persist reconnect settlement; will retry");
                    tokio::time::sleep(RECONNECT_POLL).await;
                    continue;
                }
            }
        }
        // Idle frame boundary: carry out a deferred self-restart (auto-update)
        // here so it never interrupts a tool that's mid-execution.
        if restart_due_at_idle() {
            tracing::info!("applying deferred restart now (idle); restarting service");
            decide_pending_reconnect(
                pending_reconnect,
                false,
                "fleetyd restarted before the reconnect completed".to_string(),
            );
            settle_pending_reconnect_before_exit(pending_reconnect).await;
            conn.close().await;
            if let Err(e) = service::restart() {
                tracing::warn!(report = ?e.report(), "could not restart service for update");
            }
            return Outcome::Shutdown;
        }
        let next = tokio::select! {
            frame = conn.recv_text() => Some(frame),
            request = wait_reconnect_request(control) => {
                conn.close().await;
                return Outcome::Reconnect(request);
            }
            _ = async {
                match welcome_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                decide_pending_reconnect(
                    pending_reconnect,
                    false,
                    "fleetyd reached the selected endpoint, but it did not complete Welcome before the handshake deadline"
                        .to_string(),
                );
                conn.close().await;
                return Outcome::Disconnected;
            }
            _ = async {
                match presence_tick.as_mut() {
                    Some(iv) => { iv.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Presence report tick: send best-effort; a send failure just
                // means we retry next tick (or the link drops and we reconnect).
                if let Some(frame) = colocation::report_frame() {
                    if let Err(e) = conn.send_text(frame).await {
                        tracing::warn!(report = ?e.report(), "could not send co-location report");
                    }
                }
                None
            }
            _ = wait_stop(shutdown.clone()) => {
                tracing::info!("stop signal received; closing and shutting down fleetyd");
                decide_pending_reconnect(
                    pending_reconnect,
                    false,
                    "fleetyd stopped before the reconnect completed".to_string(),
                );
                settle_pending_reconnect_before_exit(pending_reconnect).await;
                conn.close().await;
                return Outcome::Shutdown;
            }
        };
        // `None` = a presence tick (handled above); loop for the next event.
        let Some(frame) = next else {
            continue;
        };
        // Inner `None` = link closed or went dead (SSE half-open timeout) → reconnect.
        let Some(text) = frame else {
            decide_pending_reconnect(
                pending_reconnect,
                false,
                "fleetyd reached the selected endpoint, but it closed before Welcome".to_string(),
            );
            return Outcome::Disconnected;
        };
        let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) else {
            continue;
        };
        match msg {
            ServerMsg::Welcome {
                session_id,
                token,
                server_version,
                server_fingerprint,
                ..
            } => {
                if expected_reconnect_fingerprint
                    .as_deref()
                    .is_some_and(|expected| server_fingerprint.as_deref() != Some(expected))
                {
                    decide_pending_reconnect(
                        pending_reconnect,
                        false,
                        "fleetyd reached the selected endpoint, but its Server identity did not match the saved profile"
                            .to_string(),
                    );
                    conn.close().await;
                    return Outcome::Disconnected;
                }
                // Pin (or back-fill) the authenticated endpoint's identity for
                // future mismatch detection; never overwrite a different pin —
                // that is an anomaly worth a warning.
                if let Some(fp) = server_fingerprint.as_deref().filter(|f| !f.is_empty()) {
                    match pin_target_fingerprint(target, fp) {
                        Ok(Some(fleety_tools::connection::PinDecision::IdentityChanged)) => {
                            tracing::warn!(
                                "the server's identity fingerprint changed since it was pinned; \
                                 keeping the old pin — re-pair this device if the server was \
                                 intentionally rebuilt"
                            );
                            decide_pending_reconnect(
                                pending_reconnect,
                                false,
                                "fleetyd reached the selected endpoint, but its Server identity changed"
                                    .to_string(),
                            );
                            conn.close().await;
                            return Outcome::Disconnected;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(report = ?e.report(), "could not pin the server fingerprint");
                            if pending_reconnect.is_some() {
                                decide_pending_reconnect(
                                    pending_reconnect,
                                    false,
                                    "fleetyd authenticated the selected endpoint, but could not persist its Server identity"
                                        .to_string(),
                                );
                                conn.close().await;
                                return Outcome::Disconnected;
                            }
                        }
                    }
                } else if pending_reconnect.is_some() {
                    decide_pending_reconnect(
                        pending_reconnect,
                        false,
                        "fleetyd authenticated the selected endpoint, but it did not provide a Server identity"
                            .to_string(),
                    );
                    conn.close().await;
                    return Outcome::Disconnected;
                }
                if pending_reconnect.is_some() {
                    if let Err(error) = settle_authenticated_reconnect(
                        target,
                        server_fingerprint.as_deref(),
                        pending_reconnect,
                    ) {
                        decide_pending_reconnect(
                            pending_reconnect,
                            false,
                            format!(
                                "fleetyd could not commit the authenticated reconnect result: {}",
                                error.report().message
                            ),
                        );
                        conn.close().await;
                        return Outcome::Disconnected;
                    }
                }
                if let Some(tok) = token {
                    match persist_token(target, &tok) {
                        Ok(true) => tracing::info!(
                            "fleetyd token persisted to the owning profile in connections.toml"
                        ),
                        Ok(false) => tracing::info!(
                            "fleetyd token belongs to an ephemeral target; not persisting it"
                        ),
                        Err(e) => {
                            tracing::warn!(report = ?e.report(), "could not persist fleetyd token")
                        }
                    }
                }
                tracing::info!(%session_id, "registered with agent");
                // Forward-only fleet convergence: match this host to the server's
                // version when the server is newer (so a device that was offline
                // during a `fleety update` catches up on reconnect).
                converge_to_server_version(&server_version).await;
            }
            ServerMsg::Error { ref error } if error.kind == "unauthenticated" => {
                tracing::warn!(
                    "server rejected our token: {} — clearing saved token so the next \
                     connect can re-pair",
                    error.message
                );
                if let Err(e) = clear_target_token(target) {
                    tracing::warn!(report = ?e.report(), "could not clear the rejected token");
                }
                decide_pending_reconnect(
                    pending_reconnect,
                    false,
                    format!(
                        "fleetyd could not authenticate the selected profile: {}",
                        error.message
                    ),
                );
                return Outcome::Disconnected;
            }
            ServerMsg::RunTool {
                call_id,
                tool,
                args_json,
            } => {
                let args: serde_json::Value =
                    serde_json::from_str(&args_json).unwrap_or_else(|_| serde_json::json!({}));
                tracing::info!(%tool, "running on-device tool");
                // The tool runs inline: this loop stops reading the socket, so
                // the WebSocket layer stops answering the server's keepalive
                // pings (ws-liveness). A tool that blocks past the liveness
                // deadline gets this connection reclaimed by the server — by
                // then the call has already failed device_exec's per-call
                // timeout; the tool still completes here (side effects intact),
                // the reply send below fails, and we reconnect via the normal
                // backoff path.
                let outcome = match internal_config(&tool, &args) {
                    Some(result) => result,
                    None => registry.call(&tool, args).await,
                };
                let reply = match outcome {
                    Ok(value) => ClientMsg::ToolResult {
                        call_id,
                        result_json: value.to_string(),
                    },
                    Err(e) => {
                        let r = e.report();
                        ClientMsg::ToolError {
                            call_id,
                            error: WireError {
                                kind: r.kind,
                                message: r.message,
                                remediation: r.remediation,
                            },
                        }
                    }
                };
                let out = match serde_json::to_string(&reply) {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::warn!(%e, "could not serialize tool reply; dropping");
                        continue;
                    }
                };
                if let Err(e) = conn.send_text(out).await {
                    tracing::warn!(report = ?e.report(), "send tool reply failed; will reconnect");
                    return Outcome::Disconnected;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
        temp_home: PathBuf,
    }

    impl EnvGuard {
        fn new(name: &str) -> Self {
            let keys = [
                "HOME",
                "USERPROFILE",
                "FLEETY_AGENT_URL",
                "FLEETY_DEVICE_ID",
                "FLEETY_TOKEN",
                "FLEETY_MDNS_DISABLED",
                "FLEETY_CONNECTIONS",
                "FLEETY_CONFIG",
                "COMPUTERNAME",
                "HOSTNAME",
            ];
            let saved = keys
                .into_iter()
                .map(|key| (key, std::env::var(key).ok()))
                .collect::<Vec<_>>();
            let temp_home =
                std::env::temp_dir().join(format!("fleetyd-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&temp_home);
            std::fs::create_dir_all(&temp_home).expect("temp home");

            std::env::set_var("HOME", &temp_home);
            std::env::set_var("USERPROFILE", &temp_home);
            for key in [
                "FLEETY_AGENT_URL",
                "FLEETY_DEVICE_ID",
                "FLEETY_TOKEN",
                "FLEETY_MDNS_DISABLED",
                "FLEETY_CONNECTIONS",
                "FLEETY_CONFIG",
                "COMPUTERNAME",
                "HOSTNAME",
            ] {
                std::env::remove_var(key);
            }

            Self { saved, temp_home }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            let _ = std::fs::remove_dir_all(&self.temp_home);
        }
    }

    #[test]
    fn persist_and_clear_token_on_current_profile() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("token");

        // With no profile yet (a pure-env deployment), persist_token creates a
        // `default` profile pointing at the connected url and stores the token.
        let target = Resolved {
            url: "ws://srv:8787".to_string(),
            token: None,
            source: Source::Env,
        };
        assert!(persist_token(&target, "daemon-tok").expect("persist"));
        let conns = connection::load().expect("load");
        assert_eq!(conns.current.as_deref(), Some("default"));
        let p = conns.current_profile().expect("profile");
        assert_eq!(p.url, "ws://srv:8787");
        assert_eq!(p.token.as_deref(), Some("daemon-tok"));

        // Clearing after a rejection drops the token but keeps the profile.
        let reconnect = Resolved {
            url: "ws://srv:8787".to_string(),
            token: Some("daemon-tok".to_string()),
            source: Source::Profile("default".to_string()),
        };
        assert!(clear_target_token(&reconnect).expect("clear"));
        let conns = connection::load().expect("reload");
        assert!(conns
            .current_profile()
            .and_then(|p| p.token.as_deref())
            .is_none());
    }

    #[test]
    fn saved_profile_connect_failure_directs_explicit_repair_without_leaking_url_secrets() {
        let target = Resolved {
            url: "wss://host.test".into(),
            token: Some("stored-token".into()),
            source: Source::Profile("office".into()),
        };
        let message = reconnect_connect_error(
            &target,
            "connect wss://user:pass@host.test/x?token=SECRET#tail failed",
        );
        for secret in ["pass", "SECRET", "#tail", "stored-token"] {
            assert!(!message.contains(secret), "leaked {secret}: {message}");
        }
        assert!(message.contains("--pairing-code <code>"), "{message}");
        assert!(
            message.contains("will not send the stored token"),
            "{message}"
        );
    }

    #[test]
    fn different_env_target_never_mutates_current_profile_a() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("env-provenance");
        let mut conns = connection::Connections {
            current: Some("a".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "a".to_string(),
            connection::Profile {
                url: "ws://server-a:8787".to_string(),
                token: Some("token-a".to_string()),
                label: None,
                fingerprint: Some("fp-a".to_string()),
            },
        );
        connection::save(&conns).expect("save A");
        let target = Resolved {
            url: "ws://server-b:8787".to_string(),
            token: None,
            source: Source::Env,
        };

        assert!(!persist_token(&target, "token-b").expect("skip persist"));
        assert!(!clear_target_token(&target).expect("skip clear"));
        assert_eq!(
            pin_target_fingerprint(&target, "fp-b").expect("skip pin"),
            None
        );

        let after = connection::load().expect("reload A");
        assert_eq!(after.current.as_deref(), Some("a"));
        let a = &after.profiles["a"];
        assert_eq!(a.url, "ws://server-a:8787");
        assert_eq!(a.token.as_deref(), Some("token-a"));
        assert_eq!(a.fingerprint.as_deref(), Some("fp-a"));
        assert!(!after.profiles.contains_key("default"));
    }

    #[test]
    fn unowned_mdns_target_never_mutates_current_profile_a_or_saved_b() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("mdns-provenance");
        let mut conns = connection::Connections {
            current: Some("a".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "a".to_string(),
            connection::Profile {
                url: String::new(),
                token: Some("token-a".to_string()),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "b".to_string(),
            connection::Profile {
                url: String::new(),
                token: Some("token-b".to_string()),
                fingerprint: Some("fp-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save A/B");
        let target = Resolved {
            url: "ws://server-b:8787".to_string(),
            token: None,
            source: Source::Mdns,
        };
        assert_eq!(target.source, Source::Mdns);
        assert_eq!(target.token, None);

        assert!(!persist_token(&target, "minted").expect("skip persist"));
        assert!(!clear_target_token(&target).expect("skip clear"));
        assert_eq!(
            pin_target_fingerprint(&target, "fp-b").expect("skip pin"),
            None
        );
        let after = connection::load().expect("reload A/B");
        assert_eq!(after.profiles["a"].token.as_deref(), Some("token-a"));
        assert_eq!(after.profiles["a"].fingerprint, None);
        assert_eq!(after.profiles["b"].token.as_deref(), Some("token-b"));
        assert_eq!(after.profiles["b"].fingerprint.as_deref(), Some("fp-b"));

        // A credentialed current profile no longer enters automatic discovery
        // on reconnect, even if copied TXT metadata matches another profile.
        let error = connection::resolve(&after, &Target::Current, None, None, || {
            Some(connection::Discovered {
                url: "ws://server-b:8787".to_string(),
                fingerprint: Some("fp-b".to_string()),
            })
        })
        .expect_err("credentialed profile requires explicit recovery");
        assert!(error.report().message.contains("--pairing-code <code>"));
    }

    #[test]
    fn synthetic_mdns_profile_provenance_cannot_mutate_a_urlless_saved_profile() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("mdns-owned-provenance");
        let mut conns = connection::Connections {
            current: Some("a".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "a".to_string(),
            connection::Profile {
                url: String::new(),
                token: Some("old-a".to_string()),
                fingerprint: Some("fp-a".to_string()),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "b".to_string(),
            connection::Profile {
                url: String::new(),
                token: Some("token-b".to_string()),
                fingerprint: Some("fp-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save A/B");
        let target = Resolved {
            url: "ws://server-a:8787".to_string(),
            token: Some("old-a".to_string()),
            source: Source::Profile("a".to_string()),
        };

        assert!(!persist_token(&target, "new-a").expect("reject synthetic owner"));
        let after = connection::load().expect("reload A/B");
        assert!(after.profiles["a"].url.is_empty());
        assert_eq!(after.profiles["a"].token.as_deref(), Some("old-a"));
        assert_eq!(after.profiles["b"].token.as_deref(), Some("token-b"));
        assert_eq!(after.profiles["b"].fingerprint.as_deref(), Some("fp-b"));
    }

    #[test]
    fn rejected_explicit_env_token_does_not_clear_saved_profile_token() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("explicit-token-provenance");
        let mut conns = connection::Connections {
            current: Some("a".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "a".to_string(),
            connection::Profile {
                url: "ws://server-a:8787".to_string(),
                token: Some("saved-a".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save A");
        let target = Resolved {
            url: "ws://server-a:8787".to_string(),
            token: Some("explicit-env".to_string()),
            source: Source::Env,
        };

        assert!(!clear_target_token(&target).expect("skip clear"));
        assert_eq!(
            connection::load().expect("reload").profiles["a"]
                .token
                .as_deref(),
            Some("saved-a")
        );
    }

    #[test]
    fn occupied_default_is_never_repurposed_for_an_unowned_target() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("occupied-default");
        let mut conns = connection::Connections::default();
        conns.profiles.insert(
            "default".to_string(),
            connection::Profile {
                url: "ws://server-a:8787".to_string(),
                token: Some("token-a".to_string()),
                fingerprint: Some("fp-a".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save occupied default");

        for (source, url) in [
            (Source::Env, "ws://server-b:8787"),
            (Source::Mdns, "ws://server-b:8787"),
            (Source::Default, connection::DEFAULT_URL),
        ] {
            let target = Resolved {
                url: url.to_string(),
                token: None,
                source,
            };
            assert!(!persist_token(&target, "token-b").expect("skip persist"));
        }

        let after = connection::load().expect("reload default");
        assert!(after.current.is_none());
        let default = &after.profiles["default"];
        assert_eq!(default.url, "ws://server-a:8787");
        assert_eq!(default.token.as_deref(), Some("token-a"));
        assert_eq!(default.fingerprint.as_deref(), Some("fp-a"));
    }

    #[test]
    fn urlless_occupied_default_never_accepts_a_discovered_token() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("urlless-occupied-default");
        let mut conns = connection::Connections::default();
        conns.profiles.insert(
            "default".to_string(),
            connection::Profile {
                url: String::new(),
                token: Some("legacy-token".to_string()),
                fingerprint: Some("legacy-pin".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save URL-less default");
        let target = Resolved {
            url: "ws://rogue-advertiser:8787".to_string(),
            token: None,
            source: Source::Mdns,
        };

        assert!(!persist_token(&target, "rogue-token").expect("skip persist"));

        let after = connection::load().expect("reload default");
        assert!(after.current.is_none());
        let default = &after.profiles["default"];
        assert!(default.url.is_empty());
        assert_eq!(default.token.as_deref(), Some("legacy-token"));
        assert_eq!(default.fingerprint.as_deref(), Some("legacy-pin"));
    }

    #[test]
    fn resolve_target_prefers_env_then_current_then_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("resolve");
        std::env::set_var("FLEETY_MDNS_DISABLED", "1"); // no LAN probe in the test

        // Nothing configured → the localhost default.
        assert_eq!(
            resolve_target().expect("resolve").url,
            connection::DEFAULT_URL
        );

        // No env + a current profile (a paired device) → its url + token.
        let target = Resolved {
            url: "ws://srv:8787".to_string(),
            token: None,
            source: Source::Env,
        };
        persist_token(&target, "tok").expect("persist");
        let r = resolve_target().expect("resolve");
        assert_eq!(r.url, "ws://srv:8787");
        assert_eq!(r.token.as_deref(), Some("tok"));

        // An old env deployment still connects: FLEETY_AGENT_URL overrides.
        std::env::set_var("FLEETY_AGENT_URL", "ws://env:8787");
        assert_eq!(resolve_target().expect("resolve").url, "ws://env:8787");
    }

    #[test]
    fn daemon_internal_config_is_owner_scoped_and_transactional() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let guard = EnvGuard::new("internal-config");
        let path = guard.temp_home.join("config.toml");
        std::env::set_var("FLEETY_CONFIG", &path);

        let set = internal_config(
            INTERNAL_CONFIG_TOOL,
            &serde_json::json!({
                "op": "exec",
                "args": ["set", "FLEETY_PRESENCE", "off"]
            }),
        )
        .expect("reserved tool")
        .expect("reply");
        assert_eq!(
            set.get("ok").and_then(serde_json::Value::as_bool),
            Some(true)
        );

        let foreign = internal_config(
            INTERNAL_CONFIG_TOOL,
            &serde_json::json!({
                "op": "exec",
                "args": ["set", "FLEETY_ADDR", "127.0.0.1:8787"]
            }),
        )
        .expect("reserved tool")
        .expect("reply");
        assert_eq!(
            foreign.get("ok").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert!(fleety_tools::config::load_strict(&path)
            .unwrap()
            .contains_key(&(
                fleety_tools::config::Scope::Daemon,
                "FLEETY_PRESENCE".to_string()
            )));

        let snapshot = internal_config(
            INTERNAL_CONFIG_TOOL,
            &serde_json::json!({ "op": "snapshot" }),
        )
        .expect("reserved tool")
        .expect("snapshot");
        let entries: Vec<ConfigEntry> =
            serde_json::from_value(snapshot["entries"].clone()).expect("entries");
        assert!(entries
            .iter()
            .all(|entry| matches!(entry.scope.as_str(), "daemon" | "shared")));
        assert!(!entries.iter().any(|entry| entry.key == "FLEETY_ADDR"));

        let stale = internal_config(
            INTERNAL_CONFIG_TOOL,
            &serde_json::json!({
                "op": "apply",
                "base_revision": "stale",
                "changes": [{"key":"FLEETY_TZ","op":"set","value":"UTC"}]
            }),
        )
        .expect("reserved tool")
        .expect("reply");
        assert_eq!(
            stale.get("kind").and_then(serde_json::Value::as_str),
            Some("conflict")
        );
        assert!(!fleety_tools::config::load_strict(&path)
            .unwrap()
            .contains_key(&(fleety_tools::config::Scope::Shared, "FLEETY_TZ".to_string())));
    }

    #[test]
    fn device_id_is_stable_and_nonempty() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("device-id");

        let first = device_id();
        assert!(!first.is_empty());
        assert_eq!(device_id(), first);
    }

    #[test]
    fn reconnect_timeout_keeps_original_nonce_and_duplicate_cannot_replace_it() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-timeout-duplicate");
        let journal = reconnect_journal_path();
        let ready = ControlReady {
            pid: std::process::id(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("publish test control owner");

        let timeout = request_running_daemon_reconnect_with_wait("B", std::time::Duration::ZERO)
            .expect_err("caller should stop waiting without consuming the request");
        assert!(timeout.report().message.contains("remains durable"));
        let before = std::fs::read(&journal).expect("journal before duplicate");
        let state = load_reconnect_journal_at(&journal)
            .expect("load journal")
            .expect("active journal");
        let original_nonce = state.request.nonce.clone();
        let error = request_running_daemon_reconnect_with_wait("C", std::time::Duration::ZERO)
            .expect_err("an unsettled request must reject r2");

        assert!(error.report().message.contains(&original_nonce));
        assert_eq!(
            std::fs::read(&journal).expect("journal after duplicate"),
            before,
            "duplicate submission must not replace or append after r1"
        );
        let state = load_reconnect_journal_at(&journal)
            .expect("load journal")
            .expect("active journal");
        assert_eq!(state.request.nonce, original_nonce);
        assert!(matches!(state.phase, ReconnectPhase::Submitted));
    }

    #[test]
    fn reconnect_claim_is_durable_until_terminal_settlement() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-delayed-consume");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");

        let claimed = claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");

        assert_eq!(claimed.nonce, "r1");
        assert!(
            journal.exists(),
            "consume must not delete the durable request"
        );
        let state = load_reconnect_journal_at(&journal)
            .expect("load journal")
            .expect("active journal");
        assert_eq!(state.request.nonce, "r1");
        assert!(matches!(state.phase, ReconnectPhase::Claimed));
    }

    #[test]
    fn reconnect_journal_recovers_only_a_torn_final_record() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-torn-tail");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .expect("open journal")
            .write_all(br#"{"event":"settled","nonce":"nonce-a""#)
            .expect("write torn tail");

        let state = load_reconnect_journal_at(&journal)
            .expect("ignore torn final record")
            .expect("submitted request");
        assert!(matches!(state.phase, ReconnectPhase::Submitted));

        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Claimed {
                nonce: "nonce-a".to_string(),
            },
        )
        .expect("repair tail and append");
        let state = load_reconnect_journal_at(&journal)
            .expect("load repaired journal")
            .expect("request");
        assert!(matches!(state.phase, ReconnectPhase::Claimed));
    }

    #[test]
    fn reconnect_lease_drop_never_deletes_a_successor_lock() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-lease-owner");
        let path = reconnect_lock_path();
        std::fs::create_dir_all(path.parent().expect("control directory"))
            .expect("create control directory");
        std::fs::write(&path, "successor-owner").expect("publish successor");

        drop(ReconnectLease {
            path: path.clone(),
            owner: "previous-owner".to_string(),
        });

        assert_eq!(
            std::fs::read_to_string(path).expect("successor lock remains"),
            "successor-owner"
        );
    }

    #[test]
    fn later_profile_request_cannot_mistake_an_older_settlement_for_its_own() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-settlement-owner");
        let journal = reconnect_journal_path();
        let ready = ControlReady {
            pid: std::process::id(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("publish test control owner");
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit B");
        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Settled {
                ack: ReconnectAck {
                    nonce: "r1".to_string(),
                    accepted: true,
                    message: "B connected".to_string(),
                },
            },
        )
        .expect("settle B");

        let error = request_running_daemon_reconnect_with_wait("C", std::time::Duration::ZERO)
            .expect_err("C must not inherit B's success");

        assert!(error.report().message.contains("profile 'B'"));
        assert!(error
            .report()
            .message
            .contains("profile 'C' was not submitted"));
        assert!(!journal.exists(), "observed settlement should be reaped");
    }

    #[test]
    fn reconnect_settlement_write_failure_keeps_the_frozen_decision_for_retry() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-settlement-retry");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        let request = claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let mut pending = Some(PendingReconnect::new(request));
        decide_pending_reconnect(
            &mut pending,
            false,
            "fleetyd stopped before reconnect completed".to_string(),
        );

        let first = settle_pending_reconnect_at_with(&journal, &mut pending, |_, _| {
            Err(agent_core::CoreError::Message(
                "injected settlement write failure".to_string(),
            ))
        });
        assert!(first.is_err());
        let frozen = pending
            .as_ref()
            .and_then(|pending| pending.decision.as_ref())
            .expect("decision remains retryable");
        assert!(!frozen.accepted);
        assert!(frozen.message.contains("stopped"));

        settle_pending_reconnect_at_with(&journal, &mut pending, append_reconnect_event_at)
            .expect("retry settlement");
        assert!(pending.is_none());
        let state = load_reconnect_journal_at(&journal)
            .expect("load settled journal")
            .expect("journal");
        assert!(matches!(
            state.phase,
            ReconnectPhase::Settled(ReconnectAck {
                accepted: false,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn graceful_exit_retries_a_failed_reconnect_settlement() {
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        let mut pending = Some(PendingReconnect::new(request));
        decide_pending_reconnect(
            &mut pending,
            false,
            "fleetyd stopped before reconnect completed".to_string(),
        );
        let mut attempts = 0;

        settle_pending_reconnect_before_exit_with(&mut pending, |pending| {
            attempts += 1;
            if attempts == 1 {
                return Err(agent_core::CoreError::Message(
                    "injected shutdown settlement failure".to_string(),
                ));
            }
            pending.take();
            Ok(true)
        })
        .await;

        assert_eq!(attempts, 2);
        assert!(pending.is_none());
    }

    #[test]
    fn reconnect_settlement_stays_complete_when_caller_reaps_the_observed_journal() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-settlement-reaped");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        let request = claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let mut pending = Some(PendingReconnect::new(request));
        decide_pending_reconnect(&mut pending, true, "connected".to_string());

        let settled = settle_pending_reconnect_at_with(&journal, &mut pending, |path, event| {
            append_reconnect_event_at(path, event)?;
            std::fs::remove_file(path).expect("caller reaps observed settlement");
            Ok(())
        });

        assert!(settled.expect("durable settlement"));
        assert!(
            pending.is_none(),
            "a caller that already observed and reaped the result must not revive it"
        );
    }

    #[test]
    fn reconnect_restart_settles_the_previous_instance_once() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-restart");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-old".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-old")
            .expect("claim")
            .expect("request");

        recover_reconnect_for_instance_at(&journal, "daemon-new").expect("recover");
        recover_reconnect_for_instance_at(&journal, "daemon-new").expect("idempotent recover");

        let state = load_reconnect_journal_at(&journal)
            .expect("load journal")
            .expect("journal");
        assert_eq!(state.request.nonce, "r1");
        assert!(matches!(
            state.phase,
            ReconnectPhase::Settled(ReconnectAck {
                accepted: false,
                ref message,
                ..
            }) if message.contains("restarted")
        ));
    }

    #[test]
    fn reconnect_owner_binding_rejects_target_drift_before_terminal_success() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-owner-binding");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("token-b".to_string()),
                fingerprint: Some("fingerprint-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save B");
        let target = Resolved {
            url: "ws://server-b:8787".to_string(),
            token: Some("token-b".to_string()),
            source: Source::Profile("B".to_string()),
        };

        assert_eq!(
            reconnect_target_fingerprint("B", &target)
                .expect("owner binding")
                .as_deref(),
            Some("fingerprint-b")
        );
        connection::mutate(|live| {
            live.profiles.get_mut("B").expect("profile B").url = "ws://server-c:8787".to_string();
            Ok(())
        })
        .expect("drift target");
        assert!(reconnect_target_fingerprint("B", &target).is_err());
    }

    #[test]
    fn authenticated_reconnect_write_failure_never_freezes_uncommitted_success() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-authenticated-write-failure");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("token-b".to_string()),
                fingerprint: Some("fingerprint-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save B");
        let target = Resolved {
            url: "ws://server-b:8787".to_string(),
            token: Some("token-b".to_string()),
            source: Source::Profile("B".to_string()),
        };
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        let request = claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let mut pending = Some(PendingReconnect::new(request));

        let error = settle_authenticated_reconnect_with(
            &target,
            Some("fingerprint-b"),
            &mut pending,
            |_, _| {
                Err(agent_core::CoreError::Message(
                    "injected settlement failure".to_string(),
                ))
            },
        )
        .expect_err("first durable write fails");
        assert!(error.report().message.contains("injected"));
        assert!(
            pending
                .as_ref()
                .is_some_and(|pending| pending.decision.is_none()),
            "a success that was not durably committed is not terminal"
        );
        decide_pending_reconnect(&mut pending, false, "later failure".to_string());
        let decision = pending
            .as_ref()
            .and_then(|pending| pending.decision.as_ref())
            .expect("frozen decision");
        assert!(!decision.accepted);
        assert_eq!(decision.message, "later failure");
    }
}
