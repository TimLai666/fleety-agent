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
}

/// Resolve a conversation's workspace binding (pure). `cwd`/`origin_hostname`
/// come from the originating CLI's `OriginContext`; `conn_device` is the device
/// that opened the connection; `server_hostname` identifies the server's own
/// host; `fallback_root` is the server's default workspace.
///
/// - absolute `cwd` on the **same host** as the server → root = `cwd`, local.
/// - absolute `cwd` on a **different** device → fall back to the server root,
///   recording the originating device (its tools run there via device routing).
/// - no usable `cwd` (absent, blank, relative) → the fallback root, local.
pub fn resolve_binding(
    cwd: Option<&str>,
    origin_hostname: Option<&str>,
    conn_device: &str,
    server_hostname: &str,
    fallback_root: &Path,
) -> WorkspaceBinding {
    let usable_cwd = cwd
        .map(str::trim)
        .filter(|c| !c.is_empty() && looks_absolute(c));
    match usable_cwd {
        Some(c) if origin_hostname == Some(server_hostname) => WorkspaceBinding {
            root: PathBuf::from(c),
            device: None,
        },
        Some(_) => WorkspaceBinding {
            // cwd is on another device — keep server-local tools at the fallback;
            // record the device so a future change can route tools to it.
            root: fallback_root.to_path_buf(),
            device: Some(conn_device.to_string()),
        },
        None => WorkspaceBinding {
            root: fallback_root.to_path_buf(),
            device: None,
        },
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
            resolve_binding(None, Some("alice-box"), "dev1", "alice-box", &fb()),
            WorkspaceBinding {
                root: fb(),
                device: None
            }
        );
        assert_eq!(
            resolve_binding(
                Some("relative/dir"),
                Some("alice-box"),
                "dev1",
                "alice-box",
                &fb()
            ),
            WorkspaceBinding {
                root: fb(),
                device: None
            }
        );
        assert_eq!(
            resolve_binding(Some("   "), Some("alice-box"), "dev1", "alice-box", &fb()),
            WorkspaceBinding {
                root: fb(),
                device: None
            }
        );
    }
}
