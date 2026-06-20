//! fleety — the Fleety CLI / TUI entry point.
//!
//! M0: a skeleton that prints a banner and exits. No interactive mode, init,
//! or session yet.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use agent_core::obs;

fn main() {
    obs::init();
    tracing::info!(
        version = agent_core::VERSION,
        protocol = fleety_protocol::PROTOCOL_VERSION,
        "fleety CLI starting (M0 skeleton)"
    );
    println!(
        "fleety {} — M0 skeleton (no interactive mode yet)",
        agent_core::VERSION
    );
}
