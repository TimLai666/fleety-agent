//! Defer-restart-until-idle: a restart (manual or self-update-triggered) must
//! not interrupt in-flight work. Instead of restarting immediately we record a
//! *pending* restart and act on it only when the service is idle — bounded by a
//! deferral deadline (restart anyway after this) and a cooldown (don't thrash).
//! A `force` restart bypasses the deferral. Borrowed from openclaw's
//! `src/infra/restart.ts` (pending intent + defer-until-idle + cooldown).
//!
//! The decision is pure ([`decide`]); the idle signal and the actual manager
//! restart are supplied by the caller. Today only fleetyd's self-update path
//! wires this up (idle = no running on-device tool); fleety-server does not
//! yet — its manual `restart` verb and update-triggered restarts are immediate
//! (an interrupted turn is recovered from the journal, not lost).

use std::time::{Duration, Instant};

/// Restart anyway once a pending restart has waited this long, even if busy.
pub const DEFERRAL_CAP: Duration = Duration::from_secs(300);
/// Minimum gap between restarts (a non-forced restart waits this out).
pub const COOLDOWN: Duration = Duration::from_secs(30);

/// A restart that has been requested but not yet carried out.
#[derive(Debug, Clone)]
pub struct PendingRestart {
    pub reason: String,
    /// A forced restart (explicit manual `restart`) ignores deferral + cooldown.
    pub force: bool,
    /// After this instant, restart even if still busy.
    pub deadline: Instant,
}

impl PendingRestart {
    /// Record a restart requested at `requested_at` (deadline = +[`DEFERRAL_CAP`]).
    pub fn new(reason: impl Into<String>, force: bool, requested_at: Instant) -> Self {
        Self {
            reason: reason.into(),
            force,
            deadline: requested_at + DEFERRAL_CAP,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Carry out the restart now.
    RestartNow,
    /// Keep waiting (busy, and neither forced nor past the deadline / cooldown).
    Wait,
}

/// Decide whether to act on a pending restart at `now`.
///
/// - `force` → always now (bypasses idle, deadline, and cooldown)
/// - past the `deadline` → now, even if busy or within cooldown (hard stop)
/// - `idle` → now, unless still within `COOLDOWN` of `last_restart`
/// - otherwise → wait
pub fn decide(
    p: &PendingRestart,
    idle: bool,
    last_restart: Option<Instant>,
    now: Instant,
) -> Decision {
    if p.force {
        return Decision::RestartNow;
    }
    let past_deadline = now >= p.deadline;
    if past_deadline {
        return Decision::RestartNow;
    }
    if !idle {
        return Decision::Wait;
    }
    // Idle and before the deadline: honor the cooldown so we don't thrash.
    if let Some(last) = last_restart {
        if now.duration_since(last) < COOLDOWN {
            return Decision::Wait;
        }
    }
    Decision::RestartNow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn busy_and_unforced_waits() {
        let t0 = Instant::now();
        let p = PendingRestart::new("update", false, t0);
        assert_eq!(decide(&p, false, None, at(t0, 5)), Decision::Wait);
    }

    #[test]
    fn idle_restarts_now() {
        let t0 = Instant::now();
        let p = PendingRestart::new("update", false, t0);
        assert_eq!(decide(&p, true, None, at(t0, 5)), Decision::RestartNow);
    }

    #[test]
    fn force_restarts_even_when_busy() {
        let t0 = Instant::now();
        let p = PendingRestart::new("manual", true, t0);
        assert_eq!(decide(&p, false, None, at(t0, 1)), Decision::RestartNow);
    }

    #[test]
    fn past_deadline_restarts_even_when_busy() {
        let t0 = Instant::now();
        let p = PendingRestart::new("update", false, t0);
        // DEFERRAL_CAP is 300s; 301s in we restart despite being busy.
        assert_eq!(decide(&p, false, None, at(t0, 301)), Decision::RestartNow);
    }

    #[test]
    fn idle_within_cooldown_waits() {
        let t0 = Instant::now();
        let p = PendingRestart::new("update", false, t0);
        // Idle, before deadline, but only 10s since the last restart (< 30s).
        let last = at(t0, 0);
        assert_eq!(decide(&p, true, Some(last), at(t0, 10)), Decision::Wait);
    }

    #[test]
    fn idle_after_cooldown_restarts() {
        let t0 = Instant::now();
        let p = PendingRestart::new("update", false, t0);
        let last = at(t0, 0);
        assert_eq!(
            decide(&p, true, Some(last), at(t0, 31)),
            Decision::RestartNow
        );
    }

    #[test]
    fn past_deadline_ignores_cooldown() {
        let t0 = Instant::now();
        let p = PendingRestart::new("update", false, t0);
        let last = at(t0, 290);
        // Within cooldown of last_restart but past the deferral deadline → restart.
        assert_eq!(
            decide(&p, false, Some(last), at(t0, 301)),
            Decision::RestartNow
        );
    }
}
