//! Background polling for new fleety-agent releases.
//!
//! `fleetyd update` already does the right thing on demand — fetches the
//! manifest, downloads + verifies, swaps the exe — but it's user-driven. This
//! module runs a long-period check (default 24h) so a device that's been
//! online for weeks doesn't drift arbitrarily far behind. Default behaviour is
//! *notify only*: log a clear warning when a newer version exists. Setting
//! `FLEETY_AUTO_UPDATE=apply` opts into the equivalent of a periodic
//! `fleetyd update` (still best-effort, isolated from the WS loop).

use std::time::Duration;

use agent_core::Result;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MIN_INTERVAL: Duration = Duration::from_secs(60); // floor; lets tests opt in

/// Background auto-apply is the default: a host that configured an update
/// manifest has already opted into updates, so notify-only would be intent
/// without follow-through. `FLEETY_AUTO_UPDATE=notify` (or `0`) opts back
/// down to log-only.
fn auto_apply_enabled() -> bool {
    !matches!(
        std::env::var("FLEETY_AUTO_UPDATE").as_deref(),
        Ok("notify") | Ok("0")
    )
}

/// Interval read from `FLEETY_UPDATE_POLL_SECS`, or default. Clamped to a
/// floor so a misconfigured low value doesn't busy-loop.
fn interval_from_env() -> Duration {
    std::env::var("FLEETY_UPDATE_POLL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(|secs| Duration::from_secs(secs).max(MIN_INTERVAL))
        .unwrap_or(DEFAULT_INTERVAL)
}

/// Spawn the periodic check. Each tick reads the manifest URL fresh from env
/// so the user can rotate it without restarting; a tick failure is logged and
/// the loop continues. Returns immediately; the task runs until fleetyd exits.
pub fn spawn() {
    if std::env::var("FLEETY_UPDATE_MANIFEST").is_err() {
        // Without a manifest URL we have nothing to compare against. Don't
        // spawn an idle ticker; users opt in by setting the env var.
        tracing::info!(
            "FLEETY_UPDATE_MANIFEST not set; skipping auto-update poll (set it to enable)"
        );
        return;
    }
    let interval = interval_from_env();
    let auto_apply = auto_apply_enabled();
    tracing::info!(
        interval_secs = interval.as_secs(),
        auto_apply,
        "auto-update poll enabled"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // First tick fires immediately; that's exactly what we want on boot.
        loop {
            ticker.tick().await;
            if let Err(e) = tick(auto_apply).await {
                tracing::warn!(report = ?e.report(), "auto-update tick failed (isolated)");
            }
        }
    });
}

/// One poll. Applies by default (the full host-wide update, which
/// short-circuits when everything is current); notify-only when the user
/// opted down with `FLEETY_AUTO_UPDATE=notify`.
async fn tick(auto_apply: bool) -> Result<()> {
    if auto_apply {
        // Reuse the on-demand path verbatim — it logs version transitions and
        // returns Ok(false) when already up to date.
        let updated = fleety_tools::update::self_update().await?;
        // Bring the host's sibling binaries along BEFORE scheduling our own
        // restart, so the restart never interrupts a sibling download.
        // Best-effort — a sibling failure never fails the tick.
        if let Err(e) = fleety_tools::update::update_siblings_to_latest(
            &fleety_tools::update::host_siblings_of("fleetyd"),
        )
        .await
        {
            tracing::warn!(report = ?e.report(), "sibling updates did not complete");
        }
        if updated {
            // Request a self-restart that the serve loop carries out at the
            // next idle frame boundary (an update never interrupts a running
            // tool).
            crate::request_self_restart("self-update");
        }
        return Ok(());
    }
    // Notify-only path: check the manifest, print a one-line summary, never
    // touch the binary.
    let latest = fleety_tools::update::probe_latest().await?;
    let current = agent_core::VERSION;
    if fleety_tools::update::needs_update_str(current, &latest) {
        tracing::warn!(
            current,
            latest = %latest,
            "fleetyd update available; run `fleetyd update` to apply (or unset FLEETY_AUTO_UPDATE)"
        );
    } else {
        tracing::info!(current, "fleetyd is up to date");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, not three, so the shared FLEETY_UPDATE_POLL_SECS and
    // FLEETY_AUTO_UPDATE env vars can't race between parallel test threads.
    #[test]
    fn env_parsing_default_floor_and_auto_apply() {
        // Default when unset.
        std::env::remove_var("FLEETY_UPDATE_POLL_SECS");
        assert_eq!(interval_from_env(), DEFAULT_INTERVAL);

        // Below-floor values clamp up to MIN_INTERVAL.
        std::env::set_var("FLEETY_UPDATE_POLL_SECS", "1");
        assert_eq!(interval_from_env(), MIN_INTERVAL);
        std::env::remove_var("FLEETY_UPDATE_POLL_SECS");

        // Auto-apply is the default; `notify` (or `0`) opts down to log-only.
        std::env::remove_var("FLEETY_AUTO_UPDATE");
        assert!(auto_apply_enabled(), "unset defaults to apply");
        std::env::set_var("FLEETY_AUTO_UPDATE", "apply");
        assert!(auto_apply_enabled());
        std::env::set_var("FLEETY_AUTO_UPDATE", "notify");
        assert!(!auto_apply_enabled());
        std::env::set_var("FLEETY_AUTO_UPDATE", "0");
        assert!(!auto_apply_enabled());
        std::env::remove_var("FLEETY_AUTO_UPDATE");
    }
}
