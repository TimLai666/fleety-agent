//! Defer a fleety-server restart until it is idle.
//!
//! An external `fleety-server restart` runs in a *separate* process and can't see
//! the running server's in-flight state, so a non-forced restart doesn't stop the
//! service through the manager. Instead it drops a *restart-request* marker file
//! in the runtime dir (`~/.fleety/<name>.restart-request`); the running server's
//! [`spawn_watcher`] task consumes it and, using the shared [`restart::decide`]
//! policy, restarts only once idle (in-flight turn count == 0) or the deferral
//! deadline passes. `--force` and "no running server" skip the marker and restart
//! immediately (see `service.rs`).
//!
//! Idle is a process-wide in-flight turn count: [`turn_guard`] bumps it for the
//! life of a turn (interactive or schedule-fired) and drops it on every exit path
//! (including `?`/error unwinds), so the count is never left stuck above zero.
//!
//! The marker stores the request's **wall-clock** timestamp (unix seconds)
//! because it crosses processes — monotonic `Instant`s aren't comparable between
//! the requesting CLI and the server. [`decide_request`] bridges that wall-clock
//! stamp back onto the monotonic clock the policy uses.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_core::{CoreError, Result};
use fleety_tools::restart::{self, Decision, PendingRestart, DEFERRAL_CAP};
use fleety_tools::service::{self, ServiceSpec, Verb};

/// How often the watcher checks for a pending restart request.
const WATCH_PERIOD: Duration = Duration::from_secs(2);

/// Process-wide count of turns currently executing. Idle == 0.
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// RAII guard: a turn is in flight while this is alive. Created by [`turn_guard`];
/// dropping it (any exit path) decrements the count. Never left dangling — the
/// count is corrected by `Drop`, not by manual pairing.
pub(crate) struct TurnGuard(&'static AtomicUsize);

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Bump `counter` for the returned guard's lifetime (testable core of
/// [`turn_guard`], which binds it to the process-wide [`IN_FLIGHT`]).
fn acquire(counter: &'static AtomicUsize) -> TurnGuard {
    counter.fetch_add(1, Ordering::SeqCst);
    TurnGuard(counter)
}

/// Count the calling scope as an in-flight turn until the returned guard drops.
/// Hold it across a turn's whole execution (recovery + the turn itself).
pub(crate) fn turn_guard() -> TurnGuard {
    acquire(&IN_FLIGHT)
}

/// Whether no turn is currently in flight (the idle signal for [`restart::decide`]).
pub(crate) fn is_idle() -> bool {
    IN_FLIGHT.load(Ordering::SeqCst) == 0
}

/// Current wall-clock time in whole seconds since the unix epoch.
fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record a non-forced restart request for `name` by dropping its marker, stamped
/// with the current wall-clock time. If a request is already pending we keep the
/// existing (earlier) stamp so repeated requests don't push the deadline out.
/// Called by the requesting CLI process, not the server.
pub(crate) fn request_restart(name: &str) -> Result<()> {
    request_restart_at(&service::restart_request_path(name))
}

/// The path-based core of [`request_restart`], split out so tests can operate on
/// an explicit path instead of mutating the process-global HOME env (which would
/// race other crates' tests under `cargo test --workspace`).
fn request_restart_at(path: &Path) -> Result<()> {
    if path.exists() {
        // A restart is already pending; don't reset its deferral deadline.
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(path, now_unix_secs().to_string()).map_err(|e| {
        CoreError::Message(format!(
            "cannot write restart request {}: {e}",
            path.display()
        ))
    })?;
    Ok(())
}

/// Read a pending restart request's timestamp, if any. `None` when the marker is
/// missing or unparsable (treated as "no request").
fn read_request(path: &Path) -> Option<SystemTime> {
    let secs: u64 = std::fs::read_to_string(path).ok()?.trim().parse().ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// Delete any restart-request marker left over from before this process started.
/// Called once at server startup: a marker present at boot was written for a
/// restart that has *already* happened (this very start), so acting on it would
/// loop the service. Best-effort — a failure to clear is logged, not fatal.
pub(crate) fn clear_stale_request(name: &str) {
    clear_stale_request_at(&service::restart_request_path(name));
}

/// Path-based core of [`clear_stale_request`] (see [`request_restart_at`] for why
/// tests take an explicit path).
fn clear_stale_request_at(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::info!("cleared stale restart-request marker at startup"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(%e, "could not clear stale restart-request marker"),
    }
}

/// Decide whether a pending request stamped at wall-clock `requested_at` should be
/// carried out now. Bridges the cross-process wall-clock stamp onto the monotonic
/// clock the policy uses: the deferral deadline is `now` plus whatever remains of
/// [`DEFERRAL_CAP`] after the time already elapsed since the request. Then defers
/// to the shared [`restart::decide`] (busy → wait, idle → now unless in cooldown,
/// past deadline → now).
fn decide_request(
    requested_at: SystemTime,
    now_wall: SystemTime,
    now: Instant,
    idle: bool,
    last_restart: Option<Instant>,
) -> Decision {
    let elapsed = now_wall
        .duration_since(requested_at)
        .unwrap_or(Duration::ZERO);
    let pending = PendingRestart {
        reason: "external restart request".to_string(),
        force: false,
        deadline: now + DEFERRAL_CAP.saturating_sub(elapsed),
    };
    restart::decide(&pending, idle, last_restart, now)
}

/// Spawn the deferred-restart watcher: poll for a restart-request marker and,
/// when [`decide_request`] says so, restart `spec` through the service manager.
/// This is the **only** place a marker triggers a manager restart — a repeated
/// marker is consumed once (removed before the restart). A failed manager restart
/// is logged and the loop continues (never a busy-loop), mirroring fleetyd.
pub(crate) fn spawn_watcher(spec: ServiceSpec) {
    tokio::spawn(async move {
        let mut last_restart: Option<Instant> = None;
        let mut ticker = tokio::time::interval(WATCH_PERIOD);
        loop {
            ticker.tick().await;
            let path = service::restart_request_path(&spec.name);
            let Some(requested_at) = read_request(&path) else {
                continue; // no pending request
            };
            let now = Instant::now();
            match decide_request(
                requested_at,
                SystemTime::now(),
                now,
                is_idle(),
                last_restart,
            ) {
                Decision::Wait => {}
                Decision::RestartNow => {
                    // Consume the marker before restarting so the fresh process
                    // never re-acts on it (belt-and-braces with clear_stale_request).
                    let _ = std::fs::remove_file(&path);
                    last_restart = Some(now);
                    tracing::info!("carrying out deferred restart (server idle or past deadline)");
                    if let Err(e) = service::run_verb(&spec, Verb::Restart) {
                        tracing::warn!(report = ?e.report(), "deferred restart: manager restart failed");
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- in-flight turn accounting (idle definition) ----

    #[test]
    fn guard_add_sub_is_balanced_and_nests() {
        static C: AtomicUsize = AtomicUsize::new(0);
        assert_eq!(C.load(Ordering::SeqCst), 0);
        {
            let _g1 = acquire(&C);
            assert_eq!(C.load(Ordering::SeqCst), 1);
            {
                let _g2 = acquire(&C);
                assert_eq!(C.load(Ordering::SeqCst), 2);
            }
            assert_eq!(C.load(Ordering::SeqCst), 1);
        }
        assert_eq!(C.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn guard_drops_on_early_return_and_error_paths() {
        static C: AtomicUsize = AtomicUsize::new(0);
        fn work(c: &'static AtomicUsize, fail: bool) -> std::result::Result<(), ()> {
            let _g = acquire(c);
            if fail {
                return Err(()); // early error path still drops the guard
            }
            Ok(())
        }
        let _ = work(&C, true);
        assert_eq!(C.load(Ordering::SeqCst), 0, "guard drops on the error path");
        let _ = work(&C, false);
        assert_eq!(C.load(Ordering::SeqCst), 0, "guard drops on the ok path");
    }

    #[test]
    fn holding_a_turn_guard_is_never_idle() {
        // Sound under concurrency: a live guard means count >= 1, so not idle
        // regardless of any other test's concurrent guards.
        let _g = turn_guard();
        assert!(!is_idle(), "an in-flight turn must not read as idle");
    }

    // ---- marker channel ----

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fleety-restartwatch-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("mk temp home");
        dir
    }

    #[test]
    fn request_written_then_readable_then_cleared() {
        // Operate on an explicit unique path — never mutate the process-global
        // HOME env, which races other crates' tests under `cargo test --workspace`.
        let dir = temp_home("marker");
        let path = dir.join("fleety-server-test.restart-request");

        assert!(read_request(&path).is_none(), "no marker yet");
        request_restart_at(&path).expect("write request");
        assert!(read_request(&path).is_some(), "marker readable after write");

        // A repeat request keeps the same (earlier) stamp: the file is untouched.
        let first = std::fs::read_to_string(&path).expect("read1");
        request_restart_at(&path).expect("second request");
        let second = std::fs::read_to_string(&path).expect("read2");
        assert_eq!(first, second, "repeat request must not reset the stamp");

        clear_stale_request_at(&path);
        assert!(read_request(&path).is_none(), "marker gone after clear");
        // Clearing a missing marker is a no-op (no panic).
        clear_stale_request_at(&path);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- deferred-restart decision (reuses restart::decide) ----

    #[test]
    fn busy_request_waits() {
        let t0 = Instant::now();
        let now_wall = SystemTime::now();
        // Fresh request, server busy → wait.
        assert_eq!(
            decide_request(now_wall, now_wall, t0, false, None),
            Decision::Wait
        );
    }

    #[test]
    fn idle_request_restarts_now() {
        let t0 = Instant::now();
        let now_wall = SystemTime::now();
        assert_eq!(
            decide_request(now_wall, now_wall, t0, true, None),
            Decision::RestartNow
        );
    }

    #[test]
    fn past_deadline_restarts_even_when_busy() {
        let t0 = Instant::now();
        // Requested 301s ago (> DEFERRAL_CAP of 300s), still busy → restart anyway.
        let requested_at = SystemTime::now() - Duration::from_secs(301);
        let now_wall = SystemTime::now();
        assert_eq!(
            decide_request(requested_at, now_wall, t0, false, None),
            Decision::RestartNow
        );
    }

    #[test]
    fn idle_within_cooldown_waits() {
        let t0 = Instant::now();
        let now_wall = SystemTime::now();
        // Idle, before deadline, but only 10s since the last restart (< 30s cooldown).
        let last = t0 - Duration::from_secs(10);
        assert_eq!(
            decide_request(now_wall, now_wall, t0, true, Some(last)),
            Decision::Wait
        );
    }
}
