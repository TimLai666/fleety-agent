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

use crate::update;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MIN_INTERVAL: Duration = Duration::from_secs(60); // floor; lets tests opt in

/// Whether the user explicitly turned on background auto-apply.
fn auto_apply_enabled() -> bool {
    std::env::var("FLEETY_AUTO_UPDATE").as_deref() == Ok("apply")
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

/// One poll. Notify-only by default; if `auto_apply`, run the full `update()`
/// flow (which itself short-circuits when no newer version exists).
async fn tick(auto_apply: bool) -> Result<()> {
    if auto_apply {
        // Reuse the on-demand path verbatim — it logs version transitions and
        // returns Ok when already up to date.
        update::update().await?;
        return Ok(());
    }
    // Notify-only path: check the manifest, print a one-line summary, never
    // touch the binary.
    let latest = update::probe_latest().await?;
    let current = agent_core::VERSION;
    if update::needs_update_str(current, &latest) {
        tracing::warn!(
            current,
            latest = %latest,
            "fleetyd update available; run `fleetyd update` to apply (or set FLEETY_AUTO_UPDATE=apply)"
        );
    } else {
        tracing::info!(current, "fleetyd is up to date");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_falls_back_to_default_when_unset() {
        std::env::remove_var("FLEETY_UPDATE_POLL_SECS");
        assert_eq!(interval_from_env(), DEFAULT_INTERVAL);
    }

    #[test]
    fn interval_clamps_to_floor() {
        std::env::set_var("FLEETY_UPDATE_POLL_SECS", "1");
        assert_eq!(interval_from_env(), MIN_INTERVAL);
        std::env::remove_var("FLEETY_UPDATE_POLL_SECS");
    }

    #[test]
    fn auto_apply_flag_parses() {
        std::env::set_var("FLEETY_AUTO_UPDATE", "apply");
        assert!(auto_apply_enabled());
        std::env::set_var("FLEETY_AUTO_UPDATE", "notify");
        assert!(!auto_apply_enabled());
        std::env::remove_var("FLEETY_AUTO_UPDATE");
        assert!(!auto_apply_enabled());
    }
}
