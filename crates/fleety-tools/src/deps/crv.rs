//! `crv` (claude-real-video) as a framework dependency: the Python CLI behind
//! the `video_extract` tool. Probe: `crv --version`; install: pipx / pip --user
//! (Python itself is a separate framework dependency). Whisper transcription is
//! opt-in via `FLEETY_VIDEO_WHISPER=on`, so the heavy transcription stack (torch)
//! is pulled in only when requested. Per-dep opt-out: `FLEETY_CRV_AUTO_INSTALL=0`.

use std::process::Stdio;

use agent_core::CoreError;

use crate::deps::{Dependency, Strategy};

const CRV_AUTO_INSTALL_ENV: &str = "FLEETY_CRV_AUTO_INSTALL";

/// The `crv` command: env override (when it names a real file), else the bare
/// name for a `PATH` lookup (it is a pip console script).
fn crv_command() -> String {
    if let Ok(p) = std::env::var("FLEETY_CRV_BIN") {
        if std::path::Path::new(&p).is_file() {
            return p;
        }
    }
    if cfg!(windows) {
        "crv.exe".to_string()
    } else {
        "crv".to_string()
    }
}

/// The pip package spec, with the Whisper extra only when opted in — pure so the
/// gate is unit-testable.
fn crv_package(whisper: bool) -> &'static str {
    if whisper {
        "claude-real-video[whisper]"
    } else {
        "claude-real-video"
    }
}

/// Whether Whisper transcription (and its heavy install) is opted in.
fn whisper_opt_in() -> bool {
    std::env::var("FLEETY_VIDEO_WHISPER")
        .map(|v| v.trim().eq_ignore_ascii_case("on"))
        .unwrap_or(false)
}

/// Whether `crv --version` runs (not trusting a stale PATH shim alone).
async fn crv_runs(command: &str) -> bool {
    tokio::process::Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort install of `claude-real-video` (with `[whisper]` when opted in).
/// Tries pipx first (isolated venv), then `pip --user`. First success wins.
async fn try_install_crv() -> bool {
    let pkg = crv_package(whisper_opt_in());
    let candidates: &[(&str, &[&str])] = &[
        ("pipx", &["install", "--force", pkg]),
        ("pip3", &["install", "--user", "-U", pkg]),
        ("pip", &["install", "--user", "-U", pkg]),
        ("python3", &["-m", "pip", "install", "--user", "-U", pkg]),
        ("python", &["-m", "pip", "install", "--user", "-U", pkg]),
    ];
    for (cmd, args) in candidates {
        tracing::info!(
            installer = cmd,
            package = pkg,
            "trying to auto-install claude-real-video"
        );
        match tokio::process::Command::new(cmd).args(*args).status().await {
            Ok(s) if s.success() => {
                tracing::info!(installer = cmd, "claude-real-video installed");
                return true;
            }
            Ok(_) | Err(_) => continue, // installer absent or errored
        }
    }
    false
}

/// crv as a startup-dependency framework entry. Probe: `crv --version`; install:
/// pipx / pip --user; per-dep opt-out via `FLEETY_CRV_AUTO_INSTALL`.
pub fn crv_dependency() -> Dependency {
    Dependency::new(
        "crv",
        Strategy::UserPackage,
        Some(CRV_AUTO_INSTALL_ENV),
        || async { crv_runs(&crv_command()).await },
        || async {
            if try_install_crv().await && crv_runs(&crv_command()).await {
                Ok(())
            } else {
                Err(CoreError::Message(
                    "no pipx / pip / python on PATH, or the install errored; run \
                     `pipx install claude-real-video` (append `[whisper]` for transcription) \
                     manually"
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
    fn crv_package_gates_whisper_extra() {
        assert_eq!(crv_package(false), "claude-real-video");
        assert_eq!(crv_package(true), "claude-real-video[whisper]");
    }
}
