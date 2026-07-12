//! `ffmpeg` (+ `ffprobe`) as a framework dependency: the system codec toolkit the
//! `video_extract` engine (`crv`) shells out to. It is a SYSTEM binary, so unlike
//! the pip/managed deps it is installed via the platform package manager
//! (winget / brew / apt|dnf), mirroring the Chrome auto-install. Best-effort:
//! Linux `apt`/`dnf` need root and may fail silently, in which case the tool
//! returns an actionable "install ffmpeg" error rather than crashing. Per-dep
//! opt-out: `FLEETY_FFMPEG_AUTO_INSTALL=0`; `FLEETY_FFMPEG_BIN` overrides the path.

use std::process::Stdio;

use agent_core::CoreError;

use crate::deps::{Dependency, Strategy};

const FFMPEG_AUTO_INSTALL_ENV: &str = "FLEETY_FFMPEG_AUTO_INSTALL";

/// The `ffmpeg` command: env override (when it names a real file), else the bare
/// name for a `PATH` lookup.
fn ffmpeg_command() -> String {
    if let Ok(p) = std::env::var("FLEETY_FFMPEG_BIN") {
        if std::path::Path::new(&p).is_file() {
            return p;
        }
    }
    if cfg!(windows) {
        "ffmpeg.exe".to_string()
    } else {
        "ffmpeg".to_string()
    }
}

/// The per-OS package-manager install attempts (first success wins). Pure over
/// `os` so all three platforms are unit-testable from any host.
fn ffmpeg_install_attempts(os: &str) -> Vec<(&'static str, &'static [&'static str])> {
    match os {
        "windows" => vec![(
            "winget",
            &[
                "install",
                "-e",
                "--id",
                "Gyan.FFmpeg",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ][..],
        )],
        "macos" => vec![("brew", &["install", "ffmpeg"][..])],
        // Linux and others: apt (Debian/Ubuntu) then dnf (Fedora/RHEL).
        _ => vec![
            ("apt-get", &["install", "-y", "ffmpeg"][..]),
            ("dnf", &["install", "-y", "ffmpeg"][..]),
        ],
    }
}

/// Whether `ffmpeg -version` runs.
async fn ffmpeg_present() -> bool {
    tokio::process::Command::new(ffmpeg_command())
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort install of ffmpeg via the platform package manager.
async fn try_install_ffmpeg() -> bool {
    for (cmd, args) in ffmpeg_install_attempts(std::env::consts::OS) {
        tracing::info!(installer = cmd, "trying to auto-install ffmpeg");
        match tokio::process::Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(s) if s.success() => {
                tracing::info!(installer = cmd, "ffmpeg installed");
                return true;
            }
            Ok(_) | Err(_) => continue, // installer absent, needs root, or errored
        }
    }
    false
}

/// ffmpeg as a startup-dependency framework entry. Probe: `ffmpeg -version`;
/// install: platform package manager; per-dep opt-out via
/// `FLEETY_FFMPEG_AUTO_INSTALL`.
pub fn ffmpeg_dependency() -> Dependency {
    Dependency::new(
        "ffmpeg",
        Strategy::ManagedBinary,
        Some(FFMPEG_AUTO_INSTALL_ENV),
        || async { ffmpeg_present().await },
        || async {
            if try_install_ffmpeg().await && ffmpeg_present().await {
                Ok(())
            } else {
                Err(CoreError::Message(
                    "could not install ffmpeg via the system package manager (it may need root, \
                     or the manager is absent); install it manually (brew / apt / winget) or set \
                     FLEETY_FFMPEG_BIN"
                        .to_string(),
                ))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_attempts_cover_each_platform() {
        for os in ["windows", "macos", "linux"] {
            let attempts = ffmpeg_install_attempts(os);
            assert!(!attempts.is_empty(), "no ffmpeg installer for {os}");
            // Every attempt names ffmpeg somewhere in its command or args.
            assert!(
                attempts.iter().all(|(cmd, args)| args
                    .iter()
                    .any(|a| a.contains("ffmpeg") || a.contains("FFmpeg"))
                    || cmd.contains("ffmpeg")),
                "an installer for {os} does not reference ffmpeg"
            );
        }
        // Windows uses winget, macOS uses brew, Linux uses apt first.
        assert_eq!(ffmpeg_install_attempts("windows")[0].0, "winget");
        assert_eq!(ffmpeg_install_attempts("macos")[0].0, "brew");
        assert_eq!(ffmpeg_install_attempts("linux")[0].0, "apt-get");
    }
}
