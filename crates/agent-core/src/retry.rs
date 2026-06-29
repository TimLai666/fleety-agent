//! Resilient model-call retry: classify failures, back off, and retry the
//! transient ones.
//!
//! Model endpoints fail transiently — 429 (rate limit), 5xx, connection resets,
//! timeouts — and those are worth retrying with exponential backoff + jitter.
//! Other 4xx (auth, bad request) are not. The classification and the backoff
//! schedule are pure functions so they can be exhaustively unit-tested; the
//! driver ([`run_with_retry`]) owns the sleeping and the (time-derived) jitter.

use std::future::Future;
use std::time::Duration;

use crate::{CoreError, Result};

/// Whether a failed attempt is worth retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryable {
    Retry,
    Fatal,
}

/// Classify an attempt's failure. `transient` marks a connection/timeout error
/// (no HTTP status). Otherwise the HTTP status decides: 429/408/425 and 5xx are
/// retryable; other 4xx are fatal.
pub fn classify(status: Option<u16>, transient: bool) -> Retryable {
    if transient {
        return Retryable::Retry;
    }
    match status {
        Some(429 | 408 | 425 | 500 | 502 | 503 | 504) => Retryable::Retry,
        _ => Retryable::Fatal,
    }
}

/// Backoff before retry `attempt` (1-based). `Retry-After` (when present) wins,
/// capped. Otherwise exponential `base * 2^(attempt-1)`, capped, scaled by
/// `jitter_unit` in `[0, 1)` (full jitter). `jitter_unit` is injected so the
/// schedule is deterministic under test.
pub fn backoff_delay(
    attempt: u32,
    base: Duration,
    cap: Duration,
    retry_after: Option<Duration>,
    jitter_unit: f64,
) -> Duration {
    if let Some(ra) = retry_after {
        return ra.min(cap);
    }
    let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
    let exp = base.saturating_mul(factor).min(cap);
    exp.mul_f64(jitter_unit.clamp(0.0, 1.0))
}

/// Retry policy from env: `FLEETY_MODEL_RETRIES` (default 3; 0 disables retry),
/// `FLEETY_MODEL_RETRY_BASE_MS` (default 500), `FLEETY_MODEL_RETRY_CAP_MS`
/// (default 30000).
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub retries: u32,
    pub base: Duration,
    pub cap: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            retries: 3,
            base: Duration::from_millis(500),
            cap: Duration::from_millis(30_000),
        }
    }
}

impl RetryConfig {
    pub fn from_env() -> Self {
        let d = Self::default();
        let u32_env = |k: &str, default: u32| {
            std::env::var(k)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(default)
        };
        let ms_env = |k: &str, default: Duration| {
            std::env::var(k)
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or(default)
        };
        Self {
            retries: u32_env("FLEETY_MODEL_RETRIES", d.retries),
            base: ms_env("FLEETY_MODEL_RETRY_BASE_MS", d.base),
            cap: ms_env("FLEETY_MODEL_RETRY_CAP_MS", d.cap),
        }
    }
}

/// The result of one attempt: done, retryable (with optional Retry-After), or
/// fatal. The attempt classifies its own failure (it holds the HTTP status).
pub enum AttemptOutcome<T> {
    Done(T),
    Retry {
        err: CoreError,
        retry_after: Option<Duration>,
    },
    Fatal(CoreError),
}

/// A cheap jitter unit in `[0, 1)` derived from the clock (no rand dependency).
fn jitter_unit() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

/// Run `attempt` until it succeeds, returns a fatal error, or exhausts the
/// configured retries. `retries == 0` means a single attempt (prior behavior).
pub async fn run_with_retry<T, F, Fut>(cfg: &RetryConfig, mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = AttemptOutcome<T>>,
{
    let mut last_err: Option<CoreError> = None;
    for n in 0..=cfg.retries {
        match attempt().await {
            AttemptOutcome::Done(t) => return Ok(t),
            AttemptOutcome::Fatal(e) => return Err(e),
            AttemptOutcome::Retry { err, retry_after } => {
                last_err = Some(err);
                if n == cfg.retries {
                    break;
                }
                let delay = backoff_delay(n + 1, cfg.base, cfg.cap, retry_after, jitter_unit());
                tokio::time::sleep(delay).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        CoreError::Provider("model call failed after retries were exhausted".to_string())
    }))
}

/// Parse a `Retry-After` header value as whole seconds (HTTP-date form is not
/// supported; it falls back to computed backoff).
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_retryable_vs_fatal() {
        for s in [429u16, 408, 425, 500, 502, 503, 504] {
            assert_eq!(classify(Some(s), false), Retryable::Retry, "status {s}");
        }
        for s in [400u16, 401, 403, 404] {
            assert_eq!(classify(Some(s), false), Retryable::Fatal, "status {s}");
        }
        // Connection/timeout (no status) is always retryable.
        assert_eq!(classify(None, true), Retryable::Retry);
        // No status, not transient → fatal (can't tell, don't loop).
        assert_eq!(classify(None, false), Retryable::Fatal);
    }

    #[test]
    fn backoff_grows_caps_and_honors_retry_after() {
        let base = Duration::from_millis(500);
        let cap = Duration::from_millis(4000);
        // jitter_unit=1.0 → full delay (no shrink), so growth is observable.
        assert_eq!(
            backoff_delay(1, base, cap, None, 1.0),
            Duration::from_millis(500)
        );
        assert_eq!(
            backoff_delay(2, base, cap, None, 1.0),
            Duration::from_millis(1000)
        );
        assert_eq!(
            backoff_delay(3, base, cap, None, 1.0),
            Duration::from_millis(2000)
        );
        // Capped.
        assert_eq!(backoff_delay(6, base, cap, None, 1.0), cap);
        // Retry-After overrides the computed value (capped).
        assert_eq!(
            backoff_delay(1, base, cap, Some(Duration::from_secs(2)), 1.0),
            Duration::from_secs(2)
        );
        assert_eq!(
            backoff_delay(1, base, cap, Some(Duration::from_secs(99)), 1.0),
            cap
        );
        // Jitter scales down.
        assert_eq!(
            backoff_delay(2, base, cap, None, 0.5),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn config_from_env_defaults_and_overrides() {
        std::env::remove_var("FLEETY_MODEL_RETRIES");
        std::env::remove_var("FLEETY_MODEL_RETRY_BASE_MS");
        std::env::remove_var("FLEETY_MODEL_RETRY_CAP_MS");
        let d = RetryConfig::from_env();
        assert_eq!(d.retries, 3);
        assert_eq!(d.base, Duration::from_millis(500));
        std::env::set_var("FLEETY_MODEL_RETRIES", "0");
        std::env::set_var("FLEETY_MODEL_RETRY_BASE_MS", "100");
        let c = RetryConfig::from_env();
        assert_eq!(c.retries, 0); // disables retry
        assert_eq!(c.base, Duration::from_millis(100));
        std::env::remove_var("FLEETY_MODEL_RETRIES");
        std::env::remove_var("FLEETY_MODEL_RETRY_BASE_MS");
    }

    #[test]
    fn retry_after_parses_seconds_only() {
        assert_eq!(parse_retry_after(Some("2")), Some(Duration::from_secs(2)));
        assert_eq!(parse_retry_after(Some(" 5 ")), Some(Duration::from_secs(5)));
        // HTTP-date form is unsupported → None (caller uses computed backoff).
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(None), None);
    }

    #[tokio::test]
    async fn driver_retries_then_succeeds() {
        let cfg = RetryConfig {
            retries: 3,
            base: Duration::from_millis(0),
            cap: Duration::from_millis(0),
        };
        let mut calls = 0u32;
        let out: Result<&str> = run_with_retry(&cfg, || {
            calls += 1;
            let n = calls;
            async move {
                if n < 3 {
                    AttemptOutcome::Retry {
                        err: CoreError::Provider("transient".into()),
                        retry_after: None,
                    }
                } else {
                    AttemptOutcome::Done("ok")
                }
            }
        })
        .await;
        assert_eq!(out.unwrap(), "ok");
        assert_eq!(calls, 3);
    }

    #[tokio::test]
    async fn driver_fatal_fails_fast() {
        let cfg = RetryConfig::default();
        let mut calls = 0u32;
        let out: Result<&str> = run_with_retry(&cfg, || {
            calls += 1;
            async { AttemptOutcome::Fatal(CoreError::Provider("401".into())) }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls, 1); // no retry on fatal
    }

    #[tokio::test]
    async fn driver_exhausts_retries() {
        let cfg = RetryConfig {
            retries: 2,
            base: Duration::from_millis(0),
            cap: Duration::from_millis(0),
        };
        let mut calls = 0u32;
        let out: Result<&str> = run_with_retry(&cfg, || {
            calls += 1;
            async {
                AttemptOutcome::Retry {
                    err: CoreError::Provider("503".into()),
                    retry_after: None,
                }
            }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls, 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn retries_zero_is_single_attempt() {
        let cfg = RetryConfig {
            retries: 0,
            base: Duration::from_millis(0),
            cap: Duration::from_millis(0),
        };
        let mut calls = 0u32;
        let _: Result<&str> = run_with_retry(&cfg, || {
            calls += 1;
            async {
                AttemptOutcome::Retry {
                    err: CoreError::Provider("503".into()),
                    retry_after: None,
                }
            }
        })
        .await;
        assert_eq!(calls, 1);
    }
}
