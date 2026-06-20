//! Observability: process-wide tracing/logging init.

/// Initialize the global tracing subscriber from `RUST_LOG` (default `info`).
///
/// Idempotent and non-panicking: a second call, or an already-installed
/// subscriber, is silently ignored.
pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).try_init();
}
