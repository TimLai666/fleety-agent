//! Self-update for fleetyd.
//!
//! `fleetyd update` fetches a manifest (`FLEETY_UPDATE_MANIFEST`), compares the
//! version, downloads the artifact, verifies its SHA-256, and swaps it into
//! place (move the running binary aside, install the new one — works on Windows
//! where a running exe can be renamed but not overwritten). The manifest/version/
//! hash logic is pure and unit-tested; the download+swap is exercised manually
//! (same posture as the model provider's live calls).

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

/// Public probe for the background poller: fetch and parse the manifest,
/// return just the `version` string.
pub async fn probe_latest() -> Result<String> {
    let url = std::env::var("FLEETY_UPDATE_MANIFEST").map_err(|_| {
        CoreError::Message("set FLEETY_UPDATE_MANIFEST to the update manifest URL".to_string())
    })?;
    let text = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("fetch manifest failed: {e}")))?
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("read manifest failed: {e}")))?;
    Ok(parse_manifest(&text)?.version)
}

/// Re-export of the version-diff predicate so the background poller can use the
/// same logic as `update()`.
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

/// Fetch the manifest, and if a newer version is available, download + verify +
/// install it. Returns `true` if a new binary was installed (caller restarts the
/// service — deferred until idle — to run it), `false` if already up to date.
pub async fn update() -> Result<bool> {
    let manifest_url = std::env::var("FLEETY_UPDATE_MANIFEST").map_err(|_| {
        CoreError::Message("set FLEETY_UPDATE_MANIFEST to the update manifest URL".to_string())
    })?;
    let client = reqwest::Client::new();
    let text = client
        .get(&manifest_url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("fetch manifest failed: {e}")))?
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("read manifest failed: {e}")))?;
    let manifest = parse_manifest(&text)?;

    let current = agent_core::VERSION;
    if !needs_update(current, &manifest.version) {
        println!("fleetyd is already up to date (version {current}).");
        return Ok(false);
    }
    println!("Updating fleetyd {current} -> {} ...", manifest.version);

    let bytes = client
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

    let current_exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    let staged = current_exe.with_extension("new");
    std::fs::write(&staged, &bytes)
        .map_err(|e| CoreError::Message(format!("cannot write staged binary: {e}")))?;
    let backup = current_exe.with_extension("old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(&current_exe, &backup)
        .map_err(|e| CoreError::Message(format!("cannot move current exe aside: {e}")))?;
    if let Err(e) = std::fs::rename(&staged, &current_exe) {
        // Roll back so the daemon stays runnable.
        let _ = std::fs::rename(&backup, &current_exe);
        return Err(CoreError::Message(format!("cannot install new exe: {e}")));
    }
    println!(
        "Updated to {}. The service will restart (when idle) to apply.",
        manifest.version
    );
    Ok(true)
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
}
