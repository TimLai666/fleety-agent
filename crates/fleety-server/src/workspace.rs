//! Per-conversation workspace binding: where a conversation's file/command/git
//! tools are rooted, and on which device they run.
//!
//! The CLI already sends `OriginContext { hostname, os, cwd }`. A conversation
//! binds once (from its first message) to a [`WorkspaceBinding`]: when the
//! originating device is the same host as the server, tools are rooted at the
//! CLI's `cwd` locally (the "coding agent in my project dir" case); otherwise it
//! falls back to the server workspace and records the originating device (remote
//! tool routing is a follow-up). Resolution is pure and unit-tested.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Whether `s` looks like an absolute path on *some* OS — the cwd may come from
/// a client on a different platform than the server, so we can't use
/// `Path::is_absolute` (which only knows the server's rules). Accepts a leading
/// `/` or `\` (Unix / UNC) or a `X:\` / `X:/` Windows drive prefix.
fn looks_absolute(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s.starts_with('\\')
        || (b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBinding {
    /// The working root for this conversation's tools.
    pub root: PathBuf,
    /// The device the work runs on; `None` = the server host itself.
    pub device: Option<String>,
    /// The originating CLI's cwd (raw, as sent), retained so the origin
    /// preamble can be re-presented each turn and across resume. `None` when
    /// the origin sent no usable cwd. `#[serde(default)]` keeps bindings
    /// persisted before these fields existed loadable (they load as `None`).
    #[serde(default)]
    pub origin_cwd: Option<String>,
    /// The originating device's hostname, when known.
    #[serde(default)]
    pub origin_hostname: Option<String>,
    /// The originating device's OS, when known.
    #[serde(default)]
    pub origin_os: Option<String>,
}

/// Resolve a conversation's workspace binding (pure). `cwd`/`origin_hostname`/
/// `origin_os` come from the originating CLI's `OriginContext`; `conn_device` is
/// the device that opened the connection; `server_hostname` identifies the
/// server's own host; `fallback_root` is the server's default workspace. The
/// origin fields are retained on the binding so each turn can re-inject the
/// origin context (see the "Runtime injects origin context into each turn"
/// requirement).
///
/// - absolute `cwd` on the **same host** as the server → root = `cwd`, local.
/// - absolute `cwd` on a **different** device → fall back to the server root,
///   recording the originating device and origin; tools stay server-rooted and
///   the agent uses `device_exec` (the runtime does not auto-route them).
/// - no usable `cwd` (absent, blank, relative) → the fallback root, no origin.
pub fn resolve_binding(
    cwd: Option<&str>,
    origin_hostname: Option<&str>,
    origin_os: Option<&str>,
    conn_device: &str,
    server_hostname: &str,
    fallback_root: &Path,
) -> WorkspaceBinding {
    let usable_cwd = cwd
        .map(str::trim)
        .filter(|c| !c.is_empty() && looks_absolute(c));
    let host = origin_hostname.map(str::to_string);
    let os = origin_os.map(str::to_string);
    match usable_cwd {
        Some(c) if origin_hostname == Some(server_hostname) => WorkspaceBinding {
            root: PathBuf::from(c),
            device: None,
            origin_cwd: Some(c.to_string()),
            origin_hostname: host,
            origin_os: os,
        },
        Some(c) => WorkspaceBinding {
            // cwd is on another device — keep server-local tools at the fallback
            // and record the device + origin, so each turn injects the origin and
            // the agent can device_exec to it (the runtime never auto-routes).
            root: fallback_root.to_path_buf(),
            device: Some(conn_device.to_string()),
            origin_cwd: Some(c.to_string()),
            origin_hostname: host,
            origin_os: os,
        },
        None => WorkspaceBinding {
            root: fallback_root.to_path_buf(),
            device: None,
            origin_cwd: None,
            origin_hostname: None,
            origin_os: None,
        },
    }
}

/// Build the ephemeral per-turn origin preamble from a resolved binding, or
/// `None` when the binding carries no usable origin (nothing to inject). The
/// text is re-injected each turn (never stored in the conversation) and differs
/// by whether the origin is the server host or another device: same-host says
/// tools are already rooted at the cwd; cross-device tells the agent bare tools
/// run on the server and to use `device_exec` to act on the origin device.
pub fn origin_preamble(binding: &WorkspaceBinding) -> Option<String> {
    let cwd = binding.origin_cwd.as_deref()?;
    let host = binding.origin_hostname.as_deref().unwrap_or("unknown");
    let os = binding.origin_os.as_deref().unwrap_or("unknown");
    match binding.device.as_deref() {
        None => Some(format!(
            "This conversation was started on this server host ({host}, {os}) in `{cwd}`. \
             Your file, command, and git tools are already rooted there — act in `{cwd}` \
             unless told otherwise."
        )),
        Some(device) => Some(format!(
            "This conversation was started on device `{device}` ({host}, {os}) in `{cwd}`. \
             Bare file/command/git tools run on the server, not there. To read or change \
             files on the origin device — including that directory's own AGENTS.md / \
             CLAUDE.md — wrap the tool in `device_exec(device=\"{device}\")`."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb() -> PathBuf {
        PathBuf::from("/srv/workspace")
    }

    #[test]
    fn same_host_absolute_cwd_roots_locally() {
        let b = resolve_binding(
            Some("/home/alice/proj"),
            Some("alice-box"),
            None,
            "dev1",
            "alice-box",
            &fb(),
        );
        assert_eq!(b.root, PathBuf::from("/home/alice/proj"));
        assert_eq!(b.device, None);
    }

    #[test]
    fn other_device_falls_back_and_records_device() {
        let b = resolve_binding(
            Some("/home/bob/proj"),
            Some("bob-laptop"),
            None,
            "dev2",
            "alice-box",
            &fb(),
        );
        assert_eq!(b.root, fb());
        assert_eq!(b.device, Some("dev2".to_string()));
    }

    #[test]
    fn windows_drive_cwd_is_absolute() {
        let b = resolve_binding(
            Some("C:\\Users\\alice\\proj"),
            Some("win-box"),
            None,
            "dev1",
            "win-box",
            &fb(),
        );
        assert_eq!(b.root, PathBuf::from("C:\\Users\\alice\\proj"));
        assert_eq!(b.device, None);
    }

    #[test]
    fn no_or_relative_cwd_uses_fallback_locally() {
        assert_eq!(
            resolve_binding(None, Some("alice-box"), None, "dev1", "alice-box", &fb()),
            WorkspaceBinding {
                root: fb(),
                device: None,
                origin_cwd: None,
                origin_hostname: None,
                origin_os: None,
            }
        );
        assert_eq!(
            resolve_binding(
                Some("relative/dir"),
                Some("alice-box"),
                None,
                "dev1",
                "alice-box",
                &fb()
            ),
            WorkspaceBinding {
                root: fb(),
                device: None,
                origin_cwd: None,
                origin_hostname: None,
                origin_os: None,
            }
        );
        assert_eq!(
            resolve_binding(Some("   "), Some("alice-box"), None, "dev1", "alice-box", &fb()),
            WorkspaceBinding {
                root: fb(),
                device: None,
                origin_cwd: None,
                origin_hostname: None,
                origin_os: None,
            }
        );
    }

    #[test]
    fn binding_roundtrip_preserves_origin() {
        let b = WorkspaceBinding {
            root: PathBuf::from("/home/alice/proj"),
            device: Some("dev2".to_string()),
            origin_cwd: Some("/home/alice/proj".to_string()),
            origin_hostname: Some("alice-box".to_string()),
            origin_os: Some("linux".to_string()),
        };
        let json = serde_json::to_string(&b).expect("serialize");
        let back: WorkspaceBinding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, b);
    }

    #[test]
    fn old_binding_json_loads_origin_as_none() {
        // A binding persisted before the origin fields existed still loads.
        let json = r#"{"root":"/srv/workspace","device":null}"#;
        let b: WorkspaceBinding =
            serde_json::from_str(json).expect("deserialize old binding");
        assert_eq!(b.root, PathBuf::from("/srv/workspace"));
        assert_eq!(b.device, None);
        assert_eq!(b.origin_cwd, None);
        assert_eq!(b.origin_hostname, None);
        assert_eq!(b.origin_os, None);
    }

    #[test]
    fn resolve_binding_records_origin_fields() {
        // Same host: origin fields are captured on the binding.
        let b = resolve_binding(
            Some("/home/alice/proj"),
            Some("alice-box"),
            Some("linux"),
            "dev1",
            "alice-box",
            &fb(),
        );
        assert_eq!(b.origin_cwd.as_deref(), Some("/home/alice/proj"));
        assert_eq!(b.origin_hostname.as_deref(), Some("alice-box"));
        assert_eq!(b.origin_os.as_deref(), Some("linux"));
        // Cross device: origin fields are captured too, and the device recorded.
        let c = resolve_binding(
            Some("/home/bob/proj"),
            Some("bob-laptop"),
            Some("macos"),
            "dev2",
            "alice-box",
            &fb(),
        );
        assert_eq!(c.origin_cwd.as_deref(), Some("/home/bob/proj"));
        assert_eq!(c.origin_hostname.as_deref(), Some("bob-laptop"));
        assert_eq!(c.origin_os.as_deref(), Some("macos"));
        assert_eq!(c.device.as_deref(), Some("dev2"));
    }

    #[test]
    fn origin_preamble_same_host() {
        let b = WorkspaceBinding {
            root: PathBuf::from("/home/alice/proj"),
            device: None,
            origin_cwd: Some("/home/alice/proj".to_string()),
            origin_hostname: Some("alice-box".to_string()),
            origin_os: Some("linux".to_string()),
        };
        let p = origin_preamble(&b).expect("same-host preamble");
        assert!(p.contains("/home/alice/proj"));
        // Same host: tools already rooted at the cwd, no device_exec guidance.
        assert!(!p.contains("device_exec"));
    }

    #[test]
    fn origin_preamble_cross_device() {
        let b = WorkspaceBinding {
            root: PathBuf::from("/srv/workspace"),
            device: Some("dev2".to_string()),
            origin_cwd: Some("/home/bob/proj".to_string()),
            origin_hostname: Some("bob-laptop".to_string()),
            origin_os: Some("macos".to_string()),
        };
        let p = origin_preamble(&b).expect("cross-device preamble");
        assert!(p.contains("dev2"));
        assert!(p.contains("/home/bob/proj"));
        // Cross device: must instruct device_exec to act on the origin device.
        assert!(p.contains("device_exec"));
    }

    #[test]
    fn origin_preamble_absent() {
        let b = WorkspaceBinding {
            root: PathBuf::from("/srv/workspace"),
            device: None,
            origin_cwd: None,
            origin_hostname: None,
            origin_os: None,
        };
        assert!(origin_preamble(&b).is_none());
    }

    #[test]
    fn cross_device_stays_server_rooted() {
        // Cross-device origin: bare tools stay rooted at the server fallback and
        // the origin device is recorded. The runtime does NOT relocate or
        // auto-route tool execution — the agent uses device_exec (driven by the
        // injected origin preamble) to reach the origin device.
        let fallback = fb();
        let b = resolve_binding(
            Some("/home/bob/proj"),
            Some("bob-laptop"),
            Some("macos"),
            "dev2",
            "alice-box",
            &fallback,
        );
        assert_eq!(
            b.root, fallback,
            "bare tools stay rooted at the server fallback, not the origin cwd"
        );
        assert_eq!(
            b.device.as_deref(),
            Some("dev2"),
            "the origin device is recorded for device_exec, not auto-routed"
        );
    }
}
