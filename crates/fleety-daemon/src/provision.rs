//! Provision the fleety-insyra data-analysis sidecar onto this device.
//!
//! The sidecar ships as a raw per-target binary on the GitHub release; we
//! download it next to the fleetyd executable so the on-device `insyra_exec`
//! tool (which looks beside the current exe) finds it. Run from `fleetyd
//! install` (fetch if missing) and `fleetyd update` (refresh). Best-effort: a
//! failure here leaves `insyra_exec` returning an actionable error — it does not
//! stop fleetyd from running. The download is exercised manually (same posture
//! as the self-update download); the pure URL/target logic is unit-tested.

use std::path::PathBuf;

use agent_core::{CoreError, Result};

const REPO: &str = "TimLai666/fleety-agent";

/// The release asset target triple for this build, or None if unsupported.
fn target_triple() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("x86_64", "windows") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn sidecar_filename() -> &'static str {
    if cfg!(windows) {
        "fleety-insyra.exe"
    } else {
        "fleety-insyra"
    }
}

/// Download URL for this platform's sidecar (env override for tests/mirrors).
fn sidecar_url() -> Result<String> {
    if let Ok(url) = std::env::var("FLEETY_INSYRA_URL") {
        return Ok(url);
    }
    let target = target_triple().ok_or_else(|| {
        CoreError::Message(format!(
            "no prebuilt fleety-insyra for {}/{}; build it from sidecars/fleety-insyra",
            std::env::consts::ARCH,
            std::env::consts::OS
        ))
    })?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    Ok(format!(
        "https://github.com/{REPO}/releases/latest/download/fleety-insyra-{target}{suffix}"
    ))
}

fn dest_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| CoreError::Message("current exe has no parent directory".to_string()))?;
    Ok(dir.join(sidecar_filename()))
}

/// Ensure the sidecar sits next to fleetyd; download it when missing (or when
/// `force`, e.g. on `update`, to refresh it).
pub async fn ensure_insyra(force: bool) -> Result<()> {
    let dest = dest_path()?;
    if dest.is_file() && !force {
        tracing::info!(path = %dest.display(), "fleety-insyra already present");
        return Ok(());
    }
    let url = sidecar_url()?;
    tracing::info!(%url, "downloading fleety-insyra sidecar");
    let bytes = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("download fleety-insyra failed: {e}")))?
        .error_for_status()
        .map_err(|e| CoreError::Provider(format!("download fleety-insyra failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| CoreError::Provider(format!("read fleety-insyra failed: {e}")))?;

    let staged = dest.with_extension("new");
    std::fs::write(&staged, &bytes)
        .map_err(|e| CoreError::Message(format!("cannot write staged sidecar: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| CoreError::Message(format!("cannot mark sidecar executable: {e}")))?;
    }
    // The sidecar isn't running during install/update, so replacing is safe.
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&staged, &dest)
        .map_err(|e| CoreError::Message(format!("cannot install sidecar: {e}")))?;
    tracing::info!(path = %dest.display(), bytes = bytes.len(), "fleety-insyra installed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_and_filename_are_consistent() {
        // The mapping must never panic, and the filename ext matches the OS.
        let _ = target_triple();
        if cfg!(windows) {
            assert!(sidecar_filename().ends_with(".exe"));
        } else {
            assert_eq!(sidecar_filename(), "fleety-insyra");
        }
    }

    #[test]
    fn url_env_override_wins() {
        std::env::set_var("FLEETY_INSYRA_URL", "https://example.test/sidecar");
        assert_eq!(sidecar_url().unwrap(), "https://example.test/sidecar");
        std::env::remove_var("FLEETY_INSYRA_URL");
    }

    #[test]
    fn url_defaults_to_release_asset() {
        std::env::remove_var("FLEETY_INSYRA_URL");
        if let Some(target) = target_triple() {
            let url = sidecar_url().expect("url");
            assert!(url.contains(&format!("fleety-insyra-{target}")));
            assert!(url.starts_with("https://github.com/"));
        }
    }
}
