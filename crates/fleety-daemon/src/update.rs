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
/// install it.
pub async fn update() -> Result<()> {
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
        return Ok(());
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
    println!("Updated to {}. Restart fleetyd to apply.", manifest.version);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct EnvGuard(Option<String>);

    impl EnvGuard {
        fn set_manifest(url: &str) -> Self {
            let old = std::env::var("FLEETY_UPDATE_MANIFEST").ok();
            std::env::set_var("FLEETY_UPDATE_MANIFEST", url);
            Self(old)
        }

        fn unset_manifest() -> Self {
            let old = std::env::var("FLEETY_UPDATE_MANIFEST").ok();
            std::env::remove_var("FLEETY_UPDATE_MANIFEST");
            Self(old)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("FLEETY_UPDATE_MANIFEST", value),
                None => std::env::remove_var("FLEETY_UPDATE_MANIFEST"),
            }
        }
    }

    fn serve_update(version: &str, sha256: &str, artifact: Option<Vec<u8>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let base = format!("http://{addr}");
        let manifest =
            format!(r#"{{"version":"{version}","url":"{base}/artifact","sha256":"{sha256}"}}"#);
        let mut responses = vec![manifest.into_bytes()];
        if let Some(artifact) = artifact {
            responses.push(artifact);
        }
        thread::spawn(move || {
            for body in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .and_then(|_| stream.write_all(&body))
                    .expect("write response");
            }
        });
        format!("{base}/manifest")
    }

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
    fn parse_manifest_rejects_invalid_json_and_non_string_fields() {
        assert!(parse_manifest("not json").is_err());
        assert!(parse_manifest(r#"{"version":1,"url":"https://x/y","sha256":"aa"}"#).is_err());
        assert!(parse_manifest(r#"{"version":"1","url":false,"sha256":"aa"}"#).is_err());
        assert!(parse_manifest(r#"{"version":"1","url":"https://x/y","sha256":null}"#).is_err());
    }

    #[test]
    fn needs_update_compares_trimmed_prefix_only() {
        assert!(!needs_update("vv1", "v1"));
        assert!(needs_update(" 1", "1"));
        assert!(!needs_update("v2026.06.23", "2026.06.23"));
    }

    #[tokio::test]
    async fn update_returns_when_manifest_version_matches_current() {
        let _env_lock = ENV_LOCK.lock().await;
        let manifest_url = serve_update(agent_core::VERSION, "00", None);
        let _guard = EnvGuard::set_manifest(&manifest_url);

        update().await.expect("already up to date");
    }

    #[tokio::test]
    async fn update_requires_manifest_env() {
        let _env_lock = ENV_LOCK.lock().await;
        let _guard = EnvGuard::unset_manifest();

        let err = update().await.expect_err("missing manifest env");
        assert!(err.report().message.contains("FLEETY_UPDATE_MANIFEST"));
    }

    #[tokio::test]
    async fn update_downloads_artifact_and_rejects_hash_mismatch_before_install() {
        let _env_lock = ENV_LOCK.lock().await;
        let artifact = b"new fleetyd bytes".to_vec();
        let version = format!("{}-next", agent_core::VERSION);
        let manifest_url = serve_update(&version, "00", Some(artifact));
        let _guard = EnvGuard::set_manifest(&manifest_url);

        let err = update()
            .await
            .expect_err("hash mismatch must stop before exe replacement");
        assert!(err.report().message.contains("sha256 mismatch"));
    }
}
