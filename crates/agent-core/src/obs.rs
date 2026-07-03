//! Observability: process-wide tracing/logging init.

/// Initialize the global tracing subscriber from `RUST_LOG` (default `info`).
///
/// Idempotent and non-panicking: a second call, or an already-installed
/// subscriber, is silently ignored.
pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Logs go to stderr, never stdout. The ACP adapter (`fleety acp`) speaks a
    // JSON-RPC protocol over stdout — a log line there would corrupt the stream.
    // stderr is the correct sink for every binary here anyway.
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn init_is_idempotent_and_non_panicking() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let old = std::env::var("RUST_LOG").ok();
        init();
        init();
        std::env::set_var("RUST_LOG", "not a valid env filter [");
        init();
        match old {
            Some(value) => std::env::set_var("RUST_LOG", value),
            None => std::env::remove_var("RUST_LOG"),
        }
    }
}
