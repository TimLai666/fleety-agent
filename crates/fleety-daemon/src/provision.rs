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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct EnvGuard(Option<String>);

    impl EnvGuard {
        fn set_url(url: &str) -> Self {
            let old = std::env::var("FLEETY_INSYRA_URL").ok();
            std::env::set_var("FLEETY_INSYRA_URL", url);
            Self(old)
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("FLEETY_INSYRA_URL", value),
                None => std::env::remove_var("FLEETY_INSYRA_URL"),
            }
        }
    }

    struct SidecarGuard {
        path: PathBuf,
        original: Option<Vec<u8>>,
    }

    impl SidecarGuard {
        fn new() -> Self {
            let path = dest_path().expect("dest path");
            let original = std::fs::read(&path).ok();
            Self { path, original }
        }

        fn write(&self, bytes: &[u8]) {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).expect("sidecar parent");
            }
            std::fs::write(&self.path, bytes).expect("sidecar");
        }
    }

    impl Drop for SidecarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(bytes) => {
                    let _ = std::fs::write(&self.path, bytes);
                }
                None => {
                    let _ = std::fs::remove_file(&self.path);
                }
            }
            let _ = std::fs::remove_file(self.path.with_extension("new"));
        }
    }

    fn serve_once(status: &'static str, body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(header.as_bytes())
                .and_then(|_| stream.write_all(body))
                .expect("write response");
        });
        format!("http://{addr}/sidecar")
    }

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

    // One test (not two) so the shared FLEETY_INSYRA_URL env var can't race
    // between tests running on parallel threads.
    #[tokio::test]
    async fn url_override_then_default() {
        let _env_lock = ENV_LOCK.lock().await;
        std::env::set_var("FLEETY_INSYRA_URL", "https://example.test/sidecar");
        assert_eq!(sidecar_url().unwrap(), "https://example.test/sidecar");

        std::env::remove_var("FLEETY_INSYRA_URL");
        if let Some(target) = target_triple() {
            let url = sidecar_url().expect("url");
            assert!(url.contains(&format!("fleety-insyra-{target}")));
            assert!(url.starts_with("https://github.com/"));
        }
    }

    #[test]
    fn destination_is_next_to_current_exe_with_platform_filename() {
        let dest = dest_path().expect("dest path");
        assert_eq!(
            dest.file_name().and_then(|s| s.to_str()),
            Some(sidecar_filename())
        );
        assert!(dest.parent().is_some());
    }

    #[tokio::test]
    async fn ensure_insyra_skips_existing_sidecar_unless_forced() {
        let _env_lock = ENV_LOCK.lock().await;
        let sidecar = SidecarGuard::new();
        sidecar.write(b"existing sidecar");
        let _guard = EnvGuard::set_url("http://127.0.0.1:1/must-not-be-called");

        ensure_insyra(false).await.expect("skip existing");
        assert_eq!(
            std::fs::read(&sidecar.path).expect("sidecar"),
            b"existing sidecar"
        );
    }

    #[tokio::test]
    async fn ensure_insyra_downloads_override_and_replaces_sidecar() {
        let _env_lock = ENV_LOCK.lock().await;
        let sidecar = SidecarGuard::new();
        sidecar.write(b"old sidecar");
        let url = serve_once("200 OK", b"downloaded sidecar");
        let _guard = EnvGuard::set_url(&url);

        ensure_insyra(true).await.expect("download sidecar");
        assert_eq!(
            std::fs::read(&sidecar.path).expect("sidecar"),
            b"downloaded sidecar"
        );
    }

    #[tokio::test]
    async fn ensure_insyra_reports_download_status_errors() {
        let _env_lock = ENV_LOCK.lock().await;
        let sidecar = SidecarGuard::new();
        let url = serve_once("404 Not Found", b"missing");
        let _guard = EnvGuard::set_url(&url);

        let err = ensure_insyra(true)
            .await
            .expect_err("bad http status should fail");
        assert!(err
            .report()
            .message
            .contains("download fleety-insyra failed"));
        assert!(!sidecar.path.with_extension("new").exists());
    }
}
