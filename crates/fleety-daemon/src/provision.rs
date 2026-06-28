//! Provision the fleety-insyra data-analysis sidecar onto this device.
//!
//! The actual logic now lives in the shared `fleety_tools::deps::insyra` module
//! (so fleetyd and fleety-server provision it the same way, folded into the
//! startup-dependency framework). This thin wrapper preserves fleetyd's existing
//! call sites (`fleetyd install` / `fleetyd update`).

use agent_core::Result;

/// Ensure the insyra sidecar sits next to fleetyd; download when missing (or when
/// `force`, e.g. on `update`, to refresh it). Best-effort.
pub async fn ensure_insyra(force: bool) -> Result<()> {
    fleety_tools::deps::insyra::ensure_insyra(force).await
}
