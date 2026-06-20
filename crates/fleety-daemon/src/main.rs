//! fleetyd — the Fleety device background service.
//!
//! M0: a skeleton that starts and exits cleanly. No Agent connection,
//! heartbeat, or local tools yet.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use agent_core::obs;

#[tokio::main]
async fn main() {
    obs::init();
    tracing::info!(
        version = agent_core::VERSION,
        protocol = fleety_protocol::PROTOCOL_VERSION,
        "fleetyd starting (M0 skeleton)"
    );
    tracing::info!("fleetyd: no Agent connection implemented yet (M0); exiting cleanly");
}
