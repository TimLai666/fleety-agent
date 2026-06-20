//! fleety-server — the Fleety Agent server.
//!
//! M0: a skeleton that starts, proves the never-crash boundary, and exits
//! cleanly. No conversation, device, or model service yet.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use agent_core::{obs, panic};

#[tokio::main]
async fn main() {
    obs::init();
    tracing::info!(
        version = agent_core::VERSION,
        protocol = fleety_protocol::PROTOCOL_VERSION,
        "fleety-server starting (M0 skeleton)"
    );

    // Demonstrate the never-crash boundary: an isolated task that panics must
    // not bring the process down.
    match panic::isolate_async(async { panic!("simulated task panic") }).await {
        Ok(()) => {}
        Err(err) => {
            tracing::warn!(report = ?err.report(), "isolated a panicking task; server is still alive")
        }
    }

    tracing::info!("fleety-server: no service implemented yet (M0); exiting cleanly");
}
