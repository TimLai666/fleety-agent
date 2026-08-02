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

const RECONNECT_SWEEP_BUDGET_MILLIS: u64 = 4_500;
const RECONNECT_SETTLEMENT_MARGIN_MILLIS: u64 = 500;
const RECONNECT_ACK_WAIT: std::time::Duration = std::time::Duration::from_millis(
    RECONNECT_SWEEP_BUDGET_MILLIS + RECONNECT_SETTLEMENT_MARGIN_MILLIS,
);
const RECONNECT_POLL: std::time::Duration = std::time::Duration::from_millis(50);
/// Terminal receipts and authenticated success proofs remain queryable for
/// one day after observation. Cleanup is explicit and never runs for active
/// requests.
const RECONNECT_TERMINAL_RETENTION: std::time::Duration = std::time::Duration::from_secs(86_400);
const RECONNECT_HOUSEKEEPING_RETRIES: usize = 3;
/// Total time an owner-requested reconnect may spend across every endpoint.
///
/// It is the whole sweep, not one attempt, and it stays well under
/// [`RECONNECT_ACK_WAIT`]: a sweep that outlives the caller's wait leaves a
/// durable request the caller has already given up on, and the next one is
/// refused until that settles. A profile with several endpoints therefore gives
/// each a share — deliberately tight, because someone is waiting. The ordinary
/// (non-reconnect) path uses [`CONNECT_ENDPOINT_WAIT`] instead.
const RECONNECT_SWEEP_BUDGET: std::time::Duration =
    std::time::Duration::from_millis(RECONNECT_SWEEP_BUDGET_MILLIS);
/// Budget for opening one endpoint outside an owner-requested reconnect. The
/// reconnect budget is deliberately tighter because a caller is waiting on it;
/// an ordinary connect can afford a slow link, a TLS handshake, and Noise.
const CONNECT_ENDPOINT_WAIT: std::time::Duration = std::time::Duration::from_secs(15);
const RECONNECT_CONTROL_VERSION: u64 = 1;

fn control_nonce() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}-{sequence}", std::process::id())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlReady {
    pid: u32,
    process_start: String,
    instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlReadyRecord {
    Current(ControlReady),
    Legacy { pid: u32, instance: String },
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
enum ReconnectLifecycle {
    Submitted,
    InProgress,
    Settled,
    Cancelled,
    Superseded,
    Expired,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectStatus {
    nonce: String,
    profile: String,
    owner: String,
    state: ReconnectLifecycle,
    submitted_at: u64,
    claimed_at: Option<u64>,
    terminal_at: Option<u64>,
    retention_expires_at: Option<u64>,
    replacement_nonce: Option<String>,
    terminal_result: Option<ReconnectAck>,
    detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReconnectJournalEvent {
    Submitted {
        request: ReconnectRequest,
    },
    Claimed {
        nonce: String,
    },
    InProgress {
        nonce: String,
    },
    Settled {
        ack: ReconnectAck,
    },
    Cancelled {
        ack: ReconnectAck,
    },
    Superseded {
        ack: ReconnectAck,
        replacement_nonce: String,
    },
    Expired {
        ack: ReconnectAck,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReconnectPhase {
    Submitted,
    Claimed,
    Settled(ReconnectAck),
    Cancelled(ReconnectAck),
    Superseded {
        ack: ReconnectAck,
        replacement_nonce: String,
    },
    Expired(ReconnectAck),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReconnectJournalState {
    request: ReconnectRequest,
    phase: ReconnectPhase,
    submitted_at: u64,
    claimed_at: Option<u64>,
    terminal_at: Option<u64>,
}

#[derive(Debug, Clone)]
struct PendingReconnect {
    request: ReconnectRequest,
    decision: Option<ReconnectAck>,
    authenticated: Option<AuthenticatedReconnect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthenticatedReconnect {
    target: Resolved,
    server_fingerprint: String,
}

impl PendingReconnect {
    fn new(request: ReconnectRequest) -> Self {
        Self {
            request,
            decision: None,
            authenticated: None,
        }
    }
}

fn encode_ready(ready: &ControlReady) -> Vec<u8> {
    serde_json::json!({
        "version": RECONNECT_CONTROL_VERSION,
        "pid": ready.pid,
        "process_start": ready.process_start,
        "instance": ready.instance
    })
    .to_string()
    .into_bytes()
}

fn parse_ready_record(bytes: &[u8]) -> Result<ControlReadyRecord> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        agent_core::CoreError::Message(format!(
            "fleetyd reconnect control identity is unreadable: {error}; restart fleetyd, then retry"
        ))
    })?;
    let version = value.get("version");
    if let Some(version) = version {
        if version.as_u64() != Some(RECONNECT_CONTROL_VERSION) {
            return Err(agent_core::CoreError::Message(format!(
                "fleetyd reconnect control version {} is incompatible with this binary (supported: {}); update fleetyd and the Fleety CLI together, restart fleetyd, then retry",
                version,
                RECONNECT_CONTROL_VERSION
            )));
        }
    }
    let pid = value
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| {
            agent_core::CoreError::Message(
                "fleetyd reconnect control identity has no valid pid; restart fleetyd, then retry"
                    .to_string(),
            )
        })?;
    let instance = value
        .get("instance")
        .and_then(serde_json::Value::as_str)
        .filter(|instance| !instance.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            agent_core::CoreError::Message(
                "fleetyd reconnect control identity has no instance; restart fleetyd, then retry"
                    .to_string(),
            )
        })?;
    match version {
        None => Ok(ControlReadyRecord::Legacy { pid, instance }),
        Some(_) => {
            let process_start = value
                .get("process_start")
                .and_then(serde_json::Value::as_str)
                .filter(|identity| !identity.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    agent_core::CoreError::Message(
                        "fleetyd reconnect control identity has no process-start proof; update or restart fleetyd, then retry"
                            .to_string(),
                    )
                })?;
            Ok(ControlReadyRecord::Current(ControlReady {
                pid,
                process_start,
                instance,
            }))
        }
    }
}

fn parse_ready(bytes: &[u8]) -> Option<ControlReady> {
    match parse_ready_record(bytes).ok()? {
        ControlReadyRecord::Current(ready) => Some(ready),
        ControlReadyRecord::Legacy { .. } => None,
    }
}

fn publish_ready_at_with<W, P, F, S, C>(
    path: &std::path::Path,
    ready: &ControlReady,
    mut stage: W,
    mut publish: P,
    mut flush_published: F,
    mut sync_directory: S,
    mut cleanup: C,
) -> Result<()>
where
    W: FnMut(&std::path::Path, &[u8]) -> Result<()>,
    P: FnMut(&std::path::Path, &std::path::Path) -> Result<()>,
    F: FnMut(&std::path::Path) -> Result<()>,
    S: FnMut(&std::path::Path) -> Result<()>,
    C: FnMut(&std::path::Path) -> Result<()>,
{
    let parent = path.parent().ok_or_else(|| {
        agent_core::CoreError::Message(
            "daemon reconnect ready path has no control directory".to_string(),
        )
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        agent_core::CoreError::Message(format!(
            "cannot create daemon control directory {}: {error}",
            parent.display()
        ))
    })?;
    if path.exists() {
        return Err(agent_core::CoreError::Message(format!(
            "cannot publish daemon reconnect identity because {} already exists",
            path.display()
        )));
    }
    let temp = parent.join(format!(".fleetyd.control-ready.{}.tmp", control_nonce()));
    if let Err(error) = stage(&temp, &encode_ready(ready)) {
        let _ = cleanup(&temp);
        return Err(error);
    }
    if let Err(error) = publish(&temp, path) {
        let _ = cleanup(&temp);
        return Err(error);
    }
    let durability_error = flush_published(path)
        .err()
        .or_else(|| sync_directory(parent).err());
    if let Some(error) = durability_error {
        loop {
            match cleanup(path) {
                Ok(()) => break,
                Err(cleanup_error) => {
                    tracing::warn!(
                        ready = %path.display(),
                        report = ?cleanup_error.report(),
                        "cannot hide ambiguous daemon reconnect identity; retaining ownership"
                    );
                    std::thread::sleep(RECONNECT_POLL);
                }
            }
        }
        while let Err(sync_error) = sync_directory(parent) {
            tracing::warn!(
                directory = %parent.display(),
                report = ?sync_error.report(),
                "daemon reconnect identity absence is not durable; retaining ownership"
            );
            std::thread::sleep(RECONNECT_POLL);
        }
        return Err(error);
    }
    Ok(())
}

fn sync_ready_directory_at(parent: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                agent_core::CoreError::Message(format!(
                    "cannot make daemon reconnect identity directory durable {}: {error}",
                    parent.display()
                ))
            })?;
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

fn cleanup_ready_artifact_at(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(agent_core::CoreError::Message(format!(
            "cannot remove daemon reconnect identity artifact {}: {error}",
            path.display()
        ))),
    }
}

fn publish_ready_at(path: &std::path::Path, ready: &ControlReady) -> Result<()> {
    publish_ready_at_with(
        path,
        ready,
        |temp, bytes| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(temp)
                .map_err(|error| {
                    agent_core::CoreError::Message(format!(
                        "cannot stage daemon reconnect identity {}: {error}",
                        temp.display()
                    ))
                })?;
            use std::io::Write;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    agent_core::CoreError::Message(format!(
                        "cannot make staged daemon reconnect identity durable {}: {error}",
                        temp.display()
                    ))
                })
        },
        |temp, target| {
            std::fs::rename(temp, target).map_err(|error| {
                agent_core::CoreError::Message(format!(
                    "cannot publish daemon reconnect identity {}: {error}",
                    target.display()
                ))
            })
        },
        |target| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(target)
                .and_then(|file| file.sync_all())
                .map_err(|error| {
                    agent_core::CoreError::Message(format!(
                        "cannot flush published daemon reconnect identity {}: {error}",
                        target.display()
                    ))
                })
        },
        sync_ready_directory_at,
        cleanup_ready_artifact_at,
    )
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

fn process_identity_path(process_start: &str) -> std::path::PathBuf {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(process_start.len().saturating_mul(2));
    for byte in process_start.as_bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    control_path(&format!("fleetyd.process-start-{encoded}.lock"))
}

fn reconnect_journal_path() -> std::path::PathBuf {
    control_path("fleetyd.reconnect-journal.jsonl")
}

fn reconnect_nonce_record_path_at(
    journal: &std::path::Path,
    directory: &str,
    nonce: &str,
) -> Result<std::path::PathBuf> {
    let parent = journal
        .parent()
        .ok_or_else(|| reconnect_journal_error("reconnect journal has no control directory"))?;
    let mut encoded = String::with_capacity(nonce.len().saturating_mul(2));
    use std::fmt::Write as _;
    for byte in nonce.as_bytes() {
        write!(&mut encoded, "{byte:02x}")
            .map_err(|error| reconnect_journal_error(format!("encode receipt nonce: {error}")))?;
    }
    Ok(parent.join(directory).join(format!("{encoded}.json")))
}

fn reconnect_receipt_path_at(journal: &std::path::Path, nonce: &str) -> Result<std::path::PathBuf> {
    reconnect_nonce_record_path_at(journal, "fleetyd.reconnect-receipts", nonce)
}

fn reconnect_success_proof_path_at(
    journal: &std::path::Path,
    nonce: &str,
) -> Result<std::path::PathBuf> {
    reconnect_nonce_record_path_at(journal, "fleetyd.reconnect-success", nonce)
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
    if let Some(version) = value.get("version") {
        if version.as_u64() != Some(RECONNECT_CONTROL_VERSION) {
            return Err(reconnect_journal_error(format!(
                "control version {version} is incompatible (supported: {RECONNECT_CONTROL_VERSION}); update fleetyd and the Fleety CLI together"
            )));
        }
    }
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
        "in_progress" => Ok(ReconnectJournalEvent::InProgress { nonce: nonce()? }),
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
        "cancelled" => Ok(ReconnectJournalEvent::Cancelled {
            ack: ReconnectAck {
                nonce: nonce()?,
                accepted: false,
                message: value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| reconnect_journal_error("cancellation message is missing"))?,
            },
        }),
        "superseded" => Ok(ReconnectJournalEvent::Superseded {
            ack: ReconnectAck {
                nonce: nonce()?,
                accepted: false,
                message: value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| reconnect_journal_error("supersession message is missing"))?,
            },
            replacement_nonce: value
                .get("replacement_nonce")
                .and_then(serde_json::Value::as_str)
                .filter(|nonce| !nonce.is_empty())
                .map(str::to_string)
                .ok_or_else(|| reconnect_journal_error("replacement nonce is missing"))?,
        }),
        "expired" => Ok(ReconnectJournalEvent::Expired {
            ack: ReconnectAck {
                nonce: nonce()?,
                accepted: false,
                message: value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| reconnect_journal_error("expiration message is missing"))?,
            },
        }),
        other => Err(reconnect_journal_error(format!(
            "unknown event kind '{other}'"
        ))),
    }
}

fn reconnect_event_value(event: &ReconnectJournalEvent) -> serde_json::Value {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    match event {
        ReconnectJournalEvent::Submitted { request } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "submitted",
            "timestamp": timestamp,
            "instance": request.instance,
            "nonce": request.nonce,
            "expected_profile": request.expected_profile,
        }),
        ReconnectJournalEvent::Claimed { nonce } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "claimed",
            "timestamp": timestamp,
            "nonce": nonce,
        }),
        ReconnectJournalEvent::InProgress { nonce } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "in_progress",
            "timestamp": timestamp,
            "nonce": nonce,
        }),
        ReconnectJournalEvent::Settled { ack } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "settled",
            "timestamp": timestamp,
            "nonce": ack.nonce,
            "accepted": ack.accepted,
            "message": ack.message,
        }),
        ReconnectJournalEvent::Cancelled { ack } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "cancelled",
            "timestamp": timestamp,
            "nonce": ack.nonce,
            "message": ack.message,
        }),
        ReconnectJournalEvent::Superseded {
            ack,
            replacement_nonce,
        } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "superseded",
            "timestamp": timestamp,
            "nonce": ack.nonce,
            "message": ack.message,
            "replacement_nonce": replacement_nonce,
        }),
        ReconnectJournalEvent::Expired { ack } => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "expired",
            "timestamp": timestamp,
            "nonce": ack.nonce,
            "message": ack.message,
        }),
    }
}

fn reconnect_event_value_with_context(
    event: &ReconnectJournalEvent,
    request: Option<&ReconnectRequest>,
) -> serde_json::Value {
    let mut value = reconnect_event_value(event);
    if let Some(request) = request {
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "instance".to_string(),
                serde_json::Value::String(request.instance.clone()),
            );
            fields.insert(
                "expected_profile".to_string(),
                serde_json::Value::String(request.expected_profile.clone()),
            );
        }
    }
    value
}

fn reconnect_event_timestamp(value: &serde_json::Value, fallback: u64) -> u64 {
    value
        .get("timestamp")
        .and_then(serde_json::Value::as_u64)
        .filter(|timestamp| *timestamp > 0)
        .unwrap_or(fallback)
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
        let timestamp = reconnect_event_timestamp(&value, reconnect_file_timestamp(path));
        let event = reconnect_event_from_value(&value)?;
        match event {
            ReconnectJournalEvent::Submitted { request } => match &state {
                None => {
                    state = Some(ReconnectJournalState {
                        request,
                        phase: ReconnectPhase::Submitted,
                        submitted_at: timestamp,
                        claimed_at: None,
                        terminal_at: None,
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
                    ReconnectPhase::Submitted => {
                        current.phase = ReconnectPhase::Claimed;
                        current.claimed_at = Some(timestamp);
                    }
                    ReconnectPhase::Claimed => {}
                    ReconnectPhase::Settled(_)
                    | ReconnectPhase::Cancelled(_)
                    | ReconnectPhase::Superseded { .. }
                    | ReconnectPhase::Expired(_) => {
                        return Err(reconnect_journal_error("claim follows terminal settlement"))
                    }
                }
            }
            ReconnectJournalEvent::InProgress { nonce } => {
                let current = state
                    .as_mut()
                    .ok_or_else(|| reconnect_journal_error("in-progress precedes submission"))?;
                if current.request.nonce != nonce {
                    return Err(reconnect_journal_error(
                        "in-progress nonce does not match request",
                    ));
                }
                match current.phase {
                    ReconnectPhase::Submitted => {
                        current.phase = ReconnectPhase::Claimed;
                        current.claimed_at = Some(timestamp);
                    }
                    ReconnectPhase::Claimed => {}
                    ReconnectPhase::Settled(_)
                    | ReconnectPhase::Cancelled(_)
                    | ReconnectPhase::Superseded { .. }
                    | ReconnectPhase::Expired(_) => {
                        return Err(reconnect_journal_error(
                            "in-progress follows terminal settlement",
                        ))
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
                        current.terminal_at = Some(timestamp);
                    }
                    ReconnectPhase::Settled(existing) if existing == &ack => {}
                    ReconnectPhase::Settled(_)
                    | ReconnectPhase::Cancelled(_)
                    | ReconnectPhase::Superseded { .. }
                    | ReconnectPhase::Expired(_) => {
                        return Err(reconnect_journal_error(
                            "nonce has conflicting terminal settlements",
                        ))
                    }
                }
            }
            ReconnectJournalEvent::Cancelled { ack } => {
                let current = state
                    .as_mut()
                    .ok_or_else(|| reconnect_journal_error("cancellation precedes submission"))?;
                if current.request.nonce != ack.nonce {
                    return Err(reconnect_journal_error(
                        "cancellation nonce does not match request",
                    ));
                }
                match &current.phase {
                    ReconnectPhase::Submitted | ReconnectPhase::Claimed => {
                        current.phase = ReconnectPhase::Cancelled(ack);
                        current.terminal_at = Some(timestamp);
                    }
                    ReconnectPhase::Cancelled(existing) if existing == &ack => {}
                    _ => {
                        return Err(reconnect_journal_error(
                            "nonce has conflicting terminal cancellation",
                        ))
                    }
                }
            }
            ReconnectJournalEvent::Superseded {
                ack,
                replacement_nonce,
            } => {
                let current = state
                    .as_mut()
                    .ok_or_else(|| reconnect_journal_error("supersession precedes submission"))?;
                if current.request.nonce != ack.nonce {
                    return Err(reconnect_journal_error(
                        "supersession nonce does not match request",
                    ));
                }
                match &current.phase {
                    ReconnectPhase::Submitted | ReconnectPhase::Claimed => {
                        current.phase = ReconnectPhase::Superseded {
                            ack,
                            replacement_nonce,
                        };
                        current.terminal_at = Some(timestamp);
                    }
                    ReconnectPhase::Superseded {
                        ack: existing,
                        replacement_nonce: current_replacement,
                    } if existing == &ack && current_replacement == &replacement_nonce => {}
                    _ => {
                        return Err(reconnect_journal_error(
                            "nonce has conflicting terminal supersession",
                        ))
                    }
                }
            }
            ReconnectJournalEvent::Expired { ack } => {
                let current = state
                    .as_mut()
                    .ok_or_else(|| reconnect_journal_error("expiration precedes submission"))?;
                if current.request.nonce != ack.nonce {
                    return Err(reconnect_journal_error(
                        "expiration nonce does not match request",
                    ));
                }
                match &current.phase {
                    ReconnectPhase::Settled(existing) if existing == &ack => {}
                    ReconnectPhase::Submitted | ReconnectPhase::Claimed => {
                        current.phase = ReconnectPhase::Expired(ack);
                        current.terminal_at = Some(timestamp);
                    }
                    _ => {
                        return Err(reconnect_journal_error(
                            "nonce has conflicting terminal expiration",
                        ))
                    }
                }
            }
        }
    }
    Ok(state)
}

fn reconnect_status_for_state(state: ReconnectJournalState) -> ReconnectStatus {
    let (lifecycle, replacement_nonce, terminal_result) = match &state.phase {
        ReconnectPhase::Submitted => (ReconnectLifecycle::Submitted, None, None),
        ReconnectPhase::Claimed => (ReconnectLifecycle::InProgress, None, None),
        ReconnectPhase::Settled(ack) => (ReconnectLifecycle::Settled, None, Some(ack.clone())),
        ReconnectPhase::Cancelled(ack) => (ReconnectLifecycle::Cancelled, None, Some(ack.clone())),
        ReconnectPhase::Superseded {
            ack,
            replacement_nonce,
        } => (
            ReconnectLifecycle::Superseded,
            Some(replacement_nonce.clone()),
            Some(ack.clone()),
        ),
        ReconnectPhase::Expired(ack) => (ReconnectLifecycle::Expired, None, Some(ack.clone())),
    };
    ReconnectStatus {
        nonce: state.request.nonce,
        profile: state.request.expected_profile,
        owner: state.request.instance,
        state: lifecycle,
        submitted_at: state.submitted_at,
        claimed_at: state.claimed_at,
        terminal_at: state.terminal_at,
        retention_expires_at: state.terminal_at.map(reconnect_retention_deadline),
        replacement_nonce,
        terminal_result,
        detail: None,
    }
}

fn reconnect_terminal_event_from_phase(phase: &ReconnectPhase) -> Option<ReconnectJournalEvent> {
    match phase {
        ReconnectPhase::Settled(ack) => Some(ReconnectJournalEvent::Settled { ack: ack.clone() }),
        ReconnectPhase::Cancelled(ack) => {
            Some(ReconnectJournalEvent::Cancelled { ack: ack.clone() })
        }
        ReconnectPhase::Superseded {
            ack,
            replacement_nonce,
        } => Some(ReconnectJournalEvent::Superseded {
            ack: ack.clone(),
            replacement_nonce: replacement_nonce.clone(),
        }),
        ReconnectPhase::Expired(ack) => Some(ReconnectJournalEvent::Expired { ack: ack.clone() }),
        ReconnectPhase::Submitted | ReconnectPhase::Claimed => None,
    }
}

fn reconnect_status_for_retained_event(
    path: &std::path::Path,
    nonce: &str,
    event: ReconnectJournalEvent,
) -> ReconnectStatus {
    let (state, replacement_nonce, terminal_result) = match event {
        ReconnectJournalEvent::Settled { ack } => (ReconnectLifecycle::Settled, None, Some(ack)),
        ReconnectJournalEvent::Cancelled { ack } => {
            (ReconnectLifecycle::Cancelled, None, Some(ack))
        }
        ReconnectJournalEvent::Superseded {
            ack,
            replacement_nonce,
        } => (
            ReconnectLifecycle::Superseded,
            Some(replacement_nonce),
            Some(ack),
        ),
        ReconnectJournalEvent::Expired { ack } => (ReconnectLifecycle::Expired, None, Some(ack)),
        ReconnectJournalEvent::Submitted { .. }
        | ReconnectJournalEvent::Claimed { .. }
        | ReconnectJournalEvent::InProgress { .. } => (ReconnectLifecycle::Ambiguous, None, None),
    };
    let timestamp = reconnect_file_timestamp(path);
    ReconnectStatus {
        nonce: nonce.to_string(),
        profile: String::new(),
        owner: String::new(),
        state,
        submitted_at: timestamp,
        claimed_at: None,
        terminal_at: Some(timestamp),
        retention_expires_at: Some(reconnect_retention_deadline(timestamp)),
        replacement_nonce,
        terminal_result,
        detail: Some("terminal result is retained after its journal was reaped".to_string()),
    }
}

fn reconnect_success_proof_is_present(
    journal: &std::path::Path,
    ack: &ReconnectAck,
) -> Result<bool> {
    let proof = reconnect_success_proof_path_at(journal, &ack.nonce)?;
    Ok(load_reconnect_receipt_at(&proof)?.as_ref() == Some(ack))
}

fn mark_status_ambiguous_without_success_proof(
    journal: &std::path::Path,
    status: &mut ReconnectStatus,
) -> Result<()> {
    let Some(ack) = status.terminal_result.as_ref() else {
        return Ok(());
    };
    if ack.accepted && !reconnect_success_proof_is_present(journal, ack)? {
        status.state = ReconnectLifecycle::Ambiguous;
        status.detail = Some(
            "accepted reconnect result has no durable success proof; preserve the record and recover explicitly"
                .to_string(),
        );
    }
    Ok(())
}

fn reconnect_retention_deadline(timestamp: u64) -> u64 {
    timestamp.saturating_add(RECONNECT_TERMINAL_RETENTION.as_millis() as u64)
}

fn reconnect_file_timestamp(path: &std::path::Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Read a nonce's lifecycle without acquiring a lease or changing any file.
/// A malformed journal is represented as `ambiguous` so status remains useful
/// for recovery while the lower-level loader continues to fail closed.
fn reconnect_status_at(journal: &std::path::Path, nonce: &str) -> Result<Option<ReconnectStatus>> {
    let state = match load_reconnect_journal_at(journal) {
        Ok(state) => state,
        Err(error) => {
            return Ok(Some(ReconnectStatus {
                nonce: nonce.to_string(),
                profile: String::new(),
                owner: String::new(),
                state: ReconnectLifecycle::Ambiguous,
                submitted_at: reconnect_file_timestamp(journal),
                claimed_at: None,
                terminal_at: None,
                retention_expires_at: None,
                replacement_nonce: None,
                terminal_result: None,
                detail: Some(error.report().message),
            }))
        }
    };
    if let Some(state) = state {
        if state.request.nonce == nonce {
            let mut status = reconnect_status_for_state(state);
            mark_status_ambiguous_without_success_proof(journal, &mut status)?;
            return Ok(Some(status));
        }
    }
    let receipt = reconnect_receipt_path_at(journal, nonce)?;
    let Some(record) = load_reconnect_receipt_record_at(&receipt)? else {
        return Ok(None);
    };
    let mut status = reconnect_status_for_retained_event(&receipt, nonce, record.event);
    status.submitted_at = record.timestamp;
    status.terminal_at = Some(record.timestamp);
    status.retention_expires_at = Some(reconnect_retention_deadline(record.timestamp));
    status.owner = record.instance.unwrap_or_default();
    status.profile = record.expected_profile.unwrap_or_default();
    mark_status_ambiguous_without_success_proof(journal, &mut status)?;
    Ok(Some(status))
}

fn rollback_reconnect_journal_at(path: &std::path::Path, original_len: u64) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_len(original_len)?;
    file.sync_all()
}

fn append_reconnect_bytes_at_with<M, O, W, S, R>(
    path: &std::path::Path,
    bytes: &[u8],
    mut metadata_len: M,
    mut open_append: O,
    mut write_all: W,
    mut sync_all: S,
    mut rollback: R,
) -> Result<()>
where
    M: FnMut(&std::path::Path) -> std::io::Result<u64>,
    O: FnMut(&std::path::Path) -> std::io::Result<std::fs::File>,
    W: FnMut(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
    S: FnMut(&std::fs::File) -> std::io::Result<()>,
    R: FnMut(&std::path::Path, u64) -> std::io::Result<()>,
{
    let original_len = metadata_len(path).map_err(|error| {
        agent_core::CoreError::Message(format!(
            "cannot inspect fleetyd reconnect journal before append {}: {error}",
            path.display()
        ))
    })?;
    let mut file = open_append(path).map_err(|error| {
        agent_core::CoreError::Message(format!(
            "cannot open fleetyd reconnect journal {}: {error}",
            path.display()
        ))
    })?;
    let result = write_all(&mut file, bytes).and_then(|()| sync_all(&file));
    if let Err(error) = result {
        drop(file);
        if let Err(rollback_error) = rollback(path, original_len) {
            return Err(agent_core::CoreError::Message(format!(
                "cannot make fleetyd reconnect journal event durable {}: {error}; \
                 rollback to {original_len} bytes also failed: {rollback_error}",
                path.display()
            )));
        }
        return Err(agent_core::CoreError::Message(format!(
            "cannot make fleetyd reconnect journal event durable {}: {error}",
            path.display()
        )));
    }
    Ok(())
}

fn append_reconnect_event_once_at(
    path: &std::path::Path,
    event: &ReconnectJournalEvent,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            agent_core::CoreError::Message(format!(
                "cannot create daemon control directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    match std::fs::read(path) {
        Ok(existing) => {
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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot inspect fleetyd reconnect journal before repair {}: {error}",
                path.display()
            )));
        }
    }
    let mut bytes = reconnect_event_value(event).to_string().into_bytes();
    bytes.push(b'\n');
    let created = !path.exists();
    use std::io::Write;
    append_reconnect_bytes_at_with(
        path,
        &bytes,
        |path| match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error),
        },
        |path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        },
        |file, bytes| file.write_all(bytes),
        |file| file.sync_all(),
        rollback_reconnect_journal_at,
    )?;
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

fn append_reconnect_event_at(path: &std::path::Path, event: &ReconnectJournalEvent) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        match append_reconnect_event_once_at(path, event) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                    return Err(error);
                }
                last_error = Some(error);
                std::thread::sleep(RECONNECT_POLL);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Err(reconnect_journal_error(
            "reconnect journal append exhausted its retry budget",
        )),
    }
}

fn submit_reconnect_at(path: &std::path::Path, request: &ReconnectRequest) -> Result<()> {
    let receipt = reconnect_receipt_path_at(path, &request.nonce)?;
    if load_reconnect_receipt_at(&receipt)?.is_some() {
        return Err(agent_core::CoreError::Message(format!(
            "fleetyd reconnect nonce '{}' already has a retained terminal result",
            request.nonce
        )));
    }
    let proof = reconnect_success_proof_path_at(path, &request.nonce)?;
    if proof.exists() {
        return Err(agent_core::CoreError::Message(format!(
            "fleetyd reconnect nonce '{}' already has a retained success proof",
            request.nonce
        )));
    }
    if let Some(existing) = load_reconnect_journal_at(path)? {
        let state = match &existing.phase {
            ReconnectPhase::Submitted => "queued",
            ReconnectPhase::Claimed => "being processed",
            ReconnectPhase::Settled(_)
            | ReconnectPhase::Cancelled(_)
            | ReconnectPhase::Superseded { .. }
            | ReconnectPhase::Expired(_) => "settled but not yet observed",
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
        ReconnectPhase::Claimed
        | ReconnectPhase::Settled(_)
        | ReconnectPhase::Cancelled(_)
        | ReconnectPhase::Superseded { .. }
        | ReconnectPhase::Expired(_) => Ok(None),
    }
}

fn cancel_reconnect_at(path: &std::path::Path, nonce: &str, owner: &str) -> Result<ReconnectAck> {
    let state = load_reconnect_journal_at(path)?.ok_or_else(|| {
        agent_core::CoreError::Message(format!(
            "reconnect request '{nonce}' is unknown or its terminal result is no longer retained"
        ))
    })?;
    if state.request.nonce != nonce {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect request '{nonce}' is unknown while another request is active"
        )));
    }
    if state.request.instance != owner {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect request '{nonce}' belongs to a foreign owner; cancellation refused"
        )));
    }
    match &state.phase {
        ReconnectPhase::Settled(ack) if ack.accepted => {
            return Err(agent_core::CoreError::Message(format!(
                "reconnect request '{nonce}' has a durable success proof; cancellation refused"
            )))
        }
        ReconnectPhase::Settled(_)
        | ReconnectPhase::Cancelled(_)
        | ReconnectPhase::Superseded { .. }
        | ReconnectPhase::Expired(_) => {
            return Err(agent_core::CoreError::Message(format!(
                "reconnect request '{nonce}' is already terminal; cancellation refused"
            )))
        }
        ReconnectPhase::Submitted | ReconnectPhase::Claimed => {}
    }
    let ack = ReconnectAck {
        nonce: nonce.to_string(),
        accepted: false,
        message: "fleetyd reconnect request was cancelled by its owner".to_string(),
    };
    let event = ReconnectJournalEvent::Cancelled { ack: ack.clone() };
    append_reconnect_event_at(path, &event)?;
    let receipt = reconnect_receipt_path_at(path, nonce)?;
    preserve_reconnect_terminal_event_at_with(&receipt, &event, Some(&state.request))?;
    Ok(ack)
}

fn supersede_reconnect_at(
    path: &std::path::Path,
    nonce: &str,
    replacement_nonce: &str,
    owner: &str,
) -> Result<ReconnectRequest> {
    if nonce == replacement_nonce {
        return Err(agent_core::CoreError::Message(
            "reconnect supersession requires a new nonce".to_string(),
        ));
    }
    let retained_receipt = reconnect_receipt_path_at(path, nonce)?;
    if let Some(record) = load_reconnect_receipt_record_at(&retained_receipt)? {
        if let ReconnectJournalEvent::Superseded {
            ack,
            replacement_nonce: retained_replacement,
        } = record.event
        {
            if ack.nonce == nonce && retained_replacement == replacement_nonce {
                let expected_profile = record.expected_profile.ok_or_else(|| {
                    reconnect_journal_error(
                        "retained supersession has no profile context; inspect the receipt before retrying",
                    )
                })?;
                let replacement = ReconnectRequest {
                    instance: owner.to_string(),
                    nonce: replacement_nonce.to_string(),
                    expected_profile,
                };
                if let Some(active) = load_reconnect_journal_at(path)? {
                    if active.request == replacement {
                        return Ok(replacement);
                    }
                    return Err(agent_core::CoreError::Message(format!(
                        "replacement nonce '{}' is already owned by another reconnect request",
                        replacement_nonce
                    )));
                }
                submit_reconnect_at(path, &replacement)?;
                return Ok(replacement);
            }
        }
    }
    let state = load_reconnect_journal_at(path)?.ok_or_else(|| {
        agent_core::CoreError::Message(format!(
            "reconnect request '{nonce}' is unknown or its terminal result is no longer retained"
        ))
    })?;
    if state.request.nonce != nonce {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect request '{nonce}' is unknown while another request is active"
        )));
    }
    if state.request.instance != owner {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect request '{nonce}' belongs to a foreign owner; supersession refused"
        )));
    }
    match &state.phase {
        ReconnectPhase::Submitted | ReconnectPhase::Claimed => {}
        ReconnectPhase::Settled(ack) if ack.accepted => {
            return Err(agent_core::CoreError::Message(format!(
                "reconnect request '{nonce}' has a durable success proof; supersession refused"
            )))
        }
        ReconnectPhase::Settled(_)
        | ReconnectPhase::Cancelled(_)
        | ReconnectPhase::Superseded { .. }
        | ReconnectPhase::Expired(_) => {
            return Err(agent_core::CoreError::Message(format!(
                "reconnect request '{nonce}' is already terminal; supersession refused"
            )))
        }
    }
    let replacement = ReconnectRequest {
        instance: owner.to_string(),
        nonce: replacement_nonce.to_string(),
        expected_profile: state.request.expected_profile.clone(),
    };
    let ack = ReconnectAck {
        nonce: nonce.to_string(),
        accepted: false,
        message: format!(
            "fleetyd reconnect request was superseded by '{}'",
            replacement_nonce
        ),
    };
    let event = ReconnectJournalEvent::Superseded {
        ack,
        replacement_nonce: replacement_nonce.to_string(),
    };
    append_reconnect_event_at(path, &event)?;
    preserve_reconnect_terminal_event_at_with(&retained_receipt, &event, Some(&state.request))?;
    reap_reconnect_journal_at(path)?;
    submit_reconnect_at(path, &replacement)?;
    Ok(replacement)
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

fn settle_pending_reconnect_at_with_proof<P, F>(
    path: &std::path::Path,
    pending: &mut Option<PendingReconnect>,
    publish_proof: P,
    append: F,
) -> Result<bool>
where
    P: FnOnce(&std::path::Path, &ReconnectAck) -> Result<()>,
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
    if decision.accepted {
        publish_proof(path, &decision)?;
    }
    // A waiting caller may observe and reap the durable event immediately after
    // the proof returns. Re-reading here races that legitimate cleanup and can
    // revive an already-observed decision into a journal without its Submitted
    // event. Accepted success commits only after both durability steps return.
    pending.take();
    Ok(true)
}

fn settle_pending_reconnect_at_with<F>(
    path: &std::path::Path,
    pending: &mut Option<PendingReconnect>,
    append: F,
) -> Result<bool>
where
    F: FnOnce(&std::path::Path, &ReconnectJournalEvent) -> Result<()>,
{
    settle_pending_reconnect_at_with_proof(path, pending, publish_reconnect_success_proof, append)
}

fn settle_pending_reconnect(pending: &mut Option<PendingReconnect>) -> Result<bool> {
    settle_pending_reconnect_with_credential_sync(pending, || {
        connection::sync_connections_publication()
    })
}

fn settle_pending_reconnect_bounded_with<F>(
    pending: &mut Option<PendingReconnect>,
    mut settle: F,
) -> Result<bool>
where
    F: FnMut(&mut Option<PendingReconnect>) -> Result<bool>,
{
    let mut last_error = None;
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        match settle(pending) {
            Ok(result) => return Ok(result),
            Err(error) => {
                if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                    return Err(error);
                }
                last_error = Some(error);
                std::thread::sleep(RECONNECT_POLL);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Err(reconnect_journal_error(
            "reconnect settlement exhausted its retry budget",
        )),
    }
}

fn settle_pending_reconnect_bounded(pending: &mut Option<PendingReconnect>) -> Result<bool> {
    settle_pending_reconnect_bounded_with(pending, settle_pending_reconnect)
}

/// Whether the endpoint an authenticated session used is one of this profile's.
///
/// Roaming means it may be a saved alternative rather than the primary, and the
/// credential commit deliberately freezes the endpoint that authenticated.
/// Demanding equality with the primary turned a retryable storage failure into
/// a permanent one for exactly the sessions roaming exists to serve.
fn authenticated_endpoint_belongs_to(profile: &connection::Profile, url: &str) -> bool {
    profile.url == url
        || profile.configured_url.as_deref() == Some(url)
        || profile.endpoints.iter().any(|endpoint| endpoint == url)
}

fn settle_pending_reconnect_with_credential_sync<S>(
    pending: &mut Option<PendingReconnect>,
    sync_credentials: S,
) -> Result<bool>
where
    S: FnOnce() -> Result<()>,
{
    if let Some(authenticated) = pending
        .as_ref()
        .and_then(|pending| pending.authenticated.clone())
    {
        let expected_profile = pending
            .as_ref()
            .map(|pending| pending.request.expected_profile.as_str())
            .ok_or_else(|| reconnect_journal_error("authenticated reconnect disappeared"))?;
        let owner_matches = connection::inspect_locked(|conns| {
            let source_matches = matches!(
                authenticated.target.source(),
                Source::Profile(name) | Source::OverrideProfile(name)
                    if name == expected_profile
            );
            let profile = conns.profiles.get(expected_profile);
            Ok(source_matches
                && conns.current.as_deref() == Some(expected_profile)
                && profile.is_some_and(|profile| {
                    authenticated_endpoint_belongs_to(profile, authenticated.target.url())
                        && profile.token.as_deref() == authenticated.target.token()
                        && profile.fingerprint.as_deref()
                            == Some(authenticated.server_fingerprint.as_str())
                }))
        })?;
        if !owner_matches {
            return reject_frozen_authenticated_reconnect(
                pending,
                format!(
                    "requested profile '{expected_profile}' changed before reconnect success became durable"
                ),
            );
        }
        sync_credentials()?;
        return settle_authenticated_reconnect_with(
            &authenticated.target,
            Some(&authenticated.server_fingerprint),
            pending,
            append_reconnect_event_at,
        );
    }
    let _lease = acquire_reconnect_lease()?;
    settle_pending_reconnect_at_with(
        &reconnect_journal_path(),
        pending,
        append_reconnect_event_at,
    )
}

fn reject_frozen_authenticated_reconnect(
    pending: &mut Option<PendingReconnect>,
    message: String,
) -> Result<bool> {
    let _lease = acquire_reconnect_lease()?;
    let journal = reconnect_journal_path();
    let nonce = pending
        .as_ref()
        .map(|pending| pending.request.nonce.clone())
        .ok_or_else(|| reconnect_journal_error("authenticated reconnect disappeared"))?;
    let failure = ReconnectAck {
        nonce: nonce.clone(),
        accepted: false,
        message,
    };
    let receipt = reconnect_receipt_path_at(&journal, &nonce)?;
    if load_reconnect_receipt_at(&receipt)?.as_ref() == Some(&failure) {
        reap_reconnect_journal_at(&journal)?;
        pending.take();
        return Ok(true);
    }
    if let Some(state) = load_reconnect_journal_at(&journal)? {
        if state.request.nonce == nonce {
            if let ReconnectPhase::Settled(ack) = state.phase {
                if ack.accepted && reconnect_success_proof_matches(&journal, &ack)? {
                    pending.take();
                    return Ok(true);
                }
                preserve_reconnect_receipt_at(&receipt, &failure)?;
                reap_reconnect_journal_at(&journal)?;
                pending.take();
                return Ok(true);
            }
        }
    }
    if let Some(pending) = pending.as_mut() {
        pending.decision = Some(failure);
        pending.authenticated = None;
    }
    settle_pending_reconnect_at_with(&journal, pending, append_reconnect_event_at)
}

async fn settle_pending_reconnect_before_exit_with<F>(
    pending: &mut Option<PendingReconnect>,
    mut settle: F,
) where
    F: FnMut(&mut Option<PendingReconnect>) -> Result<bool>,
{
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        if !pending
            .as_ref()
            .is_some_and(|pending| pending.decision.is_some())
        {
            return;
        }
        match settle(pending) {
            Ok(_) => return,
            Err(error) => {
                tracing::warn!(
                    report = ?error.report(),
                    "cannot persist reconnect settlement during shutdown; will retry"
                );
                if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                    tracing::error!(
                        report = ?error.report(),
                        "reconnect settlement remained unpersisted after bounded shutdown retries; retaining evidence"
                    );
                    return;
                }
                tokio::time::sleep(RECONNECT_POLL).await;
            }
        }
    }
}

async fn settle_pending_reconnect_before_exit(pending: &mut Option<PendingReconnect>) {
    settle_pending_reconnect_before_exit_with(pending, settle_pending_reconnect).await;
}

fn reconnect_success_proof_matches(journal: &std::path::Path, ack: &ReconnectAck) -> Result<bool> {
    let proof = reconnect_success_proof_path_at(journal, &ack.nonce)?;
    if load_reconnect_receipt_at(&proof)?.as_ref() != Some(ack) {
        return Ok(false);
    }
    let parent = proof
        .parent()
        .ok_or_else(|| reconnect_journal_error("success proof has no control directory"))?;
    sync_reconnect_receipt_at(&proof, parent)?;
    Ok(true)
}

fn require_reconnect_success_proof(journal: &std::path::Path, ack: &ReconnectAck) -> Result<()> {
    if ack.accepted && !reconnect_success_proof_matches(journal, ack)? {
        return Err(reconnect_journal_error(
            "success settlement has no durable credential commit proof",
        ));
    }
    Ok(())
}

fn quarantine_ambiguous_success_proof_with<R, D, S, W>(
    path: &std::path::Path,
    mut rename: R,
    mut remove: D,
    mut sync_directories: S,
    mut wait: W,
) -> Result<()>
where
    R: FnMut(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
    D: FnMut(&std::path::Path) -> std::io::Result<()>,
    S: FnMut(&std::path::Path) -> Result<()>,
    W: FnMut(),
{
    let parent = path
        .parent()
        .ok_or_else(|| reconnect_journal_error("success proof has no control directory"))?;
    let quarantine = parent.join(format!(".receipt-{}.tmp", control_nonce()));
    let mut hidden = false;
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        match rename(path, &quarantine) {
            Ok(()) => {
                hidden = true;
                break;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                hidden = true;
                break;
            }
            Err(rename_error) => match remove(path) {
                Ok(()) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) =>
                {
                    hidden = true;
                    break;
                }
                Err(remove_error) => {
                    tracing::warn!(
                        proof = %path.display(),
                        %rename_error,
                        %remove_error,
                        "cannot hide ambiguous reconnect success proof; retaining leases"
                    );
                    if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                        return Err(reconnect_journal_error(format!(
                            "cannot hide ambiguous reconnect success proof after {} retries: {rename_error}; {remove_error}",
                            RECONNECT_HOUSEKEEPING_RETRIES
                        )));
                    }
                    wait();
                }
            },
        }
    }
    if !hidden {
        return Err(reconnect_journal_error(
            "cannot hide ambiguous reconnect success proof after bounded retries",
        ));
    }
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        match sync_directories(parent) {
            Ok(()) => return Ok(()),
            Err(error) => {
                tracing::warn!(
                    proof = %path.display(),
                    report = ?error.report(),
                    "ambiguous reconnect success proof is hidden but not durable; retaining leases"
                );
                if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                    return Err(reconnect_journal_error(format!(
                        "ambiguous reconnect success proof quarantine is not durable after {} retries: {}",
                        RECONNECT_HOUSEKEEPING_RETRIES,
                        error.report().message
                    )));
                }
                wait();
            }
        }
    }
    Ok(())
}

fn quarantine_ambiguous_success_proof(path: &std::path::Path) -> Result<()> {
    quarantine_ambiguous_success_proof_with(
        path,
        |from, to| std::fs::rename(from, to),
        |target| std::fs::remove_file(target),
        sync_hidden_reconnect_proof_directories_at,
        || std::thread::sleep(RECONNECT_POLL),
    )
}

fn publish_reconnect_success_proof(journal: &std::path::Path, ack: &ReconnectAck) -> Result<()> {
    let proof = reconnect_success_proof_path_at(journal, &ack.nonce)?;
    match preserve_reconnect_receipt_at(&proof, ack) {
        Ok(()) => Ok(()),
        Err(error) => {
            quarantine_ambiguous_success_proof(&proof)?;
            Err(error)
        }
    }
}

fn freeze_authenticated_reconnect(
    target: &Resolved,
    server_fingerprint: Option<&str>,
    pending: &mut Option<PendingReconnect>,
) -> Result<()> {
    let decision = ReconnectAck {
        nonce: pending
            .as_ref()
            .map(|pending| pending.request.nonce.clone())
            .ok_or_else(|| reconnect_journal_error("authenticated request disappeared"))?,
        accepted: true,
        message: "fleetyd authenticated the selected profile and loaded its Server identity"
            .to_string(),
    };
    if let Some(existing) = pending
        .as_ref()
        .and_then(|pending| pending.decision.as_ref())
    {
        if existing != &decision {
            return Err(reconnect_journal_error(
                "authenticated reconnect already froze a different terminal result",
            ));
        }
    } else if let Some(pending) = pending.as_mut() {
        pending.decision = Some(decision);
    }
    let authenticated = AuthenticatedReconnect {
        target: target.clone(),
        server_fingerprint: server_fingerprint
            .ok_or_else(|| reconnect_journal_error("authenticated reconnect has no identity"))?
            .to_string(),
    };
    if let Some(existing) = pending
        .as_ref()
        .and_then(|pending| pending.authenticated.as_ref())
    {
        if existing != &authenticated {
            return Err(reconnect_journal_error(
                "authenticated reconnect retry changed its committed owner snapshot",
            ));
        }
    } else if let Some(pending) = pending.as_mut() {
        pending.authenticated = Some(authenticated);
    }
    Ok(())
}

fn settle_authenticated_reconnect_with_proof<P, F>(
    target: &Resolved,
    server_fingerprint: Option<&str>,
    pending: &mut Option<PendingReconnect>,
    publish_proof: P,
    append: F,
) -> Result<bool>
where
    P: FnOnce(&std::path::Path, &ReconnectAck) -> Result<()>,
    F: FnOnce(&std::path::Path, &ReconnectJournalEvent) -> Result<()>,
{
    let Some(expected_profile) = pending
        .as_ref()
        .map(|pending| pending.request.expected_profile.clone())
    else {
        return Ok(false);
    };
    // Keep reconnect ownership outside the connections lease. The success
    // journal cannot become observable until the inner connections guard has
    // dropped, so an immediate caller-triggered process exit cannot strand the
    // credential mutation lock.
    let _reconnect_lease = acquire_reconnect_lease()?;
    connection::inspect_locked(|conns| {
        let fingerprint = reconnect_target_fingerprint_in(conns, &expected_profile, target)?;
        if fingerprint.as_deref() != server_fingerprint {
            return Err(agent_core::CoreError::Message(format!(
                "fleetyd authenticated profile '{expected_profile}', but its persisted owner snapshot changed before settlement"
            )));
        }
        freeze_authenticated_reconnect(target, server_fingerprint, pending)?;
        let journal = reconnect_journal_path();
        // The journal must report a successful durability boundary before the
        // caller-visible proof is published. Any append error remains a
        // retryable frozen success and is never promoted from readable bytes.
        settle_pending_reconnect_at_with_proof(&journal, pending, publish_proof, append)
    })
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
    settle_authenticated_reconnect_with_proof(
        target,
        server_fingerprint,
        pending,
        publish_reconnect_success_proof,
        append,
    )
}

#[cfg(test)]
fn persist_authenticated_profile_credentials(
    target: &Resolved,
    expected_profile: &str,
    server_fingerprint: &str,
    minted_token: Option<&str>,
) -> Result<Resolved> {
    if target.profile_owner_name() != Some(expected_profile) {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' did not resolve to its durable owner"
        )));
    }
    connection::store_resolved_profile_credentials(target, minted_token, server_fingerprint)
        .map(|(_, committed)| committed)
}

#[cfg(test)]
fn commit_authenticated_reconnect_with<P, A>(
    target: &Resolved,
    server_fingerprint: &str,
    minted_token: Option<&str>,
    pending: &mut Option<PendingReconnect>,
    persist: P,
    append: A,
) -> Result<bool>
where
    P: FnOnce(&Resolved, &str, &str, Option<&str>) -> Result<Resolved>,
    A: FnOnce(&std::path::Path, &ReconnectJournalEvent) -> Result<()>,
{
    commit_authenticated_reconnect_with_settle(
        target,
        server_fingerprint,
        minted_token,
        pending,
        persist,
        |persisted_target, fingerprint, pending| {
            settle_authenticated_reconnect_with(persisted_target, fingerprint, pending, append)
        },
    )
}

#[cfg(test)]
fn commit_authenticated_reconnect_with_settle<P, S>(
    target: &Resolved,
    server_fingerprint: &str,
    minted_token: Option<&str>,
    pending: &mut Option<PendingReconnect>,
    persist: P,
    settle: S,
) -> Result<bool>
where
    P: FnOnce(&Resolved, &str, &str, Option<&str>) -> Result<Resolved>,
    S: FnOnce(&Resolved, Option<&str>, &mut Option<PendingReconnect>) -> Result<bool>,
{
    let Some(expected_profile) = pending
        .as_ref()
        .map(|pending| pending.request.expected_profile.clone())
    else {
        return Ok(false);
    };
    let persisted_target = persist(target, &expected_profile, server_fingerprint, minted_token)?;
    freeze_authenticated_reconnect(&persisted_target, Some(server_fingerprint), pending)?;
    settle(&persisted_target, Some(server_fingerprint), pending)
}

fn commit_authenticated_reconnect(
    target: &Resolved,
    owner: Option<&SessionCredentialOwner>,
    server_fingerprint: &str,
    minted_token: Option<&str>,
    server_endpoints: &[String],
    sealed: bool,
    pending: &mut Option<PendingReconnect>,
) -> Result<bool> {
    let Some(expected_profile) = pending
        .as_ref()
        .map(|pending| pending.request.expected_profile.clone())
    else {
        return Ok(false);
    };
    let Some(SessionCredentialOwner::Existing(owner_target)) = owner else {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' has no frozen credential owner"
        )));
    };
    if owner_target.as_ref() != target
        || owner_target.profile_owner_name() != Some(expected_profile.as_str())
    {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect requested profile '{expected_profile}', but the frozen owner does not match"
        )));
    }
    match connection::store_resolved_profile_credentials_recoverable(
        owner_target,
        minted_token,
        server_fingerprint,
    )? {
        connection::CredentialCommit::Durable { committed, .. } => {
            let committed = match connection::learn_resolved_profile_endpoints(
                &committed,
                server_fingerprint,
                server_endpoints,
                sealed,
            ) {
                Ok(refreshed) => refreshed,
                Err(error) => {
                    tracing::warn!(
                        report = ?error.report(),
                        "could not record what this session learned about the profile"
                    );
                    committed
                }
            };
            freeze_authenticated_reconnect(&committed, Some(server_fingerprint), pending)?;
            settle_authenticated_reconnect_with(
                &committed,
                Some(server_fingerprint),
                pending,
                append_reconnect_event_at,
            )
        }
        connection::CredentialCommit::PublishedNotDurable {
            committed, error, ..
        } => {
            freeze_authenticated_reconnect(&committed, Some(server_fingerprint), pending)?;
            Err(error)
        }
    }
}

fn recover_reconnect_for_instance_at(path: &std::path::Path, instance: &str) -> Result<()> {
    let Some(state) = load_reconnect_journal_at(path)? else {
        return Ok(());
    };
    let receipt = reconnect_receipt_path_at(path, &state.request.nonce)?;
    if let Some(ack) = load_reconnect_receipt_at(&receipt)? {
        if ack.nonce != state.request.nonce {
            return Err(reconnect_journal_error(format!(
                "reconnect receipt {} belongs to nonce {} instead of {}",
                receipt.display(),
                ack.nonce,
                state.request.nonce
            )));
        }
        reap_reconnect_journal_at(path)?;
        return Ok(());
    }
    if let Some(event) = reconnect_terminal_event_from_phase(&state.phase) {
        let ack = match &event {
            ReconnectJournalEvent::Settled { ack }
            | ReconnectJournalEvent::Cancelled { ack }
            | ReconnectJournalEvent::Superseded { ack, .. }
            | ReconnectJournalEvent::Expired { ack } => ack,
            ReconnectJournalEvent::Submitted { .. }
            | ReconnectJournalEvent::Claimed { .. }
            | ReconnectJournalEvent::InProgress { .. } => {
                return Err(reconnect_journal_error("terminal event conversion failed"))
            }
        };
        if !ack.accepted {
            preserve_reconnect_terminal_event_at_with(&receipt, &event, Some(&state.request))?;
            reap_reconnect_journal_at(path)?;
            return Ok(());
        }
        if reconnect_success_proof_matches(path, ack)? {
            preserve_reconnect_terminal_event_at_with(&receipt, &event, Some(&state.request))?;
            reap_reconnect_journal_at(path)?;
            return Ok(());
        }
        let failure = ReconnectAck {
            nonce: state.request.nonce,
            accepted: false,
            message: "fleetyd restarted before reconnect success committed its durable proof"
                .to_string(),
        };
        preserve_reconnect_receipt_at(&receipt, &failure)?;
        reap_reconnect_journal_at(path)?;
        return Ok(());
    }
    if state.request.instance == instance {
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

fn reap_expired_reconnect_records_at(
    journal: &std::path::Path,
    owner: &str,
    now_millis: u64,
) -> Result<usize> {
    let mut removed = 0;
    if let Some(state) = load_reconnect_journal_at(journal)? {
        let (ack, terminal_at) = match &state.phase {
            ReconnectPhase::Submitted | ReconnectPhase::Claimed => return Ok(0),
            ReconnectPhase::Settled(ack)
            | ReconnectPhase::Cancelled(ack)
            | ReconnectPhase::Expired(ack) => (ack, state.terminal_at),
            ReconnectPhase::Superseded { ack, .. } => (ack, state.terminal_at),
        };
        if state.request.instance != owner {
            return Err(agent_core::CoreError::Message(format!(
                "reconnect retention cleanup belongs to a foreign owner; request '{}' was preserved",
                state.request.nonce
            )));
        }
        let Some(terminal_at) = terminal_at else {
            return Err(reconnect_journal_error(
                "terminal reconnect record has no retention timestamp",
            ));
        };
        if now_millis < reconnect_retention_deadline(terminal_at) {
            return Ok(0);
        }
        let receipt = reconnect_receipt_path_at(journal, &ack.nonce)?;
        if load_reconnect_receipt_event_at(&receipt)?.is_none() {
            return Err(reconnect_journal_error(
                "expired reconnect record is missing its terminal receipt",
            ));
        }
        if ack.accepted && !reconnect_success_proof_matches(journal, ack)? {
            return Err(reconnect_journal_error(
                "expired reconnect success is missing its durable proof",
            ));
        }
        let proof = if ack.accepted {
            Some(reconnect_success_proof_path_at(journal, &ack.nonce)?)
        } else {
            None
        };
        let mut paths = vec![journal, receipt.as_path()];
        if let Some(proof) = proof.as_deref() {
            paths.push(proof);
        }
        reap_reconnect_files_at_with(&paths, |path| {
            if path == journal {
                reap_reconnect_journal_at(path)
            } else {
                reap_reconnect_receipt_at(path)
            }
        })?;
        return Ok(1);
    }

    let Some(control_root) = journal.parent() else {
        return Ok(0);
    };
    let receipt_dir = control_root.join("fleetyd.reconnect-receipts");
    let entries = match std::fs::read_dir(&receipt_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(reconnect_journal_error(format!(
                "cannot inspect reconnect retention records {}: {error}",
                receipt_dir.display()
            )))
        }
    };
    if let Some(entries) = entries {
        for entry in entries {
            let receipt = entry
                .map_err(|error| {
                    reconnect_journal_error(format!("cannot inspect receipt: {error}"))
                })?
                .path();
            if !receipt.is_file() {
                continue;
            }
            let Some((event, timestamp)) =
                load_reconnect_receipt_event_with_timestamp_at(&receipt)?
            else {
                continue;
            };
            if now_millis < reconnect_retention_deadline(timestamp) {
                continue;
            }
            let ack = match &event {
                ReconnectJournalEvent::Settled { ack }
                | ReconnectJournalEvent::Cancelled { ack }
                | ReconnectJournalEvent::Superseded { ack, .. }
                | ReconnectJournalEvent::Expired { ack } => ack,
                ReconnectJournalEvent::Submitted { .. }
                | ReconnectJournalEvent::Claimed { .. }
                | ReconnectJournalEvent::InProgress { .. } => continue,
            };
            let proof = reconnect_success_proof_path_at(journal, &ack.nonce)?;
            if ack.accepted && !proof.exists() {
                return Err(reconnect_journal_error(format!(
                    "expired reconnect success '{}' is missing its durable proof",
                    ack.nonce
                )));
            }
            let paths = if ack.accepted {
                vec![receipt.as_path(), proof.as_path()]
            } else {
                vec![receipt.as_path()]
            };
            reap_reconnect_files_at_with(&paths, reap_reconnect_receipt_at)?;
            removed += 1;
        }
    }
    // A success proof can outlive its receipt after an interrupted cleanup.
    // Once it is past the same retention deadline and has no receipt carrier,
    // it is safe to reap the orphan proof itself. Invalid proofs remain for
    // diagnosis and are never treated as successful evidence.
    let proof_dir = control_root.join("fleetyd.reconnect-success");
    let proof_entries = match std::fs::read_dir(&proof_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(reconnect_journal_error(format!(
                "cannot inspect reconnect success retention records {}: {error}",
                proof_dir.display()
            )))
        }
    };
    if let Some(entries) = proof_entries {
        for entry in entries {
            let proof = entry
                .map_err(|error| reconnect_journal_error(format!("cannot inspect proof: {error}")))?
                .path();
            if !proof.is_file() {
                continue;
            }
            let file_name = proof
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if file_name.starts_with(".receipt-") && file_name.ends_with(".tmp") {
                continue;
            }
            let Some(record) = load_reconnect_receipt_record_at(&proof)? else {
                continue;
            };
            let ReconnectJournalEvent::Settled { ack } = record.event else {
                continue;
            };
            if !ack.accepted || now_millis < reconnect_retention_deadline(record.timestamp) {
                continue;
            }
            let receipt = reconnect_receipt_path_at(journal, &ack.nonce)?;
            if receipt.exists() {
                continue;
            }
            reap_reconnect_receipt_at(&proof)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn reap_reconnect_journal_at(path: &std::path::Path) -> Result<()> {
    let sync_parent = || -> Result<()> {
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    agent_core::CoreError::Message(format!(
                        "reconnect result cleanup directory was not durable {}: {error}",
                        parent.display()
                    ))
                })?;
        }
        Ok(())
    };
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => sync_parent(),
        Err(error) => Err(agent_core::CoreError::Message(format!(
            "reconnect result was observed, but its journal could not be reaped {}: {error}",
            path.display()
        ))),
    }
}

fn restore_reconnect_file_at(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            reconnect_journal_error(format!(
                "reconnect retention cleanup could not restore {}: {error}",
                path.display()
            ))
        })?;
    std::io::Write::write_all(&mut file, bytes).map_err(|error| {
        reconnect_journal_error(format!(
            "reconnect retention cleanup could not restore {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        reconnect_journal_error(format!(
            "reconnect retention cleanup could not make restored file durable {}: {error}",
            path.display()
        ))
    })?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                reconnect_journal_error(format!(
                    "reconnect retention cleanup could not make restored directory durable {}: {error}",
                    parent.display()
                ))
            })?;
    }
    Ok(())
}

fn reap_reconnect_files_at_with<F>(paths: &[&std::path::Path], mut reap: F) -> Result<()>
where
    F: FnMut(&std::path::Path) -> Result<()>,
{
    let snapshots = paths
        .iter()
        .filter_map(|path| match std::fs::read(path) {
            Ok(bytes) => Some(Ok(((*path).to_path_buf(), bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(reconnect_journal_error(format!(
                "reconnect retention cleanup could not snapshot {}: {error}",
                path.display()
            )))),
        })
        .collect::<Result<Vec<_>>>()?;

    for path in paths {
        if let Err(error) = reap(path) {
            let restore_result = snapshots
                .iter()
                .try_for_each(|(original, bytes)| restore_reconnect_file_at(original, bytes));
            return match restore_result {
                Ok(()) => Err(error),
                Err(restore_error) => Err(reconnect_journal_error(format!(
                    "reconnect retention cleanup failed: {error}; restoring the terminal record also failed: {restore_error}"
                ))),
            };
        }
    }
    Ok(())
}

struct ReconnectReceiptRecord {
    event: ReconnectJournalEvent,
    timestamp: u64,
    instance: Option<String>,
    expected_profile: Option<String>,
}

fn load_reconnect_receipt_record_at(
    path: &std::path::Path,
) -> Result<Option<ReconnectReceiptRecord>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(reconnect_journal_error(format!(
                "cannot read reconnect receipt {}: {error}",
                path.display()
            )))
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        reconnect_journal_error(format!(
            "reconnect receipt {} is not valid JSON: {error}",
            path.display()
        ))
    })?;
    let event = match reconnect_event_from_value(&value)? {
        event @ (ReconnectJournalEvent::Settled { .. }
        | ReconnectJournalEvent::Cancelled { .. }
        | ReconnectJournalEvent::Superseded { .. }
        | ReconnectJournalEvent::Expired { .. }) => event,
        ReconnectJournalEvent::Submitted { .. }
        | ReconnectJournalEvent::Claimed { .. }
        | ReconnectJournalEvent::InProgress { .. } => {
            return Err(reconnect_journal_error(format!(
                "reconnect receipt {} is not terminal",
                path.display()
            )))
        }
    };
    Ok(Some(ReconnectReceiptRecord {
        event,
        timestamp: reconnect_event_timestamp(&value, reconnect_file_timestamp(path)),
        instance: value
            .get("instance")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        expected_profile: value
            .get("expected_profile")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    }))
}

fn load_reconnect_receipt_event_at(
    path: &std::path::Path,
) -> Result<Option<ReconnectJournalEvent>> {
    Ok(load_reconnect_receipt_record_at(path)?.map(|record| record.event))
}

fn load_reconnect_receipt_event_with_timestamp_at(
    path: &std::path::Path,
) -> Result<Option<(ReconnectJournalEvent, u64)>> {
    Ok(load_reconnect_receipt_record_at(path)?.map(|record| (record.event, record.timestamp)))
}

fn load_reconnect_receipt_at(path: &std::path::Path) -> Result<Option<ReconnectAck>> {
    Ok(
        load_reconnect_receipt_event_at(path)?.and_then(|event| match event {
            ReconnectJournalEvent::Settled { ack }
            | ReconnectJournalEvent::Cancelled { ack }
            | ReconnectJournalEvent::Superseded { ack, .. }
            | ReconnectJournalEvent::Expired { ack } => Some(ack),
            ReconnectJournalEvent::Submitted { .. }
            | ReconnectJournalEvent::Claimed { .. }
            | ReconnectJournalEvent::InProgress { .. } => None,
        }),
    )
}

fn reconnect_receipt_sync_directories(parent: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut directories = vec![parent.to_path_buf()];
    if let Some(control_root) = parent
        .parent()
        .filter(|path| !path.as_os_str().is_empty() && *path != parent)
    {
        directories.push(control_root.to_path_buf());
    }
    directories
}

fn sync_reconnect_receipt_directories_at(parent: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    for directory in reconnect_receipt_sync_directories(parent) {
        std::fs::File::open(&directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                reconnect_journal_error(format!(
                    "cannot make reconnect receipt directory durable {}: {error}",
                    directory.display()
                ))
            })?;
    }
    Ok(())
}

fn sync_hidden_reconnect_proof_directories_at(parent: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        let mut synced_existing_directory = false;
        for directory in reconnect_receipt_sync_directories(parent) {
            let file = match std::fs::File::open(&directory) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(reconnect_journal_error(format!(
                        "cannot open reconnect proof directory for durable quarantine {}: {error}",
                        directory.display()
                    )));
                }
            };
            file.sync_all().map_err(|error| {
                reconnect_journal_error(format!(
                    "cannot make reconnect proof quarantine durable {}: {error}",
                    directory.display()
                ))
            })?;
            synced_existing_directory = true;
        }
        if !synced_existing_directory {
            return Err(reconnect_journal_error(
                "cannot prove reconnect success proof absence without an existing control directory",
            ));
        }
    }
    Ok(())
}

fn sync_reconnect_receipt_at(path: &std::path::Path, parent: &std::path::Path) -> Result<()> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            reconnect_journal_error(format!(
                "cannot make reconnect receipt durable {}: {error}",
                path.display()
            ))
        })?;
    sync_reconnect_receipt_directories_at(parent)
}

fn preserve_reconnect_receipt_at_with<F>(
    path: &std::path::Path,
    ack: &ReconnectAck,
    mut sync_published: F,
) -> Result<()>
where
    F: FnMut(&std::path::Path, &std::path::Path) -> Result<()>,
{
    let parent = path
        .parent()
        .ok_or_else(|| reconnect_journal_error("reconnect receipt has no control directory"))?;
    if let Some(existing) = load_reconnect_receipt_at(path)? {
        return if existing == *ack {
            sync_published(path, parent)
        } else {
            Err(reconnect_journal_error(format!(
                "reconnect receipt {} conflicts with its terminal result",
                path.display()
            )))
        };
    }
    std::fs::create_dir_all(parent).map_err(|error| {
        reconnect_journal_error(format!(
            "cannot create reconnect receipt directory {}: {error}",
            parent.display()
        ))
    })?;
    let temp = parent.join(format!(".receipt-{}.tmp", control_nonce()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            reconnect_journal_error(format!(
                "cannot stage reconnect receipt {}: {error}",
                temp.display()
            ))
        })?;
    let bytes =
        reconnect_event_value(&ReconnectJournalEvent::Settled { ack: ack.clone() }).to_string();
    use std::io::Write;
    if let Err(error) = file
        .write_all(bytes.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&temp);
        return Err(reconnect_journal_error(format!(
            "cannot make reconnect receipt durable {}: {error}",
            temp.display()
        )));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(reconnect_journal_error(format!(
            "cannot publish reconnect receipt {}: {error}",
            path.display()
        )));
    }
    sync_published(path, parent)
}

fn preserve_reconnect_receipt_once_at(path: &std::path::Path, ack: &ReconnectAck) -> Result<()> {
    preserve_reconnect_receipt_at_with(path, ack, sync_reconnect_receipt_at)
}

fn preserve_reconnect_receipt_at(path: &std::path::Path, ack: &ReconnectAck) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        match preserve_reconnect_receipt_once_at(path, ack) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                    return Err(error);
                }
                last_error = Some(error);
                std::thread::sleep(RECONNECT_POLL);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Err(reconnect_journal_error(
            "reconnect receipt publication exhausted its retry budget",
        )),
    }
}

fn preserve_reconnect_terminal_event_once_at(
    path: &std::path::Path,
    event: &ReconnectJournalEvent,
    request: Option<&ReconnectRequest>,
) -> Result<()> {
    if !matches!(
        event,
        ReconnectJournalEvent::Settled { .. }
            | ReconnectJournalEvent::Cancelled { .. }
            | ReconnectJournalEvent::Superseded { .. }
            | ReconnectJournalEvent::Expired { .. }
    ) {
        return Err(reconnect_journal_error(
            "only a terminal event may be retained as a reconnect receipt",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| reconnect_journal_error("reconnect receipt has no control directory"))?;
    if let Some(existing) = load_reconnect_receipt_event_at(path)? {
        return if existing == *event {
            sync_reconnect_receipt_at(path, parent)
        } else {
            Err(reconnect_journal_error(format!(
                "reconnect receipt {} conflicts with its terminal result",
                path.display()
            )))
        };
    }
    std::fs::create_dir_all(parent).map_err(|error| {
        reconnect_journal_error(format!(
            "cannot create reconnect receipt directory {}: {error}",
            parent.display()
        ))
    })?;
    let temp = parent.join(format!(".receipt-{}.tmp", control_nonce()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| {
            reconnect_journal_error(format!(
                "cannot stage reconnect receipt {}: {error}",
                temp.display()
            ))
        })?;
    use std::io::Write;
    if let Err(error) = file
        .write_all(
            reconnect_event_value_with_context(event, request)
                .to_string()
                .as_bytes(),
        )
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&temp);
        return Err(reconnect_journal_error(format!(
            "cannot make reconnect receipt durable {}: {error}",
            temp.display()
        )));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(reconnect_journal_error(format!(
            "cannot publish reconnect receipt {}: {error}",
            path.display()
        )));
    }
    sync_reconnect_receipt_at(path, parent)
}

fn preserve_reconnect_terminal_event_at_with(
    path: &std::path::Path,
    event: &ReconnectJournalEvent,
    request: Option<&ReconnectRequest>,
) -> Result<()> {
    let mut last_error = None;
    for attempt in 0..=RECONNECT_HOUSEKEEPING_RETRIES {
        match preserve_reconnect_terminal_event_once_at(path, event, request) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == RECONNECT_HOUSEKEEPING_RETRIES {
                    return Err(error);
                }
                last_error = Some(error);
                std::thread::sleep(RECONNECT_POLL);
            }
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Err(reconnect_journal_error(
            "reconnect terminal publication exhausted its retry budget",
        )),
    }
}

fn reap_reconnect_receipt_at(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            #[cfg(unix)]
            if let Some(parent) = path.parent() {
                std::fs::File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| {
                        reconnect_journal_error(format!(
                            "reconnect receipt was reaped, but its directory was not durable {}: {error}",
                            parent.display()
                        ))
                    })?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(reconnect_journal_error(format!(
            "reconnect receipt could not be reaped {}: {error}",
            path.display()
        ))),
    }
}

fn reap_orphan_reconnect_success_proofs_at(journal: &std::path::Path) -> Result<()> {
    let Some(control_root) = journal.parent() else {
        return Err(reconnect_journal_error(
            "reconnect journal has no control directory",
        ));
    };
    let proof_dir = control_root.join("fleetyd.reconnect-success");
    let entries = match std::fs::read_dir(&proof_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(reconnect_journal_error(format!(
                "cannot inspect reconnect success proofs {}: {error}",
                proof_dir.display()
            )))
        }
    };
    let active = load_reconnect_journal_at(journal)?;
    let active_proof = active.as_ref().and_then(|state| match &state.phase {
        ReconnectPhase::Settled(ack) if ack.accepted => {
            reconnect_success_proof_path_at(journal, &ack.nonce).ok()
        }
        _ => None,
    });
    let receipt_dir = control_root.join("fleetyd.reconnect-receipts");
    for entry in entries {
        let path = entry
            .map_err(|error| {
                reconnect_journal_error(format!(
                    "cannot inspect reconnect success proof entry: {error}"
                ))
            })?
            .path();
        if !path.is_file() {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if file_name.starts_with(".receipt-") && file_name.ends_with(".tmp") {
            reap_reconnect_receipt_at(&path)?;
            continue;
        }
        let ack = match load_reconnect_receipt_at(&path) {
            Ok(Some(ack)) => ack,
            Ok(None) => {
                reap_reconnect_receipt_at(&path)?;
                continue;
            }
            Err(error) => {
                let has_carrier = active_proof.as_deref() == Some(path.as_path())
                    || receipt_dir.join(file_name).exists();
                if has_carrier {
                    return Err(error);
                }
                tracing::warn!(
                    proof = %path.display(),
                    report = ?error.report(),
                    "ambiguous reconnect success proof is retained for diagnosis"
                );
                continue;
            }
        };
        let active_carrier = active.as_ref().is_some_and(|state| {
            state.request.nonce == ack.nonce
                && matches!(state.phase, ReconnectPhase::Settled(ref active_ack) if active_ack == &ack)
        });
        let receipt = reconnect_receipt_path_at(journal, &ack.nonce)?;
        let receipt_carrier = load_reconnect_receipt_at(&receipt)?.as_ref() == Some(&ack);
        if !active_carrier && !receipt_carrier {
            tracing::warn!(
                proof = %path.display(),
                "orphan reconnect success proof is retained until retention cleanup"
            );
        }
    }
    Ok(())
}

fn take_reconnect_ack_at_with<FR, FJ>(
    journal: &std::path::Path,
    nonce: &str,
    _reap_receipt: FR,
    mut reap_journal: FJ,
) -> Result<Option<ReconnectAck>>
where
    FR: FnMut(&std::path::Path) -> Result<()>,
    FJ: FnMut(&std::path::Path) -> Result<()>,
{
    let receipt = reconnect_receipt_path_at(journal, nonce)?;
    if let Some(ack) = load_reconnect_receipt_at(&receipt)? {
        if ack.nonce != nonce {
            return Err(reconnect_journal_error(
                "receipt nonce does not match its requested result",
            ));
        }
        require_reconnect_success_proof(journal, &ack)?;
        // Observation is deliberately read-only for terminal carriers. The
        // retention command is the only path allowed to reap a receipt or a
        // success proof, so repeated status/observation remains possible.
        return Ok(Some(ack));
    }
    let Some(state) = load_reconnect_journal_at(journal)? else {
        return Ok(None);
    };
    if state.request.nonce != nonce {
        return Ok(None);
    }
    let terminal_event = reconnect_terminal_event_from_phase(&state.phase);
    match state.phase {
        ReconnectPhase::Settled(ack)
        | ReconnectPhase::Cancelled(ack)
        | ReconnectPhase::Superseded { ack, .. }
        | ReconnectPhase::Expired(ack) => {
            if ack.accepted {
                require_reconnect_success_proof(journal, &ack)?;
            }
            let event = terminal_event.as_ref().ok_or_else(|| {
                reconnect_journal_error("terminal reconnect state has no terminal event")
            })?;
            preserve_reconnect_terminal_event_at_with(&receipt, event, Some(&state.request))?;
            match reap_journal(journal) {
                Ok(()) => {}
                Err(error) => {
                    tracing::warn!(
                        report = ?error.report(),
                        "reconnect result was observed, but its journal cleanup needs retry"
                    );
                }
            }
            Ok(Some(ack))
        }
        ReconnectPhase::Submitted | ReconnectPhase::Claimed => Ok(None),
    }
}

fn take_reconnect_ack_at(journal: &std::path::Path, nonce: &str) -> Result<Option<ReconnectAck>> {
    take_reconnect_ack_at_with(
        journal,
        nonce,
        reap_reconnect_receipt_at,
        reap_reconnect_journal_at,
    )
}

struct ControlGuard {
    ready: ControlReady,
    _process_identity: fleety_tools::service::ProcessIdentityGuard,
}

impl ControlGuard {
    fn claim() -> Result<Self> {
        let process_start = control_nonce();
        let process_identity = fleety_tools::service::claim_process_identity_at(
            &process_identity_path(&process_start),
            &process_start,
        )?;
        let ready = ControlReady {
            pid: std::process::id(),
            process_start,
            instance: control_nonce(),
        };
        // Ready ownership, old-generation recovery, and publication are one
        // generation handoff. Two starters must never inspect/remove ready
        // outside the same reconnect lease.
        let _reconnect_lease = acquire_reconnect_lease()?;
        let path = ready_path();
        if let Ok(bytes) = std::fs::read(&path) {
            match parse_ready_record(&bytes)? {
                ControlReadyRecord::Current(existing) => {
                    let existing_identity = process_identity_path(&existing.process_start);
                    match fleety_tools::service::probe_process_identity_at(&existing_identity)? {
                        fleety_tools::service::ProcessIdentityState::Available => {
                            let _ = std::fs::remove_file(&path);
                            let _ = std::fs::remove_file(existing_identity);
                        }
                        fleety_tools::service::ProcessIdentityState::Held => {
                            return Err(agent_core::CoreError::Message(format!(
                                "another fleetyd process owns local reconnect control (pid {})",
                                existing.pid
                            )));
                        }
                    }
                }
                ControlReadyRecord::Legacy { pid, .. } => {
                    match fleety_tools::service::probe_pid(pid) {
                        fleety_tools::service::PidState::Dead => {
                            let _ = std::fs::remove_file(&path);
                        }
                        fleety_tools::service::PidState::Alive
                        | fleety_tools::service::PidState::Unknown => {
                            return Err(agent_core::CoreError::Message(format!(
                                "running fleetyd uses an incompatible legacy reconnect control contract (pid {pid}); update fleetyd and the Fleety CLI together, restart fleetyd, then retry"
                            )));
                        }
                    }
                }
            }
        }
        // Serialize generation handoff with publishers: a request can bind to
        // either the old owner (then recovery settles it) or this new owner,
        // never to an unpublished generation between the two.
        recover_reconnect_for_instance_at(&reconnect_journal_path(), &ready.instance)?;
        reap_orphan_reconnect_success_proofs_at(&reconnect_journal_path())?;
        publish_ready_at(&path, &ready)?;
        Ok(Self {
            ready,
            _process_identity: process_identity,
        })
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
            .is_some_and(|ready| ready == self.ready);
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
                    if let Err(error) = settle_pending_reconnect_bounded(&mut pending) {
                        tracing::error!(
                            report = ?error.report(),
                            "cannot persist reconnect rejection after bounded retries; inspect the retained request"
                        );
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
                if let Err(error) = settle_pending_reconnect_bounded(&mut pending) {
                    tracing::error!(
                        report = ?error.report(),
                        "cannot persist reconnect rejection after bounded retries; inspect the retained request"
                    );
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
        Ok(bytes) => match parse_ready_record(&bytes)? {
            ControlReadyRecord::Current(ready) => ready,
            ControlReadyRecord::Legacy { .. } => {
                return Err(agent_core::CoreError::Message(
                    "running fleetyd uses an incompatible legacy reconnect control contract; update fleetyd and the Fleety CLI together, restart fleetyd, then retry"
                        .to_string(),
                ))
            }
        },
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
    if fleety_tools::service::probe_process_identity_at(&process_identity_path(
        &ready.process_start,
    ))? != fleety_tools::service::ProcessIdentityState::Held
    {
        return Err(agent_core::CoreError::Message(
            "fleetyd reconnect control owner is stale; restart fleetyd, then retry".to_string(),
        ));
    }
    let request = ReconnectRequest {
        instance: ready.instance,
        nonce: control_nonce(),
        expected_profile: expected_profile.to_string(),
    };
    let journal = reconnect_journal_path();
    if let Some(state) = load_reconnect_journal_at(&journal)? {
        if let Some(event) = reconnect_terminal_event_from_phase(&state.phase) {
            let ack = match &event {
                ReconnectJournalEvent::Settled { ack }
                | ReconnectJournalEvent::Cancelled { ack }
                | ReconnectJournalEvent::Superseded { ack, .. }
                | ReconnectJournalEvent::Expired { ack } => ack,
                ReconnectJournalEvent::Submitted { .. }
                | ReconnectJournalEvent::Claimed { .. }
                | ReconnectJournalEvent::InProgress { .. } => {
                    return Err(reconnect_journal_error("terminal event conversion failed"))
                }
            };
            if ack.accepted {
                require_reconnect_success_proof(&journal, ack)?;
            }
            let receipt = reconnect_receipt_path_at(&journal, &ack.nonce)?;
            preserve_reconnect_terminal_event_at_with(&receipt, &event, Some(&state.request))?;
            reap_reconnect_journal_at(&journal)?;
        }
    }
    submit_reconnect_at(&journal, &request)?;
    drop(lease);

    let deadline = std::time::Instant::now() + wait;
    while std::time::Instant::now() < deadline {
        let receipt = reconnect_receipt_path_at(&journal, &request.nonce)?;
        let active_is_settled = load_reconnect_journal_at(&journal)?.is_some_and(|state| {
            state.request.nonce == request.nonce
                && matches!(state.phase, ReconnectPhase::Settled(_))
        });
        if receipt.exists() || active_is_settled {
            let _observe_lease = acquire_reconnect_lease()?;
            if let Some(observed_ack) = take_reconnect_ack_at(&journal, &request.nonce)? {
                return if observed_ack.accepted {
                    Ok(observed_ack.message)
                } else {
                    Err(agent_core::CoreError::Message(observed_ack.message))
                };
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

fn reconnect_lifecycle_label(state: &ReconnectLifecycle) -> &'static str {
    match state {
        ReconnectLifecycle::Submitted => "submitted",
        ReconnectLifecycle::InProgress => "in-progress",
        ReconnectLifecycle::Settled => "settled",
        ReconnectLifecycle::Cancelled => "cancelled",
        ReconnectLifecycle::Superseded => "superseded",
        ReconnectLifecycle::Expired => "expired",
        ReconnectLifecycle::Ambiguous => "ambiguous",
    }
}

fn reconnect_status_value(status: &ReconnectStatus) -> serde_json::Value {
    serde_json::json!({
        "nonce": status.nonce,
        "profile": status.profile,
        "owner": status.owner,
        "state": reconnect_lifecycle_label(&status.state),
        "submitted_at": status.submitted_at,
        "claimed_at": status.claimed_at,
        "terminal_at": status.terminal_at,
        "retention_expires_at": status.retention_expires_at,
        "replacement_nonce": status.replacement_nonce,
        "terminal_result": status.terminal_result.as_ref().map(|ack| serde_json::json!({
            "accepted": ack.accepted,
            "message": ack.message,
        })),
        "detail": status.detail,
    })
}

fn print_reconnect_status(status: &ReconnectStatus, machine: bool) {
    if machine {
        println!("{}", reconnect_status_value(status));
        return;
    }
    println!("nonce: {}", status.nonce);
    println!("state: {}", reconnect_lifecycle_label(&status.state));
    if !status.profile.is_empty() {
        println!("profile: {}", status.profile);
    }
    if !status.owner.is_empty() {
        println!("owner: {}", status.owner);
    }
    println!("submitted_at: {}", status.submitted_at);
    if let Some(claimed_at) = status.claimed_at {
        println!("claimed_at: {claimed_at}");
    }
    if let Some(terminal_at) = status.terminal_at {
        println!("terminal_at: {terminal_at}");
    }
    if let Some(expires_at) = status.retention_expires_at {
        println!("retention_expires_at: {expires_at}");
    }
    if let Some(replacement) = &status.replacement_nonce {
        println!("replacement_nonce: {replacement}");
    }
    if let Some(result) = &status.terminal_result {
        println!("result: {}", result.message);
    }
    if let Some(detail) = &status.detail {
        println!("detail: {detail}");
    }
}

fn current_reconnect_owner_instance() -> Result<String> {
    let ready = match std::fs::read(ready_path()) {
        Ok(bytes) => match parse_ready_record(&bytes)? {
            ControlReadyRecord::Current(ready) => ready,
            ControlReadyRecord::Legacy { .. } => {
                return Err(agent_core::CoreError::Message(
                    "reconnect control is using an incompatible legacy contract; update fleetyd and restart it"
                        .to_string(),
                ))
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(agent_core::CoreError::Message(
                "no running fleetyd reconnect owner was found".to_string(),
            ))
        }
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot inspect fleetyd reconnect owner: {error}"
            )))
        }
    };
    if fleety_tools::service::probe_process_identity_at(&process_identity_path(
        &ready.process_start,
    ))? != fleety_tools::service::ProcessIdentityState::Held
    {
        return Err(agent_core::CoreError::Message(
            "reconnect control owner is stale; inspect control ownership before mutating it"
                .to_string(),
        ));
    }
    Ok(ready.instance)
}

fn inspect_reconnect_control_value() -> Result<serde_json::Value> {
    let ready = match std::fs::read(ready_path()) {
        Ok(bytes) => Some(parse_ready_record(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot read reconnect control identity: {error}"
            )))
        }
    };
    let lock = match std::fs::read_to_string(reconnect_lock_path()) {
        Ok(owner) => Some(owner),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot read reconnect control lease: {error}"
            )))
        }
    };
    let active_nonce =
        load_reconnect_journal_at(&reconnect_journal_path())?.map(|state| state.request.nonce);
    let ready_value = ready.as_ref().map(|record| match record {
        ControlReadyRecord::Current(ready) => serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "pid": ready.pid,
            "process_start": ready.process_start,
            "instance": ready.instance,
            "owner_generation": ready.instance,
            "process_state": match fleety_tools::service::probe_process_identity_at(&process_identity_path(&ready.process_start)) {
                Ok(fleety_tools::service::ProcessIdentityState::Held) => "held",
                Ok(fleety_tools::service::ProcessIdentityState::Available) => "dead",
                Err(_) => "unknown",
            },
        }),
        ControlReadyRecord::Legacy { pid, instance } => serde_json::json!({
            "version": "legacy",
            "pid": pid,
            "instance": instance,
            "owner_generation": instance,
            "process_state": "legacy",
        }),
    });
    Ok(serde_json::json!({
        "ready": ready_value,
        "lease_owner": lock,
        "nonce": active_nonce,
        "age_millis": ready_path().metadata().ok().and_then(|metadata| metadata.modified().ok()).and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok()).map(|at| current_epoch_millis().saturating_sub(at.as_millis() as u64)),
    }))
}

fn current_epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn recover_stale_reconnect_control(confirm: bool) -> Result<String> {
    let ready_bytes = match std::fs::read(ready_path()) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("reconnect control is already clear".to_string())
        }
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "cannot inspect reconnect control before recovery: {error}"
            )))
        }
    };
    let ready = match parse_ready_record(&ready_bytes)? {
        ControlReadyRecord::Current(ready) => ready,
        ControlReadyRecord::Legacy { .. } => return Err(agent_core::CoreError::Message(
            "legacy reconnect control cannot be safely recovered; update fleetyd and restart it"
                .to_string(),
        )),
    };
    let process_state = fleety_tools::service::probe_process_identity_at(&process_identity_path(
        &ready.process_start,
    ))?;
    if process_state == fleety_tools::service::ProcessIdentityState::Held {
        return Err(agent_core::CoreError::Message(
            "reconnect control owner is live; recovery refused".to_string(),
        ));
    }
    if !confirm {
        return Err(agent_core::CoreError::Message(
            "stale-control recovery requires --confirm after `fleetyd reconnect control inspect`"
                .to_string(),
        ));
    }
    if matches!(
        fleety_tools::service::probe_pid(ready.pid),
        fleety_tools::service::PidState::Alive
    ) {
        return Err(agent_core::CoreError::Message(
            "recorded pid is live but its process-start identity does not match; recovery refused"
                .to_string(),
        ));
    }
    // A stale control record is not enough evidence to delete the generation:
    // the journal must also be readable under this binary's control version.
    // Otherwise recovery would destroy the only evidence needed to diagnose a
    // mixed-version or conflicting reconnect state.
    let _ = load_reconnect_journal_at(&reconnect_journal_path())?;
    if let Ok(owner) = std::fs::read_to_string(reconnect_lock_path()) {
        let Some(pid) = owner.split(':').next().and_then(|value| value.parse().ok()) else {
            return Err(agent_core::CoreError::Message(
                "a successor reconnect lease has no verifiable process owner; recovery refused"
                    .to_string(),
            ));
        };
        if matches!(
            fleety_tools::service::probe_pid(pid),
            fleety_tools::service::PidState::Alive | fleety_tools::service::PidState::Unknown
        ) {
            return Err(agent_core::CoreError::Message(
                "a live or unverifiable successor owns the reconnect lease; recovery refused"
                    .to_string(),
            ));
        }
        if std::fs::read_to_string(reconnect_lock_path())
            .ok()
            .as_deref()
            != Some(owner.as_str())
        {
            return Err(agent_core::CoreError::Message(
                "the reconnect lease changed during stale-control inspection; recovery refused"
                    .to_string(),
            ));
        }
        std::fs::remove_file(reconnect_lock_path()).map_err(|error| {
            agent_core::CoreError::Message(format!(
                "stale reconnect lease could not be removed: {error}"
            ))
        })?;
    }
    // Serialize the final evidence check with a successor's ControlGuard::claim.
    // If a successor wins the lease between removing a dead lock and this
    // acquisition, acquisition times out and the ready record is preserved.
    let _recovery_lease = acquire_reconnect_lease()?;
    let current_ready = match parse_ready_record(&std::fs::read(ready_path()).map_err(
        |error| {
            agent_core::CoreError::Message(format!(
                "stale reconnect ready record changed during recovery: {error}"
            ))
        },
    )?)? {
        ControlReadyRecord::Current(current) => current,
        ControlReadyRecord::Legacy { .. } => return Err(agent_core::CoreError::Message(
            "reconnect control changed to a legacy contract during recovery; artifacts preserved"
                .to_string(),
        )),
    };
    if current_ready != ready
        || fleety_tools::service::probe_process_identity_at(&process_identity_path(
            &current_ready.process_start,
        ))? == fleety_tools::service::ProcessIdentityState::Held
        || matches!(
            fleety_tools::service::probe_pid(current_ready.pid),
            fleety_tools::service::PidState::Alive
        )
    {
        return Err(agent_core::CoreError::Message(
            "reconnect control changed or became live during recovery; artifacts preserved"
                .to_string(),
        ));
    }
    std::fs::remove_file(ready_path()).map_err(|error| {
        agent_core::CoreError::Message(format!(
            "stale reconnect ready record could not be removed: {error}"
        ))
    })?;
    let identity = process_identity_path(&ready.process_start);
    match std::fs::remove_file(&identity) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(agent_core::CoreError::Message(format!(
                "stale process-start proof could not be removed: {error}"
            )))
        }
    }
    Ok(format!(
        "recovered stale reconnect owner pid={} process_start={} instance={}",
        ready.pid, ready.process_start, ready.instance
    ))
}

fn reap_expired_reconnect_records() -> Result<usize> {
    let _lease = acquire_reconnect_lease()?;
    let owner = current_reconnect_owner_instance()?;
    reap_expired_reconnect_records_at(&reconnect_journal_path(), &owner, current_epoch_millis())
}

/// Return the exact persisted profile that owns this resolved target. A fresh
/// local/default connection may create `default`; raw URL and environment
/// overrides deliberately have no persisted owner.
#[cfg(test)]
fn target_profile_name(
    conns: &connection::Connections,
    target: &Resolved,
    allow_default: bool,
) -> Option<String> {
    if let Some(name) = target.profile_owner_name() {
        return connection::validate_resolved_profile_owner(conns, target, "profile lookup")
            .ok()
            .map(|_| name.to_string());
    }
    if allow_default
        && target.can_create_fresh_default_owner()
        && conns.current.is_none()
        && conns.profiles.is_empty()
    {
        Some("default".to_string())
    } else {
        None
    }
}

/// Confirm that the resolved target is still the exact persisted owner snapshot
/// selected by this reconnect. Re-run this before terminal success so a later
/// profile mutation cannot make one nonce acknowledge another owner.
#[cfg(test)]
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
    let Some(source_profile) = target.profile_owner_name() else {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' did not resolve to a persisted owner"
        )));
    };
    if source_profile != expected_profile {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect requested profile '{expected_profile}', but resolved profile '{source_profile}'"
        )));
    }
    let profile =
        connection::validate_resolved_profile_owner(conns, target, "reconnect owner validation")?;
    Ok(profile.fingerprint.clone())
}

fn reconnect_owner_fingerprint_in(
    conns: &connection::Connections,
    expected_profile: &str,
    target: &Resolved,
    owner: Option<&SessionCredentialOwner>,
) -> Result<Option<String>> {
    let Some(source_profile) = target.profile_owner_name() else {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' did not resolve to a persisted owner"
        )));
    };
    let Some(SessionCredentialOwner::Existing(owner_target)) = owner else {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect profile '{expected_profile}' has no frozen credential owner"
        )));
    };
    if source_profile != expected_profile
        || owner_target.profile_owner_name() != Some(expected_profile)
        || owner_target.as_ref() != target
    {
        return Err(agent_core::CoreError::Message(format!(
            "reconnect requested profile '{expected_profile}', but resolved profile '{source_profile}'"
        )));
    }
    let profile = connection::validate_resolved_profile_owner(
        conns,
        owner_target,
        "reconnect owner validation",
    )?;
    Ok(profile.fingerprint.clone())
}

/// Atomically persist a normal authenticated session's minted token and
/// identity pin onto its exact durable owner. Ephemeral explicit targets remain
/// unpersisted.
#[derive(Debug, Clone)]
enum SessionCredentialOwner {
    Existing(Box<Resolved>),
    FreshDefault,
}

fn session_credential_owner_in(
    conns: &connection::Connections,
    target: &Resolved,
) -> Option<SessionCredentialOwner> {
    if target.has_profile_owner()
        && connection::validate_resolved_profile_owner(conns, target, "session owner capture")
            .is_ok()
    {
        return Some(SessionCredentialOwner::Existing(Box::new(target.clone())));
    }
    (target.can_create_fresh_default_owner()
        && conns.current.is_none()
        && conns.profiles.is_empty())
    .then_some(SessionCredentialOwner::FreshDefault)
}

#[cfg(test)]
fn session_credential_owner(target: &Resolved) -> Result<Option<SessionCredentialOwner>> {
    Ok(session_credential_owner_in(&connection::load()?, target))
}

fn persist_authenticated_target_credentials(
    owner: &SessionCredentialOwner,
    target: &Resolved,
    server_fingerprint: &str,
    minted_token: Option<&str>,
    server_endpoints: &[String],
    sealed: bool,
) -> Result<bool> {
    if server_fingerprint.trim().is_empty()
        || minted_token.is_some_and(|token| token.trim().is_empty())
    {
        return Err(agent_core::CoreError::Message(
            "fleetyd received incomplete authenticated credentials".to_string(),
        ));
    }
    if let SessionCredentialOwner::Existing(owner_target) = owner {
        if owner_target.as_ref() != target {
            return Err(agent_core::CoreError::Message(
                "authenticated session owner does not match the resolved target".to_string(),
            ));
        }
        let (_, committed) = connection::store_resolved_profile_credentials(
            owner_target,
            minted_token,
            server_fingerprint,
        )?;
        if let Err(error) = connection::learn_resolved_profile_endpoints(
            &committed,
            server_fingerprint,
            server_endpoints,
            sealed,
        ) {
            tracing::warn!(
                report = ?error.report(),
                "could not record what this session learned about the profile"
            );
        }
        return Ok(true);
    }
    connection::mutate(|conns| {
        if conns.device_id.is_empty() {
            conns.device_id = fleety_tools::device::device_id();
        }
        if conns.current.is_some() || !conns.profiles.is_empty() {
            return Err(agent_core::CoreError::Message(
                "connection profiles changed before the fresh authenticated owner committed"
                    .to_string(),
            ));
        }
        let name = "default".to_string();
        let profile = conns.profiles.entry(name.clone()).or_default();
        profile.url = target.url_owned();
        // Only a sealed session may teach a profile addresses; this fresh-default
        // enrollment has none yet.
        profile.endpoints.clear();
        conns.current = Some(name.clone());
        match connection::tofu_pin_decision(profile.fingerprint.as_deref(), server_fingerprint) {
            connection::PinDecision::IdentityChanged => {
                return Err(agent_core::CoreError::Message(format!(
                    "profile '{name}' has a different Server identity"
                )))
            }
            connection::PinDecision::Pin => {
                profile.fingerprint = Some(server_fingerprint.to_string());
            }
            connection::PinDecision::AlreadyPinned => {}
        }
        if let Some(token) = minted_token {
            profile.token = Some(token.to_string());
        }
        connection::rebind_profile_generation_after_authorized_mutation(profile)?;
        let _ = name;
        Ok(true)
    })
}

/// Persist a freshly-minted token only onto the profile that owned the resolved
/// connection. Returns false when the target is intentionally ephemeral.
#[cfg(test)]
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
            profile.url = target.url_owned();
        }
        profile.token = Some(token.to_string());
        if first_profile {
            conns.current = Some(name);
        }
        Ok(true)
    })
}

fn clear_session_owner_token(
    owner: Option<&SessionCredentialOwner>,
    target: &Resolved,
) -> Result<bool> {
    let Some(SessionCredentialOwner::Existing(owner_target)) = owner else {
        return Ok(false);
    };
    if owner_target.as_ref() != target {
        return Ok(false);
    }
    connection::clear_resolved_profile_token(owner_target)
}

/// Clear only the token that was actually sent to the rejecting target.
#[cfg(test)]
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
        if profile.token.as_deref() != target.token() || target.token().is_none() {
            return Ok(false);
        }
        profile.token = None;
        Ok(true)
    })
}

#[cfg(test)]
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
    let lifecycle = |name: &'static str| {
        Command::new(name).about(fleety_tools::service::lifecycle_about(name, "Daemon"))
    };
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
            Command::new("install").about(
                "Install the Daemon as a system service and provision the data-analysis sidecar",
            ),
            lifecycle("uninstall"),
            lifecycle("start"),
            lifecycle("stop"),
            lifecycle("restart"),
            lifecycle("enable"),
            lifecycle("disable"),
            lifecycle("status"),
            Command::new("reconnect")
                .about("Reconnect the running Daemon or inspect its durable request lifecycle")
                .arg(
                    Arg::new("profile")
                        .long("profile")
                        .required(false)
                        .value_name("NAME"),
                )
                .subcommands([
                    Command::new("status")
                        .about("Show a nonce-addressed reconnect lifecycle")
                        .arg(
                            Arg::new("nonce")
                                .long("nonce")
                                .required(true)
                                .value_name("NONCE"),
                        )
                        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                    Command::new("cancel")
                        .about("Cancel an owned reconnect before authenticated success")
                        .arg(
                            Arg::new("nonce")
                                .long("nonce")
                                .required(true)
                                .value_name("NONCE"),
                        ),
                    Command::new("supersede")
                        .about("Supersede an owned reconnect with a new nonce")
                        .arg(
                            Arg::new("nonce")
                                .long("nonce")
                                .required(true)
                                .value_name("NONCE"),
                        )
                        .arg(
                            Arg::new("replacement")
                                .long("replacement")
                                .required(true)
                                .value_name("NONCE"),
                        ),
                    Command::new("control")
                        .about("Inspect or safely recover reconnect control ownership")
                        .subcommands([
                            Command::new("inspect")
                                .arg(Arg::new("json").long("json").action(ArgAction::SetTrue)),
                            Command::new("recover").arg(
                                Arg::new("confirm")
                                    .long("confirm")
                                    .action(ArgAction::SetTrue),
                            ),
                        ]),
                    Command::new("retention")
                        .about("Reap complete terminal records past the retention deadline")
                        .arg(Arg::new("reap").long("reap").action(ArgAction::SetTrue)),
                ]),
            Command::new("update").about("Update the fleetyd binary, then restart the service"),
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
    fleety_tools::config::seed_env_from_config(
        &fleety_tools::config::load(&fleety_tools::config::config_path()),
        fleety_tools::config::DAEMON_SCOPES,
    );
    // One-time, idempotent migration of the legacy config.json / fleetyd.token
    // into connections.toml. An incompatible or unwritable store is an
    // actionable startup failure, never a reason to enter the operational
    // connection loop with an unverified profile snapshot.
    if let Err(error) = connection::migrate_from_config_json() {
        let report = error.report();
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
        let action = args.get(1).map(String::as_str);
        let operation = match action {
            Some("status") => {
                let nonce = args
                    .windows(2)
                    .find(|pair| pair[0] == "--nonce")
                    .map(|pair| pair[1].as_str())
                    .unwrap_or_default();
                let machine = args.iter().any(|arg| arg == "--json");
                let result =
                    reconnect_status_at(&reconnect_journal_path(), nonce).and_then(|status| {
                        status.ok_or_else(|| {
                            agent_core::CoreError::Message(format!(
                                "reconnect request '{nonce}' is unknown"
                            ))
                        })
                    });
                result.map(|status| {
                    print_reconnect_status(&status, machine);
                    String::new()
                })
            }
            Some("cancel") => {
                let nonce = args
                    .windows(2)
                    .find(|pair| pair[0] == "--nonce")
                    .map(|pair| pair[1].as_str())
                    .unwrap_or_default();
                let result = acquire_reconnect_lease().and_then(|_lease| {
                    let owner = current_reconnect_owner_instance()?;
                    cancel_reconnect_at(&reconnect_journal_path(), nonce, &owner)
                        .map(|ack| format!("cancelled reconnect request '{}'", ack.nonce))
                });
                result
            }
            Some("supersede") => {
                let nonce = args
                    .windows(2)
                    .find(|pair| pair[0] == "--nonce")
                    .map(|pair| pair[1].as_str())
                    .unwrap_or_default();
                let replacement = args
                    .windows(2)
                    .find(|pair| pair[0] == "--replacement")
                    .map(|pair| pair[1].as_str())
                    .unwrap_or_default();
                let result = acquire_reconnect_lease().and_then(|_lease| {
                    let owner = current_reconnect_owner_instance()?;
                    supersede_reconnect_at(&reconnect_journal_path(), nonce, replacement, &owner)
                        .map(|request| {
                            format!(
                                "superseded reconnect request '{}' with replacement '{}'",
                                nonce, request.nonce
                            )
                        })
                });
                result
            }
            Some("control") if args.get(2).map(String::as_str) == Some("inspect") => {
                inspect_reconnect_control_value().map(|value| {
                    if args.iter().any(|arg| arg == "--json") {
                        value.to_string()
                    } else {
                        format!("reconnect control: {value}")
                    }
                })
            }
            Some("control") if args.get(2).map(String::as_str) == Some("recover") => {
                recover_stale_reconnect_control(args.iter().any(|arg| arg == "--confirm"))
            }
            Some("retention") => {
                if !args.iter().any(|arg| arg == "--reap") {
                    Err(agent_core::CoreError::Message(
                        "retention cleanup is destructive; pass --reap after inspecting status"
                            .to_string(),
                    ))
                } else {
                    reap_expired_reconnect_records()
                        .map(|count| format!("reaped {count} expired reconnect record(s)"))
                }
            }
            _ => {
                let profile = args
                    .windows(2)
                    .find(|pair| pair[0] == "--profile")
                    .map(|pair| pair[1].as_str())
                    .unwrap_or_default();
                if profile.is_empty() {
                    Err(agent_core::CoreError::Message(
                        "reconnect requires --profile NAME or a lifecycle subcommand".to_string(),
                    ))
                } else {
                    request_running_daemon_reconnect(profile)
                }
            }
        };
        return match operation {
            Ok(message) => {
                if !message.is_empty() {
                    println!("{message}");
                }
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
        return match winsvc::dispatch() {
            Ok(0) => std::process::ExitCode::SUCCESS,
            Ok(_) => std::process::ExitCode::FAILURE,
            Err(e) => {
                tracing::error!(
                    %e,
                    "windows service dispatcher failed; `run-service` only works when started \
                     by the Service Control Manager (use `fleetyd start` after `fleetyd install`)"
                );
                std::process::ExitCode::FAILURE
            }
        };
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

    // Reconnect control is the atomic outer owner. Claim it before a service
    // pidfile so two starters cannot race a dead-owner pidfile takeover.
    let control = match ControlGuard::claim() {
        Ok(control) => control,
        Err(e) => {
            tracing::error!(report = ?e.report(), "cannot claim fleetyd reconnect control; exiting");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Service mode (non-Windows run-service) also claims the single-instance
    // pidfile (defense-in-depth on top of the manager). Foreground dev runs do
    // not, and Windows claims it in winsvc after reconnect control.
    let service_mode = cmd.as_deref() == Some("run-service");
    let _pid_guard = if service_mode {
        match fleety_tools::service::acquire("fleetyd") {
            Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
            Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
                tracing::error!(pid, "another fleetyd is already running; exiting");
                return std::process::ExitCode::FAILURE;
            }
            Err(e) => {
                tracing::error!(report = ?e.report(), "cannot claim fleetyd service ownership; exiting");
                return std::process::ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    tracing::info!(version = agent_core::VERSION, "fleetyd starting");
    if let Err(e) = run(None, control).await {
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
/// profile > trusted localhost. mDNS is browsed only to direct an unconfigured
/// device into explicit `fleety init` selection; it never becomes a daemon
/// connection target. The daemon has no per-invocation override.
fn local_server_url() -> String {
    let port = std::env::var("FLEETY_ADDR")
        .ok()
        .and_then(|address| address.rsplit(':').next().map(String::from))
        .and_then(|port| port.trim().parse::<u16>().ok())
        .unwrap_or(8787);
    format!("ws://127.0.0.1:{port}")
}

fn resolve_target_with_owner() -> Result<(connection::Resolved, Option<SessionCredentialOwner>)> {
    let env_url = std::env::var("FLEETY_AGENT_URL")
        .ok()
        .filter(|url| !url.is_empty());
    let env_token = std::env::var("FLEETY_TOKEN").ok();
    connection::ensure_resolvable_profile_generation(&Target::Current, env_url.is_some())?;
    let conns = connection::load()?;
    let target = connection::resolve(&conns, &Target::Current, env_url, env_token, || {
        connection::prefer_trusted_local_candidate(
            || {
                connection::trusted_local_server_up(
                    &local_server_url(),
                    std::time::Duration::from_millis(300),
                )
            },
            || {
                let discovered =
                    connection::discover_for_connections(&conns, std::time::Duration::from_secs(2));
                if let Some(server) = &discovered {
                    tracing::info!(url = %server.url, "discovered fleety server via mDNS");
                }
                discovered
            },
        )
    })?;
    let owner = session_credential_owner_in(&conns, &target);
    Ok((target, owner))
}

#[cfg(test)]
fn resolve_target() -> Result<connection::Resolved> {
    resolve_target_with_owner().map(|(target, _)| target)
}

fn reconnect_connect_error(target: &Resolved, cause: &str) -> String {
    let cause = fleety_tools::transport::redact_urls_in_text(cause);
    if matches!(
        target.source(),
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
async fn run(
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    control: ControlGuard,
) -> Result<()> {
    // Runtime work is allowed only after the caller holds every applicable
    // owner. Keeping reconnect control in this frame covers all spawned work.
    let control = Some(control);
    // Best-effort background update poller (no-op when the user hasn't set
    // FLEETY_UPDATE_MANIFEST — keeps the existing dev/install posture).
    poll_updates::spawn();
    // Ensure device dependencies in the background (best-effort, non-blocking).
    tokio::spawn(ensure_dependencies());
    let mut bo = backoff::Backoff::new();
    let mut pending_reconnect: Option<PendingReconnect> = None;
    loop {
        if pending_reconnect
            .as_ref()
            .is_some_and(|pending| pending.decision.is_some())
        {
            match settle_pending_reconnect_bounded(&mut pending_reconnect) {
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
        match resolve_target_with_owner() {
            Ok((target, session_owner)) => {
                // WebSocket first, SSE+POST fallback (unless overridden by env) —
                // so a device behind a proxy that blocks the WS upgrade connects.
                // The reconnect budget has to cover the candidate list, not just
                // one address: a single black-holed endpoint would otherwise
                // consume it all and settle a failure with working endpoints
                // still untried. A profile with one endpoint keeps the original
                // budget exactly.
                let attempts = target.connection_attempts();
                let attempt_count = attempts.len().max(1);
                let reconnect_deadline = pending_reconnect
                    .as_ref()
                    .map(|_| tokio::time::Instant::now() + RECONNECT_SWEEP_BUDGET);
                let mut last_error = None;
                // `serve` runs *inside* the candidate loop on purpose: an
                // endpoint has not earned the attempt until it has completed an
                // authenticated Welcome. One that accepts the socket and then
                // stalls, answers as a different Server, or refuses us must not
                // stand in for the endpoints behind it.
                let mut served = false;
                let mut requested_reconnect = None;
                for (tried_index, candidate) in attempts.into_iter().enumerate() {
                    // A previous candidate can consume enough time for another
                    // binary or pairing flow to replace this profile. Refuse
                    // the remaining frozen candidates before opening another
                    // transport with stale credential authority.
                    if let Err(error) =
                        connection::validate_resolved_profile_before_transport(&target)
                    {
                        last_error = Some(error);
                        break;
                    }
                    // Each candidate receives an equal share of the remaining
                    // whole-sweep deadline. A silent candidate therefore gives
                    // its unused time to later candidates and to settlement.
                    let share = reconnect_deadline
                        .map(|deadline| {
                            let now = tokio::time::Instant::now();
                            let remaining = deadline.saturating_duration_since(now);
                            let candidates_left =
                                (attempt_count.saturating_sub(tried_index)).max(1) as u32;
                            remaining / candidates_left
                        })
                        .unwrap_or(CONNECT_ENDPOINT_WAIT);
                    let candidate_deadline = match reconnect_deadline {
                        Some(deadline) => (tokio::time::Instant::now() + share).min(deadline),
                        None => tokio::time::Instant::now() + CONNECT_ENDPOINT_WAIT,
                    };
                    let opened = if let Some(deadline) = reconnect_deadline {
                        let slice = candidate_deadline;
                        tokio::select! {
                            result = tokio::time::timeout_at(
                                slice.min(deadline),
                                connection::open_candidate(
                                    &candidate,
                                    connection::open_budget_within(share),
                                ),
                            ) => match result {
                                Ok(result) => result,
                                Err(_) => {
                                    if tokio::time::Instant::now() >= deadline {
                                        break;
                                    }
                                    last_error = Some(agent_core::CoreError::Message(
                                        "the endpoint did not open within its share of the reconnect budget"
                                            .to_string(),
                                    ));
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
                            result = connection::open_candidate(
                                &candidate,
                                connection::open_budget_within(CONNECT_ENDPOINT_WAIT),
                            ) => result,
                            _ = wait_stop(shutdown.clone()) => return Ok(()),
                        }
                    };
                    let (conn, sealed) = match opened {
                        Ok(opened) => opened,
                        Err(error) => {
                            last_error = Some(error);
                            continue;
                        }
                    };
                    // The credential owner follows the endpoint that actually
                    // opened, not the one we resolved from.
                    let candidate_owner = match &session_owner {
                        Some(SessionCredentialOwner::Existing(_)) => Some(
                            SessionCredentialOwner::Existing(Box::new(candidate.clone())),
                        ),
                        Some(SessionCredentialOwner::FreshDefault) => {
                            Some(SessionCredentialOwner::FreshDefault)
                        }
                        None => None,
                    };
                    // Snapshot the reconnect decision so a candidate that fails
                    // before proving itself does not settle the request that a
                    // later candidate can still satisfy.
                    let decision_before = pending_reconnect
                        .as_ref()
                        .and_then(|pending| pending.decision.clone());
                    let mut authenticated = false;
                    let outcome = serve(
                        &candidate,
                        candidate_owner.as_ref(),
                        conn,
                        shutdown.clone(),
                        control.as_ref(),
                        &mut pending_reconnect,
                        candidate_deadline,
                        sealed,
                        &mut authenticated,
                    )
                    .await;
                    match outcome {
                        Outcome::Shutdown => return Ok(()),
                        Outcome::Reconnect(request) => {
                            bo.reset();
                            requested_reconnect = Some(request);
                            served = true;
                            break;
                        }
                        // Backoff resets only for an endpoint that authenticated,
                        // so a socket that opens and never proves itself cannot
                        // keep the retry loop hot.
                        Outcome::Disconnected if authenticated => {
                            bo.reset();
                            served = true;
                            tracing::info!("fleetyd disconnected; will reconnect");
                            break;
                        }
                        Outcome::Disconnected => {
                            // A candidate that got as far as freezing a result
                            // owns this request: its credential commit already
                            // happened, so the frozen outcome must survive to be
                            // settled or retried, not be rolled back by the loop.
                            let froze = pending_reconnect.as_ref().is_some_and(|pending| {
                                pending.authenticated.is_some()
                                    || pending
                                        .decision
                                        .as_ref()
                                        .is_some_and(|decision| decision.accepted)
                            });
                            if froze {
                                served = true;
                                break;
                            }
                            // Otherwise drop the decision so a later endpoint can
                            // still settle this request, but keep its reason: it
                            // is the actionable half, and the aggregated failure
                            // would otherwise say only that nothing worked.
                            let reason = pending_reconnect
                                .as_ref()
                                .and_then(|pending| pending.decision.as_ref())
                                .filter(|decision| !decision.accepted)
                                .map(|decision| decision.message.clone());
                            if let Some(pending) = pending_reconnect.as_mut() {
                                pending.decision = decision_before;
                            }
                            last_error = Some(agent_core::CoreError::Message(
                                reason.unwrap_or_else(|| {
                                    "the endpoint did not complete an authenticated handshake"
                                        .to_string()
                                }),
                            ));
                        }
                    }
                }
                if let Some(request) = requested_reconnect {
                    tracing::info!(profile = %request.expected_profile, "profile changed; reconnecting immediately");
                    pending_reconnect = Some(PendingReconnect::new(request));
                    continue;
                }
                if !served {
                    let message = last_error
                        .map(|error| error.report().message)
                        .unwrap_or_else(|| {
                            "the reconnect deadline expired before an endpoint connected"
                                .to_string()
                        });
                    let failure = reconnect_connect_error(&target, &message);
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
        if pending_reconnect
            .as_ref()
            .is_some_and(|pending| pending.decision.is_some())
        {
            if let Err(error) = settle_pending_reconnect_bounded(&mut pending_reconnect) {
                tracing::warn!(
                    report = ?error.report(),
                    "cannot persist reconnect settlement before backoff; retaining evidence for retry"
                );
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
#[allow(clippy::too_many_arguments)]
async fn serve(
    target: &Resolved,
    session_owner: Option<&SessionCredentialOwner>,
    mut conn: fleety_tools::transport::Connection,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
    control: Option<&ControlGuard>,
    pending_reconnect: &mut Option<PendingReconnect>,
    candidate_deadline: tokio::time::Instant,
    // `sealed`: this connection runs inside the encrypted channel, committed
    // with the endpoints so the profile refuses a downgrade next time.
    sealed: bool,
    // `authenticated`: set once the Server completes an authenticated
    // `Welcome`. The caller reads it to decide whether this endpoint earned the
    // attempt — one that never authenticates must not stand in for the
    // endpoints behind it.
    authenticated: &mut bool,
) -> Outcome {
    let url = target.url();
    // A pairing code has no handshake behind it, so it goes only to the address
    // a person configured — never to one roaming promoted on its own.
    let pairing_code = session_owner
        .filter(|_| target.url() == target.configured_url())
        .and_then(|_| {
            std::env::var("FLEETY_PAIRING_CODE")
                .ok()
                .filter(|s| !s.is_empty())
        });
    let expected_reconnect_fingerprint = match pending_reconnect
        .as_ref()
        .map(|pending| {
            connection::inspect_locked(|conns| {
                reconnect_owner_fingerprint_in(
                    conns,
                    &pending.request.expected_profile,
                    target,
                    session_owner,
                )
            })
        })
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
        token: target.token_owned(),
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
    let hello_result = tokio::select! {
        result = tokio::time::timeout_at(candidate_deadline, conn.send_text(hello)) => {
            match result {
                Ok(result) => result,
                Err(_) => {
                    decide_pending_reconnect(
                        pending_reconnect,
                        false,
                        "fleetyd could not send Hello before the candidate deadline".to_string(),
                    );
                    conn.close().await;
                    return Outcome::Disconnected;
                }
            }
        }
        _ = wait_stop(shutdown.clone()) => {
            if pending_reconnect.is_some() {
                decide_pending_reconnect(
                    pending_reconnect,
                    false,
                    "fleetyd stopped before the reconnect completed".to_string(),
                );
                settle_pending_reconnect_before_exit(pending_reconnect).await;
            }
            conn.close().await;
            return Outcome::Shutdown;
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
    let presence_enabled = colocation::presence_enabled();
    let mut presence_tick: Option<tokio::time::Interval> = None;
    // Always bounded. An owner-requested reconnect keeps its own tighter budget;
    // every other session still needs a limit, or one silent endpoint hides
    // every working endpoint behind it.
    let mut welcome_deadline = Some(candidate_deadline);
    let mut authenticated_welcome = false;
    loop {
        if pending_reconnect
            .as_ref()
            .is_some_and(|pending| pending.decision.is_some())
        {
            match settle_pending_reconnect_bounded(pending_reconnect) {
                Ok(true) => welcome_deadline = None,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        report = ?error.report(),
                        "cannot persist reconnect settlement; ordinary service remains available while evidence is retained"
                    );
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
            ServerMsg::Welcome { .. } if authenticated_welcome => {
                tracing::warn!(
                    "fleetyd received a duplicate Welcome after authentication; refusing the session"
                );
                conn.close().await;
                return Outcome::Disconnected;
            }
            ServerMsg::Welcome {
                session_id,
                token,
                server_version,
                server_fingerprint,
                server_endpoints,
                ..
            } => {
                if token
                    .as_deref()
                    .is_some_and(|token| token.trim().is_empty())
                {
                    decide_pending_reconnect(
                        pending_reconnect,
                        false,
                        "fleetyd received an empty credential from the selected profile"
                            .to_string(),
                    );
                    tracing::warn!(
                        "fleetyd received Welcome with an empty minted token; refusing the session"
                    );
                    conn.close().await;
                    return Outcome::Disconnected;
                }
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
                if pending_reconnect.is_some() {
                    let Some(fingerprint) = server_fingerprint
                        .as_deref()
                        .filter(|value| !value.is_empty())
                    else {
                        decide_pending_reconnect(
                            pending_reconnect,
                            false,
                            "fleetyd authenticated the selected endpoint, but it did not provide a Server identity"
                                .to_string(),
                        );
                        conn.close().await;
                        return Outcome::Disconnected;
                    };
                    if let Err(error) = commit_authenticated_reconnect(
                        target,
                        session_owner,
                        fingerprint,
                        token.as_deref(),
                        &server_endpoints,
                        sealed,
                        pending_reconnect,
                    ) {
                        decide_pending_reconnect(
                            pending_reconnect,
                            false,
                            format!(
                                "fleetyd could not persist authenticated credentials and commit the reconnect result: {}",
                                error.report().message
                            ),
                        );
                        conn.close().await;
                        return Outcome::Disconnected;
                    }
                    tracing::info!(
                        "fleetyd credentials persisted before reconnect success publication"
                    );
                } else {
                    // Outside an owner-requested reconnect there is no success
                    // settlement to publish, but the same identity rules still
                    // protect normal startup/session refresh.
                    if let Some(fp) = server_fingerprint.as_deref().filter(|f| !f.is_empty()) {
                        match session_owner {
                            Some(owner) => match persist_authenticated_target_credentials(
                                owner,
                                target,
                                fp,
                                token.as_deref(),
                                &server_endpoints,
                                sealed,
                            ) {
                                Ok(true) => tracing::info!(
                                    "fleetyd identity and token committed to their exact profile owner"
                                ),
                                Ok(false) => {}
                                Err(error) => {
                                    tracing::warn!(
                                        report = ?error.report(),
                                        "could not atomically persist fleetyd identity and token"
                                    );
                                    conn.close().await;
                                    return Outcome::Disconnected;
                                }
                            },
                            None => tracing::info!(
                                "fleetyd target is intentionally ephemeral; credentials were not persisted"
                            ),
                        }
                    } else if session_owner.is_some() {
                        tracing::warn!(
                            "fleetyd received Welcome without a Server identity for a durable profile owner; refusing the control session"
                        );
                        conn.close().await;
                        return Outcome::Disconnected;
                    }
                }
                tracing::info!(%session_id, "registered with agent");
                authenticated_welcome = true;
                *authenticated = true;
                // Welcome has arrived: the deadline has done its job. Leaving it
                // armed would cut the live session short.
                welcome_deadline = None;
                if presence_enabled {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                        colocation::interval_secs(),
                    ));
                    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    presence_tick = Some(interval);
                }
                // Forward-only fleet convergence: match this host to the server's
                // version when the server is newer (so a device that was offline
                // during a `fleety update` catches up on reconnect).
                converge_to_server_version(&server_version).await;
            }
            ServerMsg::Error { ref error } if error.kind == "unauthenticated" => {
                if target.sent_caller_explicit_token() {
                    tracing::warn!(
                        "the Server rejected the caller-explicit FLEETY_TOKEN: {} — unset or correct FLEETY_TOKEN; it overrides saved profile credentials and no saved token was changed",
                        error.message
                    );
                } else if matches!(target.source(), Source::OverrideUrl | Source::Env) {
                    tracing::warn!(
                        "the transient Server rejected authentication: {} — set a non-empty FLEETY_TOKEN for this endpoint, or unset FLEETY_AGENT_URL to use the saved current profile; no saved token was changed",
                        error.message
                    );
                } else if target.sent_saved_profile_token() {
                    tracing::warn!("server rejected our saved token: {}", error.message);
                } else {
                    tracing::warn!(
                        "the selected Server requires authentication: {} — pair this saved profile before reconnecting",
                        error.message
                    );
                }
                // Clearing is a credential mutation, so it needs the same proof
                // as any other: only a peer that opened the encrypted channel
                // has shown it is the Server this profile is paired with. An
                // unproven rejection fails this candidate and nothing else.
                if target.sent_saved_profile_token() && sealed {
                    if let Err(e) = clear_session_owner_token(session_owner, target) {
                        tracing::warn!(report = ?e.report(), "could not clear the rejected token");
                    }
                } else if target.sent_saved_profile_token() {
                    tracing::warn!(
                        "the endpoint rejected authentication before proving it is this profile's Server; the saved token was left untouched"
                    );
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
            ServerMsg::RunTool { .. } if !authenticated_welcome => {
                tracing::warn!(
                    "fleetyd received a control frame before authenticated Welcome; refusing the session"
                );
                decide_pending_reconnect(
                    pending_reconnect,
                    false,
                    "fleetyd received Server control before the selected profile completed authenticated Welcome"
                        .to_string(),
                );
                conn.close().await;
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
            // Channel-setup frames belong to the handshake. On a sealed link the
            // transport strips them, so one arriving here means the peer is
            // renegotiating mid-session or trying to seal a cleartext link.
            ServerMsg::SecureAccept { .. } | ServerMsg::SecureFrame { .. } => {
                tracing::warn!(
                    "the Server sent a secure-channel frame outside the handshake; dropping the session"
                );
                conn.close().await;
                return Outcome::Disconnected;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {

    /// A reconnect that authenticated on a saved alternative must stay
    /// retryable when its credential publication fails. Narrowing this to the
    /// primary turns a storage hiccup into a permanent failure for exactly the
    /// sessions roaming exists to serve.
    #[test]
    fn an_authenticated_alternative_still_counts_as_this_profiles_endpoint() {
        let mut profile = connection::Profile {
            url: "ws://192.168.1.20:8787".to_string(),
            ..Default::default()
        };
        profile.endpoints = vec!["ws://100.64.0.8:8787".to_string()];
        profile.configured_url = Some("ws://home.lan:8787".to_string());

        assert!(authenticated_endpoint_belongs_to(
            &profile,
            "ws://192.168.1.20:8787"
        ));
        assert!(
            authenticated_endpoint_belongs_to(&profile, "ws://100.64.0.8:8787"),
            "a learned endpoint that authenticated is still this profile's"
        );
        assert!(
            authenticated_endpoint_belongs_to(&profile, "ws://home.lan:8787"),
            "so is the address the user configured"
        );
        assert!(
            !authenticated_endpoint_belongs_to(&profile, "ws://198.51.100.9:8787"),
            "an address this profile never held must not count"
        );
    }
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
                "FLEETY_FORCE_SSE",
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
                "FLEETY_FORCE_SSE",
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

    fn seed_claimed_reconnect_journal() -> (PathBuf, Vec<u8>) {
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let durable = std::fs::read(&journal).expect("read durable journal");
        (journal, durable)
    }

    #[test]
    fn persist_and_clear_token_on_current_profile() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("token");

        let mut conns = connection::Connections {
            current: Some("default".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "default".to_string(),
            connection::Profile {
                url: "ws://srv:8787".to_string(),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save default");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve");
        assert!(persist_token(&target, "daemon-tok").expect("persist"));
        let conns = connection::load().expect("load");
        assert_eq!(conns.current.as_deref(), Some("default"));
        let p = conns.current_profile().expect("profile");
        assert_eq!(p.url, "ws://srv:8787");
        assert_eq!(p.token.as_deref(), Some("daemon-tok"));

        // Clearing after a rejection drops the token but keeps the profile.
        let persisted = connection::load().expect("load persisted token");
        let reconnect = connection::resolve(&persisted, &Target::Current, None, None, || None)
            .expect("resolve persisted token");
        assert!(clear_target_token(&reconnect).expect("clear"));
        let conns = connection::load().expect("reload");
        assert!(conns
            .current_profile()
            .and_then(|p| p.token.as_deref())
            .is_none());
    }

    #[test]
    fn explicit_env_token_has_no_disk_owner_even_for_the_same_endpoint() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("explicit-env-token-owner");
        let mut conns = connection::Connections {
            current: Some("A".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".to_string(),
            connection::Profile {
                url: "ws://server-a:8787".to_string(),
                token: Some("disk-token".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save A");
        let target = Resolved::unowned(
            "ws://server-a:8787".to_string(),
            Some("explicit-env-token".to_string()),
            Source::Env,
        );
        assert!(session_credential_owner(&target)
            .expect("capture owner")
            .is_none());
        assert!(!persist_token(&target, "minted-token").expect("skip persist"));
        assert_eq!(
            pin_target_fingerprint(&target, "fingerprint-a").expect("skip pin"),
            None
        );

        let saved = connection::load().expect("load A");
        assert_eq!(saved.profiles["A"].token.as_deref(), Some("disk-token"));
        assert!(saved.profiles["A"].fingerprint.is_none());
    }

    #[test]
    fn env_same_endpoint_has_no_owner_before_or_after_current_switch() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("env-owner-drift");
        let mut conns = connection::Connections {
            current: Some("A".to_string()),
            ..Default::default()
        };
        for name in ["A", "B"] {
            conns.profiles.insert(
                name.to_string(),
                connection::Profile {
                    url: "ws://shared:8787".to_string(),
                    token: Some("shared-token".to_string()),
                    ..Default::default()
                },
            );
        }
        connection::save(&conns).expect("save A/B");
        let target = Resolved::unowned(
            "ws://shared:8787".to_string(),
            Some("shared-token".to_string()),
            Source::Env,
        );
        assert!(session_credential_owner(&target)
            .expect("capture before switch")
            .is_none());
        connection::mutate(|live| {
            live.current = Some("B".to_string());
            Ok(())
        })
        .expect("switch to B");
        assert!(session_credential_owner(&target)
            .expect("capture after switch")
            .is_none());
        assert!(!clear_session_owner_token(None, &target).expect("skip clear"));

        let saved = connection::load().expect("load A/B");
        for name in ["A", "B"] {
            assert_eq!(saved.profiles[name].token.as_deref(), Some("shared-token"));
            assert!(saved.profiles[name].fingerprint.is_none());
        }
    }

    #[test]
    fn auth_rejection_refuses_to_clear_a_frozen_owner_after_current_switches() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("auth-rejection-frozen-owner");
        let mut conns = connection::Connections {
            current: Some("A".to_string()),
            ..Default::default()
        };
        for name in ["A", "B"] {
            conns.profiles.insert(
                name.to_string(),
                connection::Profile {
                    url: "ws://shared:8787".to_string(),
                    token: Some("shared-token".to_string()),
                    ..Default::default()
                },
            );
        }
        connection::save(&conns).expect("save A/B");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve A");
        let owner = session_credential_owner_in(&conns, &target).expect("owner A");
        connection::mutate(|live| {
            live.current = Some("B".to_string());
            Ok(())
        })
        .expect("switch to B");

        let error = clear_session_owner_token(Some(&owner), &target)
            .expect_err("a drifted current-profile owner must be rejected");
        assert!(
            error.report().message.contains("is no longer current"),
            "{}",
            error.report().message
        );

        let saved = connection::load().expect("load A/B");
        assert_eq!(saved.profiles["A"].token.as_deref(), Some("shared-token"));
        assert_eq!(saved.profiles["B"].token.as_deref(), Some("shared-token"));
    }

    #[test]
    fn reconnect_owner_snapshot_allows_an_explicit_transport_token() {
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        let disk_profile = connection::Profile {
            url: "ws://shared:8787".to_string(),
            token: Some("disk-token-b".to_string()),
            fingerprint: Some("fingerprint-b".to_string()),
            ..Default::default()
        };
        conns.profiles.insert("B".to_string(), disk_profile.clone());
        let target = connection::resolve(
            &conns,
            &Target::Current,
            None,
            Some("caller-token".to_string()),
            || None,
        )
        .expect("resolve B with explicit token");
        let owner = session_credential_owner_in(&conns, &target).expect("owner B");

        assert_eq!(
            reconnect_owner_fingerprint_in(&conns, "B", &target, Some(&owner))
                .expect("transport token does not change the disk owner"),
            Some("fingerprint-b".to_string())
        );
    }

    #[test]
    fn saved_profile_connect_failure_directs_explicit_repair_without_leaking_url_secrets() {
        let target = Resolved::unowned(
            "wss://host.test".into(),
            Some("stored-token".into()),
            Source::Profile("office".into()),
        );
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
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save A");
        let target = Resolved::unowned("ws://server-b:8787".to_string(), None, Source::Env);

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
    fn unowned_mdns_candidate_is_rejected_before_profile_mutation() {
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
        // A credentialed current profile never turns copied TXT metadata into a
        // target, so the persistence helpers cannot be reached for it.
        let error = connection::resolve(&conns, &Target::Current, None, None, || {
            Some(connection::ResolutionCandidate::Mdns(
                connection::Discovered {
                    url: "ws://server-b:8787".to_string(),
                    fingerprint: Some("fp-b".to_string()),
                },
            ))
        })
        .expect_err("credentialed profile requires explicit recovery");
        assert!(error.report().message.contains("--pairing-code <code>"));

        let after = connection::load().expect("reload A/B");
        assert_eq!(after.profiles["a"].token.as_deref(), Some("token-a"));
        assert_eq!(after.profiles["a"].fingerprint, None);
        assert_eq!(after.profiles["b"].token.as_deref(), Some("token-b"));
        assert_eq!(after.profiles["b"].fingerprint.as_deref(), Some("fp-b"));
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
        let target = Resolved::unowned(
            "ws://server-a:8787".to_string(),
            Some("old-a".to_string()),
            Source::Profile("a".to_string()),
        );

        assert!(!persist_token(&target, "new-a").expect("reject synthetic owner"));
        let after = connection::load().expect("reload A/B");
        assert!(after.profiles["a"].url.is_empty());
        assert_eq!(after.profiles["a"].token.as_deref(), Some("old-a"));
        assert_eq!(after.profiles["b"].token.as_deref(), Some("token-b"));
        assert_eq!(after.profiles["b"].fingerprint.as_deref(), Some("fp-b"));
    }

    #[test]
    fn profile_source_without_resolver_owner_capability_is_not_durable() {
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
        let target = Resolved::unowned(
            "ws://server-a:8787".to_string(),
            Some("saved-a".to_string()),
            Source::Profile("a".to_string()),
        );

        assert!(
            session_credential_owner_in(&conns, &target).is_none(),
            "display provenance is not a resolver-issued owner capability"
        );
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
        let target = connection::resolve(
            &conns,
            &Target::Current,
            None,
            Some("saved-a".to_string()),
            || None,
        )
        .expect("resolve current with same-value explicit token");
        let owner = session_credential_owner_in(&conns, &target)
            .expect("saved profile still supplies the disk owner snapshot");

        assert!(!clear_session_owner_token(Some(&owner), &target).expect("skip clear"));
        assert_eq!(
            connection::load().expect("reload").profiles["a"]
                .token
                .as_deref(),
            Some("saved-a")
        );
    }

    #[test]
    fn durable_session_rejects_whitespace_server_fingerprint() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("whitespace-server-fingerprint");
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
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve A");
        let owner = session_credential_owner_in(&conns, &target).expect("capture owner A");

        assert!(
            persist_authenticated_target_credentials(&owner, &target, " \t ", None, &[], false)
                .is_err(),
            "a durable identity pin must contain non-whitespace bytes"
        );
        assert!(connection::load().expect("reload").profiles["a"]
            .fingerprint
            .is_none());
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
            (Source::Default, connection::DEFAULT_URL),
        ] {
            let target = Resolved::unowned(url.to_string(), None, source);
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
        let error = connection::resolve(&conns, &Target::Current, None, None, || {
            Some(connection::ResolutionCandidate::Mdns(
                connection::Discovered {
                    url: "ws://rogue-advertiser:8787".to_string(),
                    fingerprint: Some("legacy-pin".to_string()),
                },
            ))
        })
        .expect_err("unselected discovery cannot become a persistence target");
        assert!(error.report().message.contains("fleety init"));

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
            resolve_target().expect("resolve").url(),
            connection::DEFAULT_URL
        );

        // No env + a current profile (a paired device) → its url + token.
        let mut conns = connection::Connections {
            current: Some("home".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "home".to_string(),
            connection::Profile {
                url: "ws://srv:8787".to_string(),
                token: Some("tok".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save paired profile");
        let r = resolve_target().expect("resolve");
        assert_eq!(r.url(), "ws://srv:8787");
        assert_eq!(r.token(), Some("tok"));

        // An old env deployment still connects: FLEETY_AGENT_URL overrides.
        std::env::set_var("FLEETY_AGENT_URL", "ws://env:8787");
        assert_eq!(resolve_target().expect("resolve").url(), "ws://env:8787");
    }

    #[test]
    fn empty_environment_url_upgrades_and_resolves_the_legacy_current_profile() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let guard = EnvGuard::new("resolve-empty-agent-url");
        std::env::set_var("FLEETY_AGENT_URL", "");
        let path = guard.temp_home.join(".fleety").join("connections.toml");
        std::fs::create_dir_all(path.parent().unwrap()).expect("create connections directory");
        std::fs::write(
            &path,
            "format_version = 1\nwriter_marker = \"fleety-store-v1\"\n\ncurrent = \"home\"\n\n[profiles.home]\nurl = \"ws://srv:8787\"\ntoken = \"tok\"\n",
        )
        .expect("seed legacy profile");

        let (target, owner) = resolve_target_with_owner().expect("empty environment URL is unset");

        assert_eq!(target.url(), "ws://srv:8787");
        assert!(target.has_profile_owner());
        assert!(owner.is_some());
        assert!(
            !connection::load().expect("load upgraded profile").profiles["home"]
                .generation
                .is_empty()
        );
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
            process_start: "test-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        let _identity = fleety_tools::service::claim_process_identity_at(
            &process_identity_path(&ready.process_start),
            &ready.process_start,
        )
        .expect("claim test process identity");
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

    #[cfg(unix)]
    #[test]
    fn reconnect_journal_read_failure_never_appends_after_a_torn_record() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-torn-tail-read-failure");
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
        let before = std::fs::read(&journal).expect("read journal before permission change");
        let original_permissions = std::fs::metadata(&journal)
            .expect("journal metadata")
            .permissions();
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o200))
            .expect("make journal write-only");

        let result = append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Claimed {
                nonce: "nonce-a".to_string(),
            },
        );

        std::fs::set_permissions(&journal, original_permissions)
            .expect("restore journal permissions");
        let error = result.expect_err("an unreadable torn tail must stop before append");
        assert!(
            error
                .report()
                .message
                .contains("cannot inspect fleetyd reconnect journal before repair"),
            "{}",
            error.report().message
        );
        assert_eq!(
            std::fs::read(&journal).expect("read preserved journal"),
            before,
            "a read failure must preserve the torn bytes and every durable event"
        );
    }

    #[test]
    fn reconnect_append_metadata_failure_never_writes_or_truncates_durable_events() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-append-metadata-failure");
        let (journal, durable) = seed_claimed_reconnect_journal();
        let opened = std::cell::Cell::new(false);
        let wrote = std::cell::Cell::new(false);

        let error = append_reconnect_bytes_at_with(
            &journal,
            b"partial",
            |_| Err(std::io::Error::other("injected metadata failure")),
            |path| {
                opened.set(true);
                std::fs::OpenOptions::new().append(true).open(path)
            },
            |file, bytes| {
                wrote.set(true);
                use std::io::Write;
                file.write_all(bytes)?;
                Err(std::io::Error::other("injected write failure"))
            },
            |_| Ok(()),
            rollback_reconnect_journal_at,
        )
        .expect_err("metadata failure must stop before append");

        assert!(error.report().message.contains("metadata"));
        assert!(
            !opened.get(),
            "metadata failure must prevent opening a writer"
        );
        assert!(!wrote.get(), "metadata failure must prevent any write");
        assert_eq!(
            std::fs::read(&journal).expect("read preserved journal"),
            durable,
            "metadata failure preserves every durable event"
        );
    }

    #[test]
    fn reconnect_append_write_failure_rolls_back_to_every_durable_event() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-append-write-failure");
        let (journal, durable) = seed_claimed_reconnect_journal();
        let synced = std::cell::Cell::new(false);

        let error = append_reconnect_bytes_at_with(
            &journal,
            b"partial",
            |path| std::fs::metadata(path).map(|metadata| metadata.len()),
            |path| std::fs::OpenOptions::new().append(true).open(path),
            |file, bytes| {
                use std::io::Write;
                file.write_all(bytes)?;
                Err(std::io::Error::other("injected write failure"))
            },
            |_| {
                synced.set(true);
                Ok(())
            },
            rollback_reconnect_journal_at,
        )
        .expect_err("write failure must roll back");

        assert!(error.report().message.contains("write failure"));
        assert!(!synced.get(), "failed write is never synced as success");
        assert_eq!(
            std::fs::read(&journal).expect("read rolled-back journal"),
            durable
        );
    }

    #[test]
    fn reconnect_append_sync_failure_rolls_back_to_every_durable_event() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-append-sync-failure");
        let (journal, durable) = seed_claimed_reconnect_journal();

        let error = append_reconnect_bytes_at_with(
            &journal,
            b"complete but not durable\n",
            |path| std::fs::metadata(path).map(|metadata| metadata.len()),
            |path| std::fs::OpenOptions::new().append(true).open(path),
            |file, bytes| {
                use std::io::Write;
                file.write_all(bytes)
            },
            |_| Err(std::io::Error::other("injected sync failure")),
            rollback_reconnect_journal_at,
        )
        .expect_err("sync failure must roll back");

        assert!(error.report().message.contains("sync failure"));
        assert_eq!(
            std::fs::read(&journal).expect("read rolled-back journal"),
            durable
        );
    }

    #[test]
    fn reconnect_append_rollback_open_failure_never_shortens_durable_events() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-append-rollback-open-failure");
        let (journal, durable) = seed_claimed_reconnect_journal();

        let error = append_reconnect_bytes_at_with(
            &journal,
            b"partial",
            |path| std::fs::metadata(path).map(|metadata| metadata.len()),
            |path| std::fs::OpenOptions::new().append(true).open(path),
            |file, bytes| {
                use std::io::Write;
                file.write_all(bytes)?;
                Err(std::io::Error::other("injected write failure"))
            },
            |_| Ok(()),
            |_, _| Err(std::io::Error::other("injected rollback open failure")),
        )
        .expect_err("rollback-open failure remains an error");

        let after = std::fs::read(&journal).expect("read journal after failed rollback");
        assert!(error.report().message.contains("rollback open failure"));
        assert!(after.starts_with(&durable));
        assert!(after.len() >= durable.len());
    }

    #[test]
    fn reconnect_append_rollback_truncate_failure_never_shortens_durable_events() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-append-rollback-truncate-failure");
        let (journal, durable) = seed_claimed_reconnect_journal();

        let error = append_reconnect_bytes_at_with(
            &journal,
            b"partial",
            |path| std::fs::metadata(path).map(|metadata| metadata.len()),
            |path| std::fs::OpenOptions::new().append(true).open(path),
            |file, bytes| {
                use std::io::Write;
                file.write_all(bytes)?;
                Err(std::io::Error::other("injected write failure"))
            },
            |_| Ok(()),
            |path, _| {
                let _rollback = std::fs::OpenOptions::new().write(true).open(path)?;
                Err(std::io::Error::other("injected rollback truncate failure"))
            },
        )
        .expect_err("rollback-truncate failure remains an error");

        let after = std::fs::read(&journal).expect("read journal after failed rollback");
        assert!(error.report().message.contains("rollback truncate failure"));
        assert!(after.starts_with(&durable));
        assert!(after.len() >= durable.len());
    }

    #[test]
    fn reconnect_rollback_uses_a_separate_write_capable_handle() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-rollback-write-handle");
        let (journal, durable) = seed_claimed_reconnect_journal();
        let mut append = std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .expect("open append-only handle");
        use std::io::Write;
        append.write_all(b"partial").expect("append partial tail");
        drop(append);

        rollback_reconnect_journal_at(&journal, durable.len() as u64)
            .expect("rollback with a separate writable handle");

        assert_eq!(
            std::fs::read(&journal).expect("read rolled-back journal"),
            durable
        );
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
    fn later_profile_request_reaps_an_older_settlement_and_submits_its_own_nonce() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-settlement-owner");
        let journal = reconnect_journal_path();
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "test-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        let _identity = fleety_tools::service::claim_process_identity_at(
            &process_identity_path(&ready.process_start),
            &ready.process_start,
        )
        .expect("claim test process identity");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("publish test control owner");
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit B");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "B connected".to_string(),
        };
        publish_reconnect_success_proof(&journal, &ack).expect("prove B success");
        append_reconnect_event_at(&journal, &ReconnectJournalEvent::Settled { ack })
            .expect("settle B");

        let error = request_running_daemon_reconnect_with_wait("C", std::time::Duration::ZERO)
            .expect_err("C must not inherit B's success");

        assert!(error.report().message.contains("remains durable"));
        assert!(!error.report().message.contains("B connected"));
        let state = load_reconnect_journal_at(&journal)
            .expect("load new journal")
            .expect("new C request");
        assert_eq!(state.request.expected_profile, "C");
        assert_ne!(state.request.nonce, "r1");
        assert!(matches!(state.phase, ReconnectPhase::Submitted));
    }

    #[test]
    fn same_profile_request_submits_a_new_nonce_after_reaping_an_older_settlement() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-same-profile-settlement-owner");
        let journal = reconnect_journal_path();
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "test-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        let _identity = fleety_tools::service::claim_process_identity_at(
            &process_identity_path(&ready.process_start),
            &ready.process_start,
        )
        .expect("claim test process identity");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("publish test control owner");
        let previous = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &previous).expect("submit previous B request");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "stale B success".to_string(),
        };
        publish_reconnect_success_proof(&journal, &ack).expect("prove previous B success");
        append_reconnect_event_at(&journal, &ReconnectJournalEvent::Settled { ack })
            .expect("settle previous B request");

        let error = request_running_daemon_reconnect_with_wait("B", std::time::Duration::ZERO)
            .expect_err("a new B operation cannot return r1 success");

        assert!(error.report().message.contains("remains durable"));
        assert!(!error.report().message.contains("stale B success"));
        let state = load_reconnect_journal_at(&journal)
            .expect("load new journal")
            .expect("new B request");
        assert_eq!(state.request.expected_profile, "B");
        assert_ne!(state.request.nonce, "r1");
        assert!(matches!(state.phase, ReconnectPhase::Submitted));
    }

    #[test]
    fn replacing_a_settled_request_preserves_its_nonce_addressed_receipt() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-settlement-receipt");
        let journal = reconnect_journal_path();
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "test-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        let _identity = fleety_tools::service::claim_process_identity_at(
            &process_identity_path(&ready.process_start),
            &ready.process_start,
        )
        .expect("claim test process identity");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("publish test control owner");
        let previous = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "r1".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &previous).expect("submit r1");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "r1 connected".to_string(),
        };
        publish_reconnect_success_proof(&journal, &ack).expect("prove r1 success");
        append_reconnect_event_at(&journal, &ReconnectJournalEvent::Settled { ack })
            .expect("settle r1");

        request_running_daemon_reconnect_with_wait("B", std::time::Duration::ZERO)
            .expect_err("r2 remains pending while daemon is offline");

        let receipt = control_path("fleetyd.reconnect-receipts").join("7231.json");
        let bytes = std::fs::read(receipt).expect("r1 result must survive r2 submission");
        assert!(String::from_utf8_lossy(&bytes).contains("r1 connected"));
        let observed = take_reconnect_ack_at(&journal, "r1")
            .expect("observe r1 receipt")
            .expect("r1 terminal result");
        assert!(observed.accepted);
        assert_eq!(observed.message, "r1 connected");
        assert!(
            reconnect_success_proof_path_at(&journal, "r1")
                .expect("proof path")
                .exists(),
            "delivered success proof remains queryable until retention cleanup"
        );
        let active = load_reconnect_journal_at(&journal)
            .expect("load active journal")
            .expect("r2 remains active");
        assert_ne!(active.request.nonce, "r1");
    }

    #[test]
    fn existing_receipt_retries_a_failed_durability_sync() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-receipt-sync-retry");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "r1").expect("receipt path");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "connected".to_string(),
        };
        let mut attempts = 0;
        let mut fail_once = |_: &std::path::Path, _: &std::path::Path| {
            attempts += 1;
            if attempts == 1 {
                Err(reconnect_journal_error(
                    "injected receipt directory sync failure",
                ))
            } else {
                Ok(())
            }
        };

        preserve_reconnect_receipt_at_with(&receipt, &ack, &mut fail_once)
            .expect_err("first durability sync fails after receipt publication");
        assert!(receipt.exists(), "published receipt remains retryable");
        preserve_reconnect_receipt_at_with(&receipt, &ack, &mut fail_once)
            .expect("existing receipt retries durability");
        assert_eq!(attempts, 2);
    }

    #[test]
    fn ambiguous_published_proof_is_hidden_before_leases_release() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-proof-quarantine");
        let journal = reconnect_journal_path();
        let proof = reconnect_success_proof_path_at(&journal, "r1").expect("proof path");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "connected".to_string(),
        };
        preserve_reconnect_receipt_at(&proof, &ack).expect("publish proof");

        quarantine_ambiguous_success_proof(&proof).expect("quarantine proof");

        assert!(!proof.exists(), "caller cannot see the ambiguous proof");
    }

    #[test]
    fn ambiguous_proof_quarantine_retries_directory_sync_before_returning() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-proof-quarantine-sync");
        let journal = reconnect_journal_path();
        let proof = reconnect_success_proof_path_at(&journal, "r1").expect("proof path");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "connected".to_string(),
        };
        preserve_reconnect_receipt_at(&proof, &ack).expect("publish proof");
        let mut sync_attempts = 0;

        quarantine_ambiguous_success_proof_with(
            &proof,
            |from, to| std::fs::rename(from, to),
            |target| std::fs::remove_file(target),
            |_| {
                sync_attempts += 1;
                if sync_attempts == 1 {
                    Err(reconnect_journal_error(
                        "injected quarantine directory sync failure",
                    ))
                } else {
                    Ok(())
                }
            },
            || {},
        )
        .expect("quarantine retries durability");

        assert_eq!(sync_attempts, 2);
        assert!(!proof.exists(), "canonical proof stays hidden");
    }

    #[test]
    fn ambiguous_proof_quarantine_retries_transient_hide_failures() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-proof-quarantine-hide");
        let journal = reconnect_journal_path();
        let proof = reconnect_success_proof_path_at(&journal, "r1").expect("proof path");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "connected".to_string(),
        };
        preserve_reconnect_receipt_at(&proof, &ack).expect("publish proof");
        let mut rename_attempts = 0;
        let mut remove_attempts = 0;

        quarantine_ambiguous_success_proof_with(
            &proof,
            |from, to| {
                rename_attempts += 1;
                if rename_attempts == 1 {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected rename failure",
                    ))
                } else {
                    std::fs::rename(from, to)
                }
            },
            |path| {
                remove_attempts += 1;
                if remove_attempts == 1 {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected remove failure",
                    ))
                } else {
                    std::fs::remove_file(path)
                }
            },
            |_| Ok(()),
            || {},
        )
        .expect("quarantine retries hiding");

        assert_eq!(rename_attempts, 2);
        assert_eq!(remove_attempts, 1);
        assert!(!proof.exists(), "canonical proof stays hidden");
    }

    #[cfg(unix)]
    #[test]
    fn absent_proof_under_a_missing_directory_syncs_its_control_root() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-proof-quarantine-absent");
        let journal = reconnect_journal_path();
        let proof = reconnect_success_proof_path_at(&journal, "r1").expect("proof path");
        let parent = proof.parent().expect("proof parent");
        std::fs::create_dir_all(journal.parent().expect("control root"))
            .expect("create control root");

        assert!(!parent.exists(), "proof directory starts absent");
        sync_hidden_reconnect_proof_directories_at(parent)
            .expect("the existing control root proves the absent child durable");
    }

    #[test]
    fn receipt_durability_includes_the_control_root_for_a_new_subdirectory() {
        let control_root = std::path::Path::new("control-root");
        let receipt_dir = control_root.join("fleetyd.reconnect-success");

        assert_eq!(
            reconnect_receipt_sync_directories(&receipt_dir),
            vec![receipt_dir, control_root.to_path_buf()]
        );
    }

    #[cfg(windows)]
    #[test]
    fn reconnect_receipt_sync_opens_the_published_file_with_write_access() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-receipt-windows-sync");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "r1").expect("receipt path");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "connected".to_string(),
        };

        preserve_reconnect_receipt_at(&receipt, &ack)
            .expect("Windows FlushFileBuffers requires a write-capable file handle");
        preserve_reconnect_receipt_at(&receipt, &ack)
            .expect("an existing receipt remains durably retryable on Windows");
    }

    #[test]
    fn receipt_cleanup_failure_cannot_replace_the_observed_terminal_result() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-receipt-cleanup-failure");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "r1").expect("receipt path");
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: false,
            message: "r1 failed".to_string(),
        };
        preserve_reconnect_receipt_at(&receipt, &ack).expect("publish r1 receipt");
        submit_reconnect_at(
            &journal,
            &ReconnectRequest {
                instance: "daemon-a".to_string(),
                nonce: "r2".to_string(),
                expected_profile: "B".to_string(),
            },
        )
        .expect("submit active r2");

        let observed = take_reconnect_ack_at_with(
            &journal,
            "r1",
            |_| {
                Err(reconnect_journal_error(
                    "injected receipt cleanup sync failure",
                ))
            },
            |_| panic!("r2 journal must not be reaped while observing r1"),
        )
        .expect("cleanup failure does not replace settlement")
        .expect("r1 terminal result");

        assert_eq!(observed, ack);
        assert!(receipt.exists(), "failed cleanup remains retryable");
        let active = load_reconnect_journal_at(&journal)
            .expect("load active journal")
            .expect("r2 remains active");
        assert_eq!(active.request.nonce, "r2");
    }

    #[test]
    fn startup_retains_a_success_proof_whose_terminal_carrier_is_gone() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-orphan-proof-reap");
        let journal = reconnect_journal_path();
        let ack = ReconnectAck {
            nonce: "r1".to_string(),
            accepted: true,
            message: "connected".to_string(),
        };
        publish_reconnect_success_proof(&journal, &ack).expect("publish orphan proof");
        let proof = reconnect_success_proof_path_at(&journal, "r1").expect("proof path");

        reap_orphan_reconnect_success_proofs_at(&journal).expect("inspect orphan proof");

        assert!(
            proof.exists(),
            "an ambiguous success proof stays diagnosable"
        );
    }

    #[test]
    fn startup_reaps_a_crash_left_success_proof_temp_file() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-orphan-proof-temp");
        let journal = reconnect_journal_path();
        let proof_dir = journal
            .parent()
            .expect("control root")
            .join("fleetyd.reconnect-success");
        std::fs::create_dir_all(&proof_dir).expect("create proof dir");
        let temp = proof_dir.join(".receipt-crash.tmp");
        std::fs::write(&temp, b"{").expect("leave torn temp");

        reap_orphan_reconnect_success_proofs_at(&journal).expect("reap crash temp");

        assert!(!temp.exists());
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
    fn candidate_deadline_bounds_a_stalled_sse_hello_post() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("stalled-sse-hello-post");
        std::env::set_var("FLEETY_FORCE_SSE", "1");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build test runtime");
        runtime.block_on(async {

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind stalled SSE server");
            let address = listener.local_addr().expect("stalled SSE address");
            tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(async move {
                        let mut request = vec![0_u8; 4096];
                        let Ok(read) =
                            tokio::io::AsyncReadExt::read(&mut stream, &mut request).await
                        else {
                            return;
                        };
                        let request = String::from_utf8_lossy(&request[..read]);
                        if request.starts_with("GET /sse?") {
                            let response = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                                         transfer-encoding: chunked\r\nconnection: keep-alive\r\n\r\n\
                                         9\r\n: ready\n\n\r\n";
                            let _ =
                                tokio::io::AsyncWriteExt::write_all(&mut stream, response).await;
                            std::future::pending::<()>().await;
                        } else if request.starts_with("POST /send?") {
                            std::future::pending::<()>().await;
                        }
                    });
                }
            });

            let url = format!("ws://{address}");
            let target = Resolved::unowned(url.clone(), None, Source::OverrideUrl);
            let connection = fleety_tools::transport::connect(&url, None)
                .await
                .expect("open SSE downstream");
            let mut pending = None;
            let mut authenticated = false;

            let result = tokio::time::timeout(
                std::time::Duration::from_millis(300),
                serve(
                    &target,
                    None,
                    connection,
                    None,
                    None,
                    &mut pending,
                    tokio::time::Instant::now() + std::time::Duration::from_millis(50),
                    false,
                    &mut authenticated,
                ),
            )
            .await;

            assert!(
                result.is_ok(),
                "one candidate deadline must also bound a stalled SSE Hello POST"
            );
            assert!(matches!(
                result.expect("bounded serve"),
                Outcome::Disconnected
            ));
            assert!(!authenticated);
        });
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

        let settled = settle_pending_reconnect_at_with_proof(
            &journal,
            &mut pending,
            |path, ack| {
                publish_reconnect_success_proof(path, ack)?;
                let observed = take_reconnect_ack_at(path, &ack.nonce)?
                    .ok_or_else(|| reconnect_journal_error("caller did not observe settlement"))?;
                if observed != *ack {
                    return Err(reconnect_journal_error(
                        "caller observed a different settlement",
                    ));
                }
                Ok(())
            },
            append_reconnect_event_at,
        );

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

        assert!(
            !journal.exists(),
            "terminal journal is reaped after recovery"
        );
        let receipt = reconnect_receipt_path_at(&journal, "r1").expect("receipt");
        let ack = load_reconnect_receipt_at(&receipt)
            .expect("load receipt")
            .expect("receipt");
        assert!(matches!(
            ack,
            ReconnectAck {
                accepted: false,
                ref message,
                ..
            } if message.contains("restarted")
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
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");

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
    fn authenticated_reconnect_write_failure_keeps_retryable_success_after_credentials() {
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
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");
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
                .and_then(|pending| pending.decision.as_ref())
                .is_some_and(|decision| decision.accepted),
            "authenticated success remains frozen for durable settlement retry"
        );
        decide_pending_reconnect(&mut pending, false, "later failure".to_string());
        let decision = pending
            .as_ref()
            .and_then(|pending| pending.decision.as_ref())
            .expect("frozen decision");
        assert!(decision.accepted);
        assert!(!decision.message.contains("later failure"));
    }

    #[test]
    fn credential_persistence_failure_prevents_success_publication() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-credential-persistence-failure");
        let target = Resolved::unowned(
            "ws://server-b:8787".to_string(),
            Some("old-token".to_string()),
            Source::Profile("B".to_string()),
        );
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit reconnect");
        let claimed = claim_reconnect_at(&journal, "daemon-a")
            .expect("claim reconnect")
            .expect("claimed request");
        let mut pending = Some(PendingReconnect::new(claimed));
        let mut appended = false;

        let error = commit_authenticated_reconnect_with(
            &target,
            "fingerprint-b",
            Some("new-token"),
            &mut pending,
            |_, _, _, _| {
                Err(agent_core::CoreError::Message(
                    "injected credential persistence failure".to_string(),
                ))
            },
            |_, _| {
                appended = true;
                Ok(())
            },
        )
        .expect_err("credential failure must stop success publication");

        assert!(error.report().message.contains("credential"));
        assert!(!appended, "success settlement must not be attempted");
        assert!(
            pending
                .as_ref()
                .is_some_and(|pending| pending.decision.is_none()),
            "failed credential persistence cannot freeze success"
        );
    }

    #[test]
    fn storage_failure_after_credential_commit_keeps_success_retryable() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-post-credential-storage-failure");
        let target = Resolved::unowned(
            "ws://server-b:8787".to_string(),
            Some("old-token".to_string()),
            Source::Profile("B".to_string()),
        );
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        let mut pending = Some(PendingReconnect::new(request));

        commit_authenticated_reconnect_with_settle(
            &target,
            "fingerprint-b",
            Some("new-token"),
            &mut pending,
            |_, _, _, _| {
                Ok(Resolved::unowned(
                    target.url_owned(),
                    Some("new-token".to_string()),
                    target.source().clone(),
                ))
            },
            |_, _, _| {
                Err(agent_core::CoreError::Message(
                    "injected reconnect lease storage failure".to_string(),
                ))
            },
        )
        .expect_err("post-credential storage failure remains retryable");

        let pending = pending.expect("pending reconnect retained");
        assert!(
            pending.decision.is_some_and(|decision| decision.accepted),
            "committed credentials freeze success before storage-dependent settlement"
        );
        assert_eq!(
            pending
                .authenticated
                .expect("committed snapshot")
                .target
                .token(),
            Some("new-token")
        );
    }

    #[test]
    fn authenticated_retry_repeats_credential_publication_sync_before_success() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-credential-publication-retry");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("new-token".to_string()),
                fingerprint: Some("fingerprint-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save committed credentials");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        let mut pending = Some(PendingReconnect::new(request));
        freeze_authenticated_reconnect(&target, Some("fingerprint-b"), &mut pending)
            .expect("freeze authenticated result");

        let error = settle_pending_reconnect_with_credential_sync(&mut pending, || {
            Err(agent_core::CoreError::Message(
                "injected credential publication sync failure".to_string(),
            ))
        })
        .expect_err("publication sync must gate success settlement");

        assert!(error.report().message.contains("credential publication"));
        let pending = pending.expect("accepted reconnect remains retryable");
        assert!(
            pending.decision.is_some_and(|decision| decision.accepted),
            "sync failure cannot replace the frozen authenticated decision"
        );
        assert!(pending.authenticated.is_some());
    }

    #[test]
    fn success_proof_failure_keeps_journal_unobservable_and_retryable() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-success-proof-failure");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("new-token".to_string()),
                fingerprint: Some("fingerprint-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save committed credentials");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");
        let mut pending = Some(PendingReconnect::new(ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        }));
        let mut appended = false;

        settle_authenticated_reconnect_with_proof(
            &target,
            Some("fingerprint-b"),
            &mut pending,
            |_, _| {
                Err(agent_core::CoreError::Message(
                    "injected success proof failure".to_string(),
                ))
            },
            |_, _| {
                appended = true;
                Ok(())
            },
        )
        .expect_err("proof failure blocks caller-visible success");

        assert!(appended, "journal durability precedes proof publication");
        assert!(
            pending
                .as_ref()
                .and_then(|pending| pending.decision.as_ref())
                .is_some_and(|decision| decision.accepted),
            "unproven success remains frozen for proof retry"
        );
    }

    #[test]
    fn append_sync_ambiguity_never_promotes_page_cache_success() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-append-sync-ambiguity");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("new-token".to_string()),
                fingerprint: Some("fingerprint-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save committed credentials");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let mut pending = Some(PendingReconnect::new(request));

        let error = settle_authenticated_reconnect_with_proof(
            &target,
            Some("fingerprint-b"),
            &mut pending,
            |_, _| Ok(()),
            |path, event| {
                append_reconnect_event_at(path, event)?;
                Err(agent_core::CoreError::Message(
                    "injected sync ambiguity after write".to_string(),
                ))
            },
        )
        .expect_err("an append error cannot be promoted from readable bytes");

        assert!(error.report().message.contains("sync ambiguity"));
        assert!(
            pending
                .as_ref()
                .and_then(|pending| pending.decision.as_ref())
                .is_some_and(|decision| decision.accepted),
            "authenticated success remains frozen for a durable retry"
        );
        assert!(
            take_reconnect_ack_at(&journal, "nonce-a").is_err(),
            "caller cannot observe success until its post-append proof commits"
        );
    }

    #[test]
    fn authenticated_success_retry_rejects_owner_drift_before_proof() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-success-retry-owner-drift");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("new-token".to_string()),
                fingerprint: Some("fingerprint-b".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save committed credentials");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let mut pending = Some(PendingReconnect::new(request));
        settle_authenticated_reconnect_with(
            &target,
            Some("fingerprint-b"),
            &mut pending,
            |_, _| {
                Err(agent_core::CoreError::Message(
                    "injected first append failure".to_string(),
                ))
            },
        )
        .expect_err("first append fails");

        connection::mutate(|live| {
            live.current = None;
            Ok(())
        })
        .expect("drift current owner");

        assert!(
            settle_pending_reconnect(&mut pending).expect("owner drift settles failure"),
            "owner drift produces a terminal result"
        );
        let result = take_reconnect_ack_at(&journal, "nonce-a")
            .expect("read owner-drift result")
            .expect("terminal result");
        assert!(!result.accepted);
        assert!(result.message.contains("changed before reconnect success"));
        assert!(
            !reconnect_success_proof_path_at(&journal, "nonce-a")
                .expect("proof path")
                .exists(),
            "owner drift cannot publish success proof"
        );
    }

    #[test]
    fn frozen_success_retry_finishes_from_an_existing_failure_receipt() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-owner-drift-receipt-retry");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        let mut pending = Some(PendingReconnect {
            request,
            decision: Some(ReconnectAck {
                nonce: "nonce-a".to_string(),
                accepted: true,
                message: "frozen success".to_string(),
            }),
            authenticated: None,
        });
        let failure = ReconnectAck {
            nonce: "nonce-a".to_string(),
            accepted: false,
            message: "owner drift".to_string(),
        };
        let receipt = reconnect_receipt_path_at(&journal, "nonce-a").expect("receipt path");
        preserve_reconnect_receipt_at(&receipt, &failure).expect("publish failure receipt");

        assert!(
            reject_frozen_authenticated_reconnect(&mut pending, "owner drift".to_string())
                .expect("resume failure cleanup"),
            "existing failure receipt is authoritative"
        );
        assert!(pending.is_none());
        assert!(!journal.exists());
        let observed = take_reconnect_ack_at(&journal, "nonce-a")
            .expect("observe failure receipt")
            .expect("failure result");
        assert!(!observed.accepted);
    }

    #[test]
    fn restart_reaps_a_journal_when_its_terminal_receipt_already_exists() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-receipt-authoritative-on-restart");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-old".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-old")
            .expect("claim")
            .expect("request");
        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Settled {
                ack: ReconnectAck {
                    nonce: "nonce-a".to_string(),
                    accepted: true,
                    message: "frozen success".to_string(),
                },
            },
        )
        .expect("settle frozen success");
        let receipt = reconnect_receipt_path_at(&journal, "nonce-a").expect("receipt path");
        let failure = ReconnectAck {
            nonce: "nonce-a".to_string(),
            accepted: false,
            message: "profile owner changed before reconnect success".to_string(),
        };
        preserve_reconnect_receipt_at(&receipt, &failure).expect("publish terminal receipt");

        let control = ControlGuard::claim()
            .expect("the terminal receipt must let restart finish journal cleanup");

        assert!(!journal.exists(), "restart reaps the obsolete journal");
        assert_eq!(
            load_reconnect_receipt_at(&receipt)
                .expect("load terminal receipt")
                .expect("terminal receipt"),
            failure,
            "restart preserves the first durable terminal result"
        );
        drop(control);
    }

    #[test]
    fn caller_rejects_success_without_durable_credential_commit_proof() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-unproven-success");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Settled {
                ack: ReconnectAck {
                    nonce: "nonce-a".to_string(),
                    accepted: true,
                    message: "page-cache-only success".to_string(),
                },
            },
        )
        .expect("inject unproven success");

        let error =
            take_reconnect_ack_at(&journal, "nonce-a").expect_err("unproven success is rejected");
        assert!(error.report().message.contains("no durable"));
    }

    #[test]
    fn caller_rejects_success_receipt_without_durable_credential_commit_proof() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-unproven-success-receipt");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "nonce-a").expect("receipt path");
        preserve_reconnect_receipt_at(
            &receipt,
            &ReconnectAck {
                nonce: "nonce-a".to_string(),
                accepted: true,
                message: "unproven receipt success".to_string(),
            },
        )
        .expect("inject unproven success receipt");

        let error = take_reconnect_ack_at(&journal, "nonce-a")
            .expect_err("unproven receipt success is rejected");
        assert!(error.report().message.contains("no durable"));
        assert!(
            receipt.exists(),
            "rejected receipt remains available for repair"
        );
    }

    #[test]
    fn restart_converts_unproven_success_projection_to_failure() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-unproven-success-restart");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Settled {
                ack: ReconnectAck {
                    nonce: "nonce-a".to_string(),
                    accepted: true,
                    message: "journal survived before proof".to_string(),
                },
            },
        )
        .expect("inject unproven success projection");

        recover_reconnect_for_instance_at(&journal, "daemon-restarted")
            .expect("restart repairs unproven success");
        let recovered = take_reconnect_ack_at(&journal, "nonce-a")
            .expect("read repaired result")
            .expect("terminal failure");
        assert!(!recovered.accepted);
        assert!(recovered
            .message
            .contains("before reconnect success committed"));
    }

    #[test]
    fn settlement_failure_after_credential_commit_leaves_restart_ready_token_without_success() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-credential-commit-before-settlement");
        let mut conns = connection::Connections {
            current: Some("B".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "B".to_string(),
            connection::Profile {
                url: "ws://server-b:8787".to_string(),
                token: Some("old-token".to_string()),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("save B");
        let target =
            connection::resolve(&conns, &Target::Current, None, None, || None).expect("resolve B");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-a".to_string(),
            expected_profile: "B".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit reconnect");
        let claimed = claim_reconnect_at(&journal, "daemon-a")
            .expect("claim reconnect")
            .expect("claimed request");
        let mut pending = Some(PendingReconnect::new(claimed));

        commit_authenticated_reconnect_with(
            &target,
            "fingerprint-b",
            Some("new-token"),
            &mut pending,
            persist_authenticated_profile_credentials,
            |_, _| {
                Err(agent_core::CoreError::Message(
                    "injected settlement failure after credential commit".to_string(),
                ))
            },
        )
        .expect_err("settlement append fails");

        let persisted = connection::load().expect("load committed credential");
        let profile = &persisted.profiles["B"];
        assert_eq!(profile.token.as_deref(), Some("new-token"));
        assert_eq!(profile.fingerprint.as_deref(), Some("fingerprint-b"));
        assert!(
            pending
                .as_ref()
                .and_then(|pending| pending.decision.as_ref())
                .is_some_and(|decision| decision.accepted),
            "failed settlement append remains retryable but has no caller-visible proof"
        );
        let restart_target = resolve_target().expect("restart resolves committed profile");
        assert_eq!(restart_target.token(), Some("new-token"));
        recover_reconnect_for_instance_at(&journal, "daemon-restarted")
            .expect("restart settles interrupted nonce");
        let recovered = load_reconnect_journal_at(&journal)
            .expect("load recovered journal")
            .expect("recovered settlement");
        assert!(matches!(
            recovered.phase,
            ReconnectPhase::Settled(ReconnectAck {
                accepted: false,
                ..
            })
        ));
    }

    #[test]
    fn ready_publication_declares_control_version_and_process_start_identity() {
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "test-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        let value: serde_json::Value =
            serde_json::from_slice(&encode_ready(&ready)).expect("ready JSON");

        assert_eq!(
            value.get("version").and_then(serde_json::Value::as_u64),
            Some(1)
        );
        assert!(
            value
                .get("process_start")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|identity| !identity.is_empty()),
            "ready publication must distinguish PID reuse"
        );
    }

    #[test]
    fn unknown_ready_and_journal_versions_fail_immediately() {
        let unknown_ready = serde_json::json!({
            "version": 999,
            "pid": std::process::id(),
            "process_start": "start-a",
            "instance": "daemon-a"
        });
        assert!(
            parse_ready(unknown_ready.to_string().as_bytes()).is_none(),
            "an unknown ready contract must not be treated as the current owner format"
        );
        let future_shape = serde_json::json!({
            "version": 999,
            "owner": {
                "process": 42,
                "generation": "future-a"
            }
        });
        let future_error = parse_ready_record(future_shape.to_string().as_bytes())
            .expect_err("version negotiation must precede current-version fields");
        assert!(future_error.report().message.contains("incompatible"));
        assert!(future_error.report().message.contains("update"));

        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("unknown-reconnect-journal-version");
        let journal = reconnect_journal_path();
        std::fs::create_dir_all(journal.parent().expect("control directory"))
            .expect("create control directory");
        std::fs::write(
            &journal,
            serde_json::json!({
                "version": 999,
                "event": "submitted",
                "instance": "daemon-a",
                "nonce": "nonce-a",
                "expected_profile": "B"
            })
            .to_string()
                + "\n",
        )
        .expect("seed unknown journal");

        let error = load_reconnect_journal_at(&journal)
            .expect_err("unknown control journal must fail closed");
        assert!(error.report().message.contains("version"));
        assert!(error.report().message.contains("update"));
    }

    #[test]
    fn current_requester_rejects_legacy_ready_without_waiting_or_submitting() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("legacy-ready-version");
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        std::fs::write(
            ready_path(),
            serde_json::json!({
                "pid": std::process::id(),
                "instance": "legacy-daemon"
            })
            .to_string(),
        )
        .expect("seed legacy ready");

        let error = request_running_daemon_reconnect_with_wait("B", std::time::Duration::ZERO)
            .expect_err("legacy control owner must require an update");

        assert!(error.report().message.contains("incompatible"));
        assert!(error.report().message.contains("update"));
        assert!(
            !reconnect_journal_path().exists(),
            "the new requester must not submit a request an old Daemon may mishandle"
        );
    }

    #[test]
    fn current_daemon_reads_and_settles_a_legacy_requester_journal() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("legacy-requester-current-daemon");
        let journal = reconnect_journal_path();
        std::fs::create_dir_all(journal.parent().expect("control directory"))
            .expect("create control directory");
        std::fs::write(
            &journal,
            serde_json::json!({
                "event": "submitted",
                "instance": "daemon-a",
                "nonce": "legacy-nonce",
                "expected_profile": "B"
            })
            .to_string()
                + "\n",
        )
        .expect("seed legacy requester journal");

        let request = claim_reconnect_at(&journal, "daemon-a")
            .expect("read legacy request")
            .expect("claim legacy request");
        let mut pending = Some(PendingReconnect::new(request));
        decide_pending_reconnect(
            &mut pending,
            false,
            "update the Fleety CLI to use reconnect control version 1".to_string(),
        );
        settle_pending_reconnect_at_with(&journal, &mut pending, append_reconnect_event_at)
            .expect("settle for legacy requester");

        let state = load_reconnect_journal_at(&journal)
            .expect("load mixed journal")
            .expect("terminal state");
        assert!(matches!(
            state.phase,
            ReconnectPhase::Settled(ReconnectAck {
                accepted: false,
                ref message,
                ..
            }) if message.contains("update")
        ));
        let records = std::fs::read_to_string(&journal).expect("mixed journal");
        assert!(
            records
                .lines()
                .skip(1)
                .all(|line| line.contains("\"version\":1")),
            "new events carry a version while remaining readable to the legacy parser"
        );
    }

    #[test]
    fn reconnect_status_reconstructs_lifecycle_without_mutating_the_journal() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-status-lifecycle");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-status".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let before = std::fs::read(&journal).expect("read journal before status");

        let status = reconnect_status_at(&journal, "nonce-status")
            .expect("status")
            .expect("known nonce");

        assert_eq!(status.nonce, "nonce-status");
        assert_eq!(status.profile, "home");
        assert_eq!(status.owner, "daemon-a");
        assert_eq!(status.state, ReconnectLifecycle::InProgress);
        assert!(status.submitted_at > 0);
        assert!(status.claimed_at.is_some());
        assert!(status.terminal_result.is_none());
        assert!(status.replacement_nonce.is_none());
        assert_eq!(
            std::fs::read(&journal).expect("read journal after status"),
            before
        );
    }

    #[test]
    fn status_marks_an_accepted_settlement_ambiguous_without_its_success_proof() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-status-missing-proof");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-unproved-success".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        let ack = ReconnectAck {
            nonce: request.nonce.clone(),
            accepted: true,
            message: "accepted without proof".to_string(),
        };
        append_reconnect_event_at(&journal, &ReconnectJournalEvent::Settled { ack })
            .expect("settle");
        let before = std::fs::read(&journal).expect("journal before status");

        let status = reconnect_status_at(&journal, &request.nonce)
            .expect("status")
            .expect("known nonce");

        assert_eq!(status.state, ReconnectLifecycle::Ambiguous);
        assert!(status
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("success proof")));
        assert_eq!(
            std::fs::read(&journal).expect("journal after status"),
            before
        );
    }

    #[test]
    fn legacy_reconnect_record_without_timestamp_uses_file_time_for_retention() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-legacy-timestamp");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "nonce-legacy-time").expect("receipt");
        std::fs::create_dir_all(receipt.parent().expect("receipt directory"))
            .expect("receipt directory");
        std::fs::write(
            &receipt,
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION,
                "event": "settled",
                "nonce": "nonce-legacy-time",
                "accepted": false,
                "message": "legacy terminal record",
            })
            .to_string(),
        )
        .expect("receipt");
        std::thread::sleep(std::time::Duration::from_millis(20));

        let record = load_reconnect_receipt_record_at(&receipt)
            .expect("load legacy receipt")
            .expect("record");
        let file_time = reconnect_file_timestamp(&receipt);

        assert!(file_time > 0);
        assert!(record.timestamp <= file_time + 1);
    }

    #[test]
    fn duplicate_nonce_cannot_replace_a_retained_terminal_result() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-duplicate-retained-nonce");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "nonce-retained").expect("receipt");
        let original = ReconnectAck {
            nonce: "nonce-retained".to_string(),
            accepted: false,
            message: "original terminal result".to_string(),
        };
        preserve_reconnect_receipt_at(&receipt, &original).expect("retain result");
        let request = ReconnectRequest {
            instance: "daemon-new".to_string(),
            nonce: original.nonce.clone(),
            expected_profile: "other-profile".to_string(),
        };

        let error = submit_reconnect_at(&journal, &request)
            .expect_err("a retained terminal nonce must remain authoritative");

        assert!(error.report().message.contains("nonce-retained"));
        assert_eq!(
            load_reconnect_receipt_at(&receipt).expect("read receipt"),
            Some(original)
        );
        assert!(
            !journal.exists(),
            "duplicate submission must not create a journal"
        );
    }

    #[test]
    fn current_owner_can_cancel_before_success_and_foreign_owner_cannot() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-owner-cancel");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-cancel".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        let before = std::fs::read(&journal).expect("read before foreign cancellation");

        let foreign = cancel_reconnect_at(&journal, "nonce-cancel", "daemon-b")
            .expect_err("foreign owner must be refused");
        assert!(foreign.report().message.contains("foreign"));
        assert_eq!(
            std::fs::read(&journal).expect("read after foreign cancellation"),
            before
        );

        let result = cancel_reconnect_at(&journal, "nonce-cancel", "daemon-a")
            .expect("current owner cancels");
        assert!(!result.accepted);
        assert_eq!(
            reconnect_status_at(&journal, "nonce-cancel")
                .expect("status")
                .expect("cancelled request")
                .state,
            ReconnectLifecycle::Cancelled
        );
    }

    #[test]
    fn cancellation_cannot_erase_a_durable_authenticated_success_proof() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-cancel-success-proof");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-success".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        claim_reconnect_at(&journal, "daemon-a")
            .expect("claim")
            .expect("request");
        let ack = ReconnectAck {
            nonce: request.nonce.clone(),
            accepted: true,
            message: "authenticated success".to_string(),
        };
        publish_reconnect_success_proof(&journal, &ack).expect("publish proof");
        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Settled { ack: ack.clone() },
        )
        .expect("settle");

        let error = cancel_reconnect_at(&journal, &request.nonce, "daemon-a")
            .expect_err("success proof must refuse cancellation");

        assert!(error.report().message.contains("success proof"));
        assert!(reconnect_success_proof_matches(&journal, &ack).expect("proof remains"));
        assert!(matches!(
            load_reconnect_journal_at(&journal)
                .expect("load")
                .expect("state")
                .phase,
            ReconnectPhase::Settled(ReconnectAck { accepted: true, .. })
        ));
    }

    #[test]
    fn supersession_settles_the_old_nonce_before_accepting_the_replacement() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-supersession-order");
        let journal = reconnect_journal_path();
        let old = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-old".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &old).expect("submit old");

        let replacement = supersede_reconnect_at(&journal, &old.nonce, "nonce-new", "daemon-a")
            .expect("supersede old request");

        assert_eq!(replacement.nonce, "nonce-new");
        assert_eq!(replacement.expected_profile, "home");
        let old_status = reconnect_status_at(&journal, "nonce-old")
            .expect("old status")
            .expect("retained old result");
        assert_eq!(old_status.state, ReconnectLifecycle::Superseded);
        assert_eq!(old_status.replacement_nonce.as_deref(), Some("nonce-new"));
        let current = load_reconnect_journal_at(&journal)
            .expect("load replacement")
            .expect("replacement journal");
        assert_eq!(current.request.nonce, "nonce-new");
    }

    #[test]
    fn supersession_retry_recreates_a_replacement_after_old_terminal_receipt_was_retained() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-supersession-retry");
        let journal = reconnect_journal_path();
        let old = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-old-retry".to_string(),
            expected_profile: "home".to_string(),
        };
        let event = ReconnectJournalEvent::Superseded {
            ack: ReconnectAck {
                nonce: old.nonce.clone(),
                accepted: false,
                message: "superseded".to_string(),
            },
            replacement_nonce: "nonce-new-retry".to_string(),
        };
        let receipt = reconnect_receipt_path_at(&journal, &old.nonce).expect("receipt");
        preserve_reconnect_terminal_event_at_with(&receipt, &event, Some(&old))
            .expect("retain old terminal result");

        let replacement =
            supersede_reconnect_at(&journal, &old.nonce, "nonce-new-retry", "daemon-a")
                .expect("retry supersession");

        assert_eq!(replacement.expected_profile, "home");
        assert_eq!(
            load_reconnect_journal_at(&journal)
                .expect("load replacement")
                .expect("replacement journal")
                .request
                .nonce,
            "nonce-new-retry"
        );
    }

    #[test]
    fn retention_cleanup_reaps_only_a_complete_expired_terminal_record() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-expired");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "nonce-expired").expect("receipt");
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-expired".to_string(),
            expected_profile: "home".to_string(),
        };
        let lines = [
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION,
                "event": "submitted",
                "timestamp": 1,
                "instance": request.instance,
                "nonce": request.nonce,
                "expected_profile": request.expected_profile,
            }),
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION,
                "event": "settled",
                "timestamp": 1,
                "nonce": "nonce-expired",
                "accepted": false,
                "message": "expired failure",
            }),
        ];
        std::fs::create_dir_all(journal.parent().expect("control directory"))
            .expect("control directory");
        std::fs::write(
            &journal,
            lines
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .expect("journal");
        std::fs::create_dir_all(receipt.parent().expect("receipt directory"))
            .expect("receipt directory");
        std::fs::write(
            &receipt,
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION,
                "event": "settled",
                "timestamp": 1,
                "nonce": "nonce-expired",
                "accepted": false,
                "message": "expired failure",
            })
            .to_string(),
        )
        .expect("receipt");

        let removed = reap_expired_reconnect_records_at(
            &journal,
            "daemon-a",
            RECONNECT_TERMINAL_RETENTION.as_millis() as u64 + 2,
        )
        .expect("reap expired record");

        assert_eq!(removed, 1);
        assert!(!journal.exists());
        assert!(!receipt.exists());
    }

    #[test]
    fn retention_cleanup_never_reaps_an_active_request() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-active");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-active".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit active request");

        let removed = reap_expired_reconnect_records_at(&journal, "daemon-a", u64::MAX)
            .expect("inspect active record");

        assert_eq!(removed, 0);
        assert!(journal.exists());
        assert!(matches!(
            load_reconnect_journal_at(&journal)
                .expect("load active journal")
                .expect("active journal")
                .phase,
            ReconnectPhase::Submitted
        ));
    }

    #[test]
    fn retention_cleanup_uses_terminal_event_timestamp_without_a_journal() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-orphan-receipt");
        let journal = reconnect_journal_path();
        let receipt =
            reconnect_receipt_path_at(&journal, "nonce-orphan-receipt").expect("receipt path");
        std::fs::create_dir_all(receipt.parent().expect("receipt directory"))
            .expect("receipt directory");
        std::fs::write(
            &receipt,
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION,
                "event": "settled",
                "timestamp": 1,
                "nonce": "nonce-orphan-receipt",
                "accepted": false,
                "message": "orphaned failure",
            })
            .to_string(),
        )
        .expect("receipt");

        let removed = reap_expired_reconnect_records_at(
            &journal,
            "daemon-a",
            RECONNECT_TERMINAL_RETENTION.as_millis() as u64 + 2,
        )
        .expect("reap orphaned receipt");

        assert_eq!(removed, 1);
        assert!(!receipt.exists());
    }

    #[test]
    fn observing_a_terminal_result_does_not_end_its_retention_period() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-observe");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-retained-after-observe".to_string(),
            expected_profile: "home".to_string(),
        };
        submit_reconnect_at(&journal, &request).expect("submit");
        let ack = ReconnectAck {
            nonce: request.nonce.clone(),
            accepted: false,
            message: "terminal failure".to_string(),
        };
        append_reconnect_event_at(
            &journal,
            &ReconnectJournalEvent::Settled { ack: ack.clone() },
        )
        .expect("settle");
        let receipt = reconnect_receipt_path_at(&journal, &request.nonce).expect("receipt");
        preserve_reconnect_receipt_at(&receipt, &ack).expect("receipt");

        assert_eq!(
            take_reconnect_ack_at(&journal, &request.nonce).expect("observe result"),
            Some(ack.clone())
        );
        assert!(
            receipt.exists(),
            "observation must retain the terminal receipt"
        );
        assert_eq!(
            take_reconnect_ack_at(&journal, &request.nonce).expect("observe retained result"),
            Some(ack)
        );
        assert_eq!(
            reconnect_status_at(&journal, &request.nonce)
                .expect("status after observation")
                .expect("retained status")
                .state,
            ReconnectLifecycle::Settled
        );
    }

    #[test]
    fn retention_cleanup_reaps_an_expired_success_receipt_and_proof_together() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-success-proof");
        let journal = reconnect_journal_path();
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-expired-success".to_string(),
            expected_profile: "home".to_string(),
        };
        let timestamp = 1_u64;
        let terminal = serde_json::json!({
            "version": RECONNECT_CONTROL_VERSION,
            "event": "settled",
            "timestamp": timestamp,
            "nonce": request.nonce,
            "accepted": true,
            "message": "authenticated success",
        });
        std::fs::create_dir_all(journal.parent().expect("control directory"))
            .expect("control directory");
        std::fs::write(
            &journal,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "version": RECONNECT_CONTROL_VERSION,
                    "event": "submitted",
                    "timestamp": timestamp,
                    "instance": "daemon-a",
                    "nonce": "nonce-expired-success",
                    "expected_profile": "home",
                }),
                terminal
            ),
        )
        .expect("journal");
        let receipt =
            reconnect_receipt_path_at(&journal, "nonce-expired-success").expect("receipt");
        let proof =
            reconnect_success_proof_path_at(&journal, "nonce-expired-success").expect("proof");
        for path in [&receipt, &proof] {
            std::fs::create_dir_all(path.parent().expect("record directory"))
                .expect("record directory");
            std::fs::write(path, terminal.to_string()).expect("terminal record");
        }

        let removed = reap_expired_reconnect_records_at(
            &journal,
            "daemon-a",
            RECONNECT_TERMINAL_RETENTION.as_millis() as u64 + 2,
        )
        .expect("reap success record");

        assert_eq!(removed, 1);
        assert!(!journal.exists());
        assert!(!receipt.exists());
        assert!(!proof.exists());
    }

    #[test]
    fn retention_cleanup_restores_all_terminal_files_after_a_partial_reap_failure() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-rollback");
        let journal = reconnect_journal_path();
        let receipt = reconnect_receipt_path_at(&journal, "nonce-rollback").expect("receipt");
        let proof = reconnect_success_proof_path_at(&journal, "nonce-rollback").expect("proof");
        let paths = [journal.as_path(), receipt.as_path(), proof.as_path()];
        for (index, path) in paths.iter().enumerate() {
            std::fs::create_dir_all(path.parent().expect("record directory"))
                .expect("record directory");
            std::fs::write(path, format!("record-{index}")).expect("record");
        }
        let before: Vec<Vec<u8>> = paths
            .iter()
            .map(|path| std::fs::read(path).expect("read record"))
            .collect();

        let result = reap_reconnect_files_at_with(&paths, |path| {
            if path == proof {
                return Err(reconnect_journal_error("injected proof cleanup failure"));
            }
            std::fs::remove_file(path).expect("remove staged record");
            Ok(())
        });

        assert!(result.is_err());
        for (path, expected) in paths.iter().zip(before) {
            assert_eq!(std::fs::read(path).expect("restored record"), expected);
        }
    }

    #[test]
    fn retention_cleanup_reaps_an_expired_orphan_success_proof() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-retention-orphan-proof");
        let journal = reconnect_journal_path();
        let proof =
            reconnect_success_proof_path_at(&journal, "nonce-orphan-proof").expect("proof path");
        std::fs::create_dir_all(proof.parent().expect("proof directory")).expect("proof directory");
        std::fs::write(
            &proof,
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION,
                "event": "settled",
                "timestamp": 1,
                "nonce": "nonce-orphan-proof",
                "accepted": true,
                "message": "authenticated success",
            })
            .to_string(),
        )
        .expect("orphan proof");

        let removed = reap_expired_reconnect_records_at(
            &journal,
            "daemon-a",
            RECONNECT_TERMINAL_RETENTION.as_millis() as u64 + 2,
        )
        .expect("reap orphan proof");

        assert_eq!(removed, 1);
        assert!(!proof.exists());
    }

    #[test]
    fn ambiguous_proof_quarantine_stops_after_bounded_retries() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-quarantine-bounded");
        let path = reconnect_success_proof_path_at(&reconnect_journal_path(), "nonce-proof")
            .expect("proof path");
        std::fs::create_dir_all(path.parent().expect("proof directory")).expect("proof dir");
        std::fs::write(&path, b"ambiguous").expect("proof");
        let waits = std::cell::Cell::new(0usize);

        let error = quarantine_ambiguous_success_proof_with(
            &path,
            |_, _| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            |_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            |_| Err(reconnect_journal_error("directory unavailable")),
            || waits.set(waits.get() + 1),
        )
        .expect_err("permanent quarantine failure must return");

        assert!(error.report().message.contains("retries"));
        assert!(
            waits.get() <= RECONNECT_HOUSEKEEPING_RETRIES,
            "bounded retry count must not grow without a limit"
        );
        assert!(path.exists(), "failed quarantine retains the proof");
    }

    #[test]
    fn shutdown_settlement_returns_after_bounded_retries() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let request = ReconnectRequest {
                instance: "daemon-a".to_string(),
                nonce: "nonce-shutdown".to_string(),
                expected_profile: "home".to_string(),
            };
            let mut pending = Some(PendingReconnect::new(request));
            decide_pending_reconnect(&mut pending, false, "shutdown failure".to_string());
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(250),
                settle_pending_reconnect_before_exit_with(&mut pending, |_| {
                    Err(reconnect_journal_error("filesystem unavailable"))
                }),
            )
            .await;

            assert!(
                result.is_ok(),
                "shutdown housekeeping must not retry forever"
            );
            assert!(
                pending.is_some(),
                "the failed settlement remains visible for recovery"
            );
        });
    }

    #[test]
    fn ordinary_service_settlement_retry_is_bounded() {
        let request = ReconnectRequest {
            instance: "daemon-a".to_string(),
            nonce: "nonce-service-retry".to_string(),
            expected_profile: "home".to_string(),
        };
        let mut pending = Some(PendingReconnect::new(request));
        decide_pending_reconnect(&mut pending, false, "settlement unavailable".to_string());
        let started = std::time::Instant::now();

        let result = settle_pending_reconnect_bounded_with(&mut pending, |_| {
            Err(reconnect_journal_error("filesystem unavailable"))
        });

        assert!(result.is_err());
        assert!(pending.is_some());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "service settlement retry must not spin indefinitely"
        );
    }

    #[test]
    fn reconnect_caller_budget_includes_the_complete_sweep_and_settlement_margin() {
        assert_eq!(
            RECONNECT_ACK_WAIT,
            RECONNECT_SWEEP_BUDGET
                + std::time::Duration::from_millis(RECONNECT_SETTLEMENT_MARGIN_MILLIS,)
        );
        assert!(RECONNECT_ACK_WAIT > RECONNECT_SWEEP_BUDGET);
        assert_eq!(CONNECT_ENDPOINT_WAIT, std::time::Duration::from_secs(15));
    }

    #[test]
    fn reconnect_command_exposes_nonce_lifecycle_and_control_operations() {
        for argv in [
            vec!["fleetyd", "reconnect", "status", "--nonce", "n-1"],
            vec!["fleetyd", "reconnect", "status", "--nonce", "n-1", "--json"],
            vec!["fleetyd", "reconnect", "cancel", "--nonce", "n-1"],
            vec![
                "fleetyd",
                "reconnect",
                "supersede",
                "--nonce",
                "n-1",
                "--replacement",
                "n-2",
            ],
            vec!["fleetyd", "reconnect", "control", "inspect", "--json"],
            vec!["fleetyd", "reconnect", "control", "recover", "--confirm"],
            vec!["fleetyd", "reconnect", "retention", "--reap"],
        ] {
            command()
                .try_get_matches_from(argv)
                .expect("reconnect lifecycle command is accepted");
        }
    }

    #[test]
    fn reconnect_control_inspection_is_read_only_and_recovery_requires_confirmation() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-control-inspection");
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "dead-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("control directory");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("ready record");
        submit_reconnect_at(
            &reconnect_journal_path(),
            &ReconnectRequest {
                instance: "daemon-a".to_string(),
                nonce: "nonce-inspected".to_string(),
                expected_profile: "home".to_string(),
            },
        )
        .expect("request record");
        let before = std::fs::read(ready_path()).expect("ready before inspect");

        let inspected = inspect_reconnect_control_value().expect("inspect control");

        assert_eq!(
            inspected["ready"]["process_start"].as_str(),
            Some("dead-process-start")
        );
        assert_eq!(
            inspected["ready"]["owner_generation"].as_str(),
            Some("daemon-a")
        );
        assert_eq!(inspected["nonce"].as_str(), Some("nonce-inspected"));
        assert_eq!(
            std::fs::read(ready_path()).expect("ready after inspect"),
            before
        );
        let error = recover_stale_reconnect_control(false).expect_err("confirmation required");
        assert!(error.report().message.contains("--confirm"));
        assert!(ready_path().exists(), "refusal preserves control artifacts");
    }

    #[test]
    fn stale_recovery_preserves_control_when_the_journal_contract_is_incompatible() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-recovery-version-mismatch");
        let ready = ControlReady {
            pid: u32::MAX,
            process_start: "dead-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("control directory");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("ready record");
        let journal = reconnect_journal_path();
        std::fs::write(
            &journal,
            serde_json::json!({
                "version": RECONNECT_CONTROL_VERSION + 1,
                "event": "submitted",
                "timestamp": 1,
                "instance": "daemon-a",
                "nonce": "nonce-incompatible",
                "expected_profile": "home",
            })
            .to_string()
                + "\n",
        )
        .expect("incompatible journal");
        assert!(journal.exists());
        assert!(load_reconnect_journal_at(&journal).is_err());

        let error = recover_stale_reconnect_control(true)
            .expect_err("incompatible journal must block destructive recovery");

        assert!(error.report().message.contains("version"));
        assert!(ready_path().exists());
    }

    #[test]
    fn reconnect_control_recovery_refuses_a_live_owner_even_with_confirmation() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reconnect-control-live-recovery");
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "live-process-start".to_string(),
            instance: "daemon-a".to_string(),
        };
        let identity = fleety_tools::service::claim_process_identity_at(
            &process_identity_path(&ready.process_start),
            &ready.process_start,
        )
        .expect("claim live identity");
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("control directory");
        std::fs::write(ready_path(), encode_ready(&ready)).expect("ready record");

        let error = recover_stale_reconnect_control(true).expect_err("live owner is protected");

        assert!(error.report().message.contains("live"));
        assert!(ready_path().exists());
        drop(identity);
    }

    #[test]
    fn same_pid_with_a_different_start_identity_is_not_a_live_owner() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("reused-pid-ready-owner");
        std::fs::create_dir_all(ready_path().parent().expect("control directory"))
            .expect("create control directory");
        let stale = ControlReady {
            pid: std::process::id(),
            process_start: "stale-process-start".to_string(),
            instance: "stale-daemon".to_string(),
        };
        std::fs::write(ready_path(), encode_ready(&stale)).expect("seed stale ready");

        let guard = ControlGuard::claim()
            .expect("a reused pid without the matching start-identity lock is stale");
        assert_ne!(guard.ready.process_start, stale.process_start);
    }

    #[test]
    fn ready_publication_failure_matrix_never_leaves_an_ambiguous_owner() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("ready-publication-crash-matrix");
        let path = ready_path();
        let ready = ControlReady {
            pid: std::process::id(),
            process_start: "start-a".to_string(),
            instance: "daemon-a".to_string(),
        };

        let stage_error = publish_ready_at_with(
            &path,
            &ready,
            |_, _| Err(agent_core::CoreError::Message("stage crash".to_string())),
            |_, _| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect_err("staging failure");
        assert!(stage_error.report().message.contains("stage crash"));
        assert!(!path.exists());

        let rename_error = publish_ready_at_with(
            &path,
            &ready,
            |temp, bytes| {
                std::fs::write(temp, bytes)
                    .map_err(|error| agent_core::CoreError::Message(error.to_string()))
            },
            |_, _| Err(agent_core::CoreError::Message("rename crash".to_string())),
            |_| Ok(()),
            |_| Ok(()),
            |target| match std::fs::remove_file(target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(agent_core::CoreError::Message(error.to_string())),
            },
        )
        .expect_err("rename failure");
        assert!(rename_error.report().message.contains("rename crash"));
        assert!(!path.exists());

        let flush_error = publish_ready_at_with(
            &path,
            &ready,
            |temp, bytes| {
                std::fs::write(temp, bytes)
                    .map_err(|error| agent_core::CoreError::Message(error.to_string()))
            },
            |temp, target| {
                std::fs::rename(temp, target)
                    .map_err(|error| agent_core::CoreError::Message(error.to_string()))
            },
            |_| {
                Err(agent_core::CoreError::Message(
                    "canonical flush crash".to_string(),
                ))
            },
            |_| Ok(()),
            |target| match std::fs::remove_file(target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(agent_core::CoreError::Message(error.to_string())),
            },
        )
        .expect_err("post-rename canonical flush failure");
        assert!(flush_error
            .report()
            .message
            .contains("canonical flush crash"));
        assert!(
            !path.exists(),
            "a failed canonical flush must hide the renamed ready record"
        );

        let sync_calls = std::cell::Cell::new(0usize);
        let sync_error = publish_ready_at_with(
            &path,
            &ready,
            |temp, bytes| {
                std::fs::write(temp, bytes)
                    .map_err(|error| agent_core::CoreError::Message(error.to_string()))
            },
            |temp, target| {
                std::fs::rename(temp, target)
                    .map_err(|error| agent_core::CoreError::Message(error.to_string()))
            },
            |_| Ok(()),
            |_| {
                let call = sync_calls.get();
                sync_calls.set(call + 1);
                if call == 0 {
                    Err(agent_core::CoreError::Message(
                        "directory sync crash".to_string(),
                    ))
                } else {
                    Ok(())
                }
            },
            |target| {
                std::fs::remove_file(target)
                    .map_err(|error| agent_core::CoreError::Message(error.to_string()))
            },
        )
        .expect_err("post-rename sync failure");
        assert!(sync_error.report().message.contains("directory sync crash"));
        assert_eq!(sync_calls.get(), 2, "absence must be synced after cleanup");
        assert!(!path.exists());
    }
}
