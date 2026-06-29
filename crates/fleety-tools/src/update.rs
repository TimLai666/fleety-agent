//! Binary self-update + multi-binary update, shared by `fleety`,
//! `fleety-server`, and `fleetyd`.
//!
//! A binary's update is driven by a JSON manifest (`{version,url,sha256}`). The
//! manifest URL comes from `FLEETY_UPDATE_MANIFEST`; when it contains the literal
//! `{bin}`, that placeholder is replaced with the binary name so one base URL can
//! serve every binary (e.g. `https://host/dl/{bin}/latest.json`). A plain URL
//! (no `{bin}`) is treated as the *current* binary's manifest — back-compat for
//! the original `fleetyd update`.
//!
//! Binaries ship in lockstep (one workspace version), so the running process's
//! `agent_core::VERSION` is the baseline for every local binary. The manifest /
//! version / hash logic is pure and unit-tested; the download + swap is exercised
//! manually (same posture as the model provider's live calls).

use std::path::Path;

use agent_core::{CoreError, Result};
use serde_json::Value;

struct Manifest {
    version: String,
    url: String,
    sha256: String,
}

fn parse_manifest(text: &str) -> Result<Manifest> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| CoreError::Message(format!("invalid update manifest: {e}")))?;
    let field = |key: &str| -> Result<String> {
        v.get(key)
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| CoreError::Message(format!("manifest missing string '{key}'")))
    };
    Ok(Manifest {
        version: field("version")?,
        url: field("url")?,
        sha256: field("sha256")?.to_lowercase(),
    })
}

/// Whether `latest` differs from `current` (ignoring a leading `v`).
fn needs_update(current: &str, latest: &str) -> bool {
    current.trim_start_matches('v') != latest.trim_start_matches('v')
}

/// Re-export of the version-diff predicate for the background poller.
pub fn needs_update_str(current: &str, latest: &str) -> bool {
    needs_update(current, latest)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The file stem of the running binary (e.g. `fleetyd`), used as its `bin` name.
fn current_bin_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "fleety".to_string())
}

/// Whether `FLEETY_UPDATE_MANIFEST` is a per-binary template (`{bin}` form) —
/// required to safely resolve a *different* binary's manifest.
pub fn manifest_is_templated() -> bool {
    std::env::var("FLEETY_UPDATE_MANIFEST")
        .map(|b| b.contains("{bin}"))
        .unwrap_or(false)
}

/// The manifest URL for `bin`, from `FLEETY_UPDATE_MANIFEST` (with `{bin}`
/// substituted when the base is a template).
pub fn manifest_url_for(bin: &str) -> Result<String> {
    let base = std::env::var("FLEETY_UPDATE_MANIFEST").map_err(|_| {
        CoreError::Message(
            "set FLEETY_UPDATE_MANIFEST to the update manifest URL (may contain {bin})".to_string(),
        )
    })?;
    Ok(if base.contains("{bin}") {
        base.replace("{bin}", bin)
    } else {
        base
    })
}

async fn fetch_text(url: &str) -> Result<String> {
    reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("fetch manifest failed: {e}")))?
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("read manifest failed: {e}")))
}

/// Probe the latest published version for `bin` (background poller).
pub async fn probe_latest_for(bin: &str) -> Result<String> {
    let url = manifest_url_for(bin)?;
    Ok(parse_manifest(&fetch_text(&url).await?)?.version)
}

/// Probe the latest version for the *running* binary.
pub async fn probe_latest() -> Result<String> {
    probe_latest_for(&current_bin_name()).await
}

/// Move `exe_path` aside and install `bytes` in its place. Works on Windows,
/// where a running exe can be renamed but not overwritten; rolls back on failure
/// so the binary stays runnable.
fn swap_exe(exe_path: &Path, bytes: &[u8]) -> Result<()> {
    let staged = exe_path.with_extension("new");
    std::fs::write(&staged, bytes)
        .map_err(|e| CoreError::Message(format!("cannot write staged binary: {e}")))?;
    let backup = exe_path.with_extension("old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(exe_path, &backup)
        .map_err(|e| CoreError::Message(format!("cannot move current exe aside: {e}")))?;
    if let Err(e) = std::fs::rename(&staged, exe_path) {
        let _ = std::fs::rename(&backup, exe_path); // roll back
        return Err(CoreError::Message(format!("cannot install new exe: {e}")));
    }
    Ok(())
}

/// Fetch `manifest_url`; if its version differs from `current_version`, download
/// the artifact, verify its SHA-256, and install it at `exe_path`. Returns `true`
/// when a new binary was installed (the caller restarts the service to apply).
pub async fn install(
    manifest_url: &str,
    exe_path: &Path,
    label: &str,
    current_version: &str,
) -> Result<bool> {
    let manifest = parse_manifest(&fetch_text(manifest_url).await?)?;
    if !needs_update(current_version, &manifest.version) {
        println!("{label} is already up to date (version {current_version}).");
        return Ok(false);
    }
    println!(
        "Updating {label} {current_version} -> {} ...",
        manifest.version
    );
    let bytes = reqwest::Client::new()
        .get(&manifest.url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("download failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| CoreError::Provider(format!("read artifact failed: {e}")))?;
    let actual = sha256_hex(&bytes);
    if actual != manifest.sha256 {
        return Err(CoreError::Message(format!(
            "sha256 mismatch: manifest {}, downloaded {actual}",
            manifest.sha256
        )));
    }
    swap_exe(exe_path, &bytes)?;
    println!("Updated {label} to {}.", manifest.version);
    Ok(true)
}

/// Self-update the running binary. Returns `true` if a new binary was installed.
pub async fn self_update() -> Result<bool> {
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    let bin = current_bin_name();
    let url = manifest_url_for(&bin)?;
    install(&url, &exe, &bin, agent_core::VERSION).await
}

/// Update the named binary at `exe_path` using `bin`'s manifest. The running
/// process's version is the baseline (binaries ship in lockstep).
pub async fn update_named(bin: &str, exe_path: &Path) -> Result<bool> {
    let url = manifest_url_for(bin)?;
    install(&url, exe_path, bin, agent_core::VERSION).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest_reads_fields() {
        let m = parse_manifest(r#"{"version":"0.2.0","url":"https://x/y","sha256":"AABB"}"#)
            .expect("parse");
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.url, "https://x/y");
        assert_eq!(m.sha256, "aabb"); // lowercased
        assert!(parse_manifest(r#"{"version":"1"}"#).is_err()); // missing fields
    }

    #[test]
    fn needs_update_ignores_v_prefix() {
        assert!(needs_update("0.1.0", "0.2.0"));
        assert!(!needs_update("0.1.0", "v0.1.0"));
        assert!(!needs_update("v0.1.0", "0.1.0"));
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    #[serial_test::serial]
    fn manifest_url_substitutes_bin_when_templated() {
        std::env::set_var("FLEETY_UPDATE_MANIFEST", "https://h/dl/{bin}/latest.json");
        assert!(manifest_is_templated());
        assert_eq!(
            manifest_url_for("fleety-server").unwrap(),
            "https://h/dl/fleety-server/latest.json"
        );
        // A plain URL is used as-is (current-binary back-compat).
        std::env::set_var("FLEETY_UPDATE_MANIFEST", "https://h/fleetyd.json");
        assert!(!manifest_is_templated());
        assert_eq!(
            manifest_url_for("fleetyd").unwrap(),
            "https://h/fleetyd.json"
        );
        std::env::remove_var("FLEETY_UPDATE_MANIFEST");
    }
}
