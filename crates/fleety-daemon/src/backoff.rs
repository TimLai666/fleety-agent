//! Exponential reconnect backoff for fleetyd.
//!
//! When the connection to the server drops (or the device sleeps and wakes),
//! fleetyd retries with `base * factor^(n-1)` capped at `cap`, plus ±jitter so a
//! fleet of devices doesn't reconnect in lockstep. A successful connect resets
//! the sequence. The delay maths is pure and unit-tested; the loop that uses it
//! lives in `main.rs`.

use std::time::Duration;

pub const BASE: Duration = Duration::from_secs(1);
pub const FACTOR: u32 = 2;
pub const CAP: Duration = Duration::from_secs(30);
pub const JITTER_FRAC: f64 = 0.2;

/// Base (un-jittered) delay before the `attempt`-th consecutive retry (attempt
/// starts at 1): `base * factor^(attempt-1)`, capped at `cap`. Saturating, so a
/// large attempt count never overflows.
pub fn base_delay(attempt: u32, base: Duration, factor: u32, cap: Duration) -> Duration {
    if attempt <= 1 {
        return base.min(cap);
    }
    let mut delay = base;
    for _ in 1..attempt {
        delay = delay.saturating_mul(factor);
        if delay >= cap {
            return cap;
        }
    }
    delay.min(cap)
}

/// Apply ±`frac` jitter to `d` given a uniform sample `unit` in [0,1]:
/// `d * (1 - frac + 2*frac*unit)`. With `unit` at 0/0.5/1 the result is
/// `d*(1-frac)` / `d` / `d*(1+frac)`.
pub fn with_jitter(d: Duration, frac: f64, unit: f64) -> Duration {
    let unit = unit.clamp(0.0, 1.0);
    let factor = ((1.0 - frac) + 2.0 * frac * unit).max(0.0);
    Duration::from_secs_f64(d.as_secs_f64() * factor)
}

/// Stateful reconnect backoff tracking consecutive failures.
pub struct Backoff {
    attempt: u32,
    base: Duration,
    factor: u32,
    cap: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Self {
            attempt: 0,
            base: BASE,
            factor: FACTOR,
            cap: CAP,
        }
    }

    /// Reset after a successful connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Count one failure and return the base (un-jittered) delay to wait.
    pub fn next_base(&mut self) -> Duration {
        self.attempt = self.attempt.saturating_add(1);
        base_delay(self.attempt, self.base, self.factor, self.cap)
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// A jitter sample in [0,1) derived from the wall clock — good enough to
/// de-correlate reconnects without pulling in an RNG dependency.
pub fn jitter_unit() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000_000) as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_delay_matches_spec_example() {
        // base 1s, factor 2, cap 30s — the table in the spec.
        let cases = [(1, 1), (2, 2), (3, 4), (4, 8), (5, 16), (6, 30), (7, 30)];
        for (attempt, secs) in cases {
            assert_eq!(
                base_delay(attempt, BASE, FACTOR, CAP),
                Duration::from_secs(secs),
                "attempt {attempt}"
            );
        }
    }

    #[test]
    fn base_delay_saturates_for_huge_attempts() {
        assert_eq!(base_delay(1000, BASE, FACTOR, CAP), CAP);
    }

    #[test]
    fn jitter_bounds_are_plus_minus_frac() {
        let d = Duration::from_secs(10);
        assert_eq!(with_jitter(d, 0.2, 0.0), Duration::from_secs_f64(8.0));
        assert_eq!(with_jitter(d, 0.2, 0.5), Duration::from_secs_f64(10.0));
        assert_eq!(with_jitter(d, 0.2, 1.0), Duration::from_secs_f64(12.0));
    }

    #[test]
    fn backoff_grows_then_resets() {
        let mut b = Backoff::new();
        assert_eq!(b.next_base(), Duration::from_secs(1));
        assert_eq!(b.next_base(), Duration::from_secs(2));
        assert_eq!(b.next_base(), Duration::from_secs(4));
        b.reset();
        assert_eq!(b.next_base(), Duration::from_secs(1));
    }

    #[test]
    fn jitter_unit_in_range() {
        let u = jitter_unit();
        assert!((0.0..1.0).contains(&u));
    }
}
