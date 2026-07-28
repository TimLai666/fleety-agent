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

/// This user's home directory, as every Fleety path is anchored to it.
///
/// `HOME=""` is a real state — Git Bash, some CI runners, `su` without `-` —
/// and `env::var("HOME")` reports it as `Ok("")`, so the old fallback chain
/// never fired and the empty string produced a *relative* path. Config, saved
/// tokens, and provider API keys then landed in whatever directory the command
/// happened to run in, silently, and the user's real profiles looked deleted.
///
/// `std::env::home_dir` treats an empty value as unset and then asks the OS
/// (the passwd entry on Unix, the known-folder API on Windows), so an empty or
/// missing variable recovers the real home instead of aiming at the cwd.
pub fn home_dir() -> std::path::PathBuf {
    located_home().unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// The home directory only when one could actually be located.
fn located_home() -> Option<std::path::PathBuf> {
    std::env::home_dir().filter(|home| !home.as_os_str().is_empty())
}

/// Whether a home directory could actually be located. A caller that is about
/// to persist a credential should refuse rather than write relative to the
/// current directory.
pub fn home_is_known() -> bool {
    located_home().is_some()
}

/// Refuse a write whose path is relative *because* no home directory was
/// located.
///
/// Every Fleety path falls back to `~/…`, so an unlocatable home turns the
/// destination into a cwd-relative one. A silent write there scatters config,
/// bearer tokens, and provider API keys through whatever directory the command
/// ran in, and makes the user's real files look deleted. Reads stay soft (they
/// simply find nothing); only writes refuse. An absolute path is always
/// allowed — it came from an explicit `FLEETY_*` override or a caller that
/// already knows where it is writing.
pub fn ensure_writable_path(path: &std::path::Path, what: &str) -> agent_core::Result<()> {
    match unwritable_path_reason(path, what, home_is_known()) {
        Some(why) => Err(agent_core::CoreError::Message(why)),
        None => Ok(()),
    }
}

/// The decision behind [`ensure_writable_path`], split out so both branches are
/// testable without mutating the process environment.
fn unwritable_path_reason(path: &std::path::Path, what: &str, home_known: bool) -> Option<String> {
    if path.is_absolute() || home_known {
        return None;
    }
    Some(format!(
        "refusing to write {what} to '{}': no home directory could be located \
         (HOME/USERPROFILE is unset or empty), so this path is relative to the \
         current directory. Set HOME, or point Fleety at an absolute path with \
         the matching FLEETY_* variable (see `fleety env`).",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_home_variable_is_treated_as_unset() {
        // `HOME=""` is what Git Bash, `su` without `-`, and some CI runners
        // leave behind. Reading it literally yields a *relative* base path, so
        // config and credentials land in the cwd. The resolved home must never
        // be relative.
        assert!(
            home_dir().is_absolute() || !home_is_known(),
            "a located home must be absolute, got {:?}",
            home_dir()
        );
    }

    #[test]
    fn writes_are_refused_only_when_a_relative_path_came_from_an_unlocatable_home() {
        let relative = std::path::Path::new(".fleety/connections.toml");
        let absolute = std::env::temp_dir().join("connections.toml");

        // The failure this guards: no home, so the path aims at the cwd.
        let why = unwritable_path_reason(relative, "connections.toml", false)
            .expect("a relative path with no home must be refused");
        assert!(why.contains("connections.toml"), "{why}");
        assert!(why.contains("HOME"), "{why}");

        // A located home makes the same relative path fine (it was resolved
        // against that home), and an absolute path is always the caller's own.
        assert!(unwritable_path_reason(relative, "connections.toml", true).is_none());
        assert!(unwritable_path_reason(&absolute, "connections.toml", false).is_none());
    }

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
