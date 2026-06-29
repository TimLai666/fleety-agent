//! Stable device identity, shared by the CLI and the daemon so every process on
//! one machine resolves the **same** id.
//!
//! Precedence: `FLEETY_DEVICE_ID` (explicit override, for VM/container clones
//! that share a machine id) → the OS machine id (`machine-uid`: Windows
//! MachineGuid, Linux `/etc/machine-id`, macOS IOPlatformUUID) → the hostname as
//! a last resort (logged, since hostnames can collide; set `FLEETY_DEVICE_ID` to
//! avoid it). The hostname is also reported separately as a human-readable label.

use std::sync::OnceLock;

/// The machine's hostname (a display label, not the identity).
pub fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
}

/// Pure id resolution (testable without touching the OS): override → machine id
/// → hostname → a fixed fallback.
pub fn resolve_device_id(
    override_id: Option<&str>,
    machine: Option<&str>,
    host: Option<&str>,
) -> String {
    if let Some(id) = override_id.filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    if let Some(m) = machine.filter(|s| !s.is_empty()) {
        return m.to_string();
    }
    host.filter(|s| !s.is_empty())
        .unwrap_or("fleety-device")
        .to_string()
}

/// This device's stable id (cached for the process). Reads `FLEETY_DEVICE_ID`,
/// else the OS machine id, else the hostname (with a warning, since that can
/// collide).
pub fn device_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let machine = match machine_uid::get() {
            Ok(id) => Some(id),
            Err(e) => {
                if std::env::var("FLEETY_DEVICE_ID").ok().filter(|s| !s.is_empty()).is_none() {
                    tracing::warn!(error = %e, "could not read a stable machine id; falling back to the hostname (set FLEETY_DEVICE_ID to avoid collisions between same-named machines)");
                }
                None
            }
        };
        resolve_device_id(
            std::env::var("FLEETY_DEVICE_ID").ok().as_deref(),
            machine.as_deref(),
            hostname().as_deref(),
        )
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_precedence() {
        // Override beats everything.
        assert_eq!(
            resolve_device_id(Some("OVR"), Some("machine"), Some("host")),
            "OVR"
        );
        // Empty override is ignored → machine id.
        assert_eq!(
            resolve_device_id(Some(""), Some("machine"), Some("host")),
            "machine"
        );
        // No override → machine id over hostname.
        assert_eq!(
            resolve_device_id(None, Some("machine"), Some("host")),
            "machine"
        );
        // No machine id → hostname.
        assert_eq!(resolve_device_id(None, None, Some("host")), "host");
        // Nothing → fixed fallback (never an empty id).
        assert_eq!(resolve_device_id(None, None, None), "fleety-device");
        assert_eq!(
            resolve_device_id(Some(""), Some(""), Some("")),
            "fleety-device"
        );
    }
}
