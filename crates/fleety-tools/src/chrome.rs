//! Chrome/Chromium auto-provisioning for the browser (CDP) tools.
//!
//! The browser tools talk CDP to a Chrome started with `--remote-debugging-port`.
//! [`ensure_chrome`] guarantees such an endpoint is reachable before we connect,
//! so the agent can drive a browser on any device without the operator setting
//! one up by hand:
//!
//! 1. Endpoint already reachable → use it.
//! 2. Endpoint is local but down → find an installed Chrome/Chromium and launch
//!    it headless on the port (config + run).
//! 3. None installed + auto-install on → try the OS package manager
//!    (winget / brew / apt|snap), then fall back to a **chrome-for-testing**
//!    `chrome-headless-shell` download unpacked into a managed cache dir.
//! 4. Launch it and poll until the CDP endpoint answers.
//!
//! `FLEETY_CHROME_AUTO_INSTALL=0` disables steps 3-4 (detect+launch still run).
//! `FLEETY_CHROME_BIN` forces a specific binary. `FLEETY_CHROME_DIR` overrides
//! the managed-download cache dir.
//!
//! Remote endpoints (a non-loopback `FLEETY_CHROME_URL` / `chrome=` arg) are
//! never provisioned — we can't reach into another host's package manager — so
//! ensure_chrome just lets the connect attempt surface its own error.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use agent_core::{CoreError, Result};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

const DEFAULT_PORT: u16 = 9222;
const CFT_VERSIONS_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

fn auto_install_enabled() -> bool {
    std::env::var("FLEETY_CHROME_AUTO_INSTALL").as_deref() != Ok("0")
}

/// Keeps any Chrome we launched alive for the process lifetime, and serialises
/// launches so concurrent browser calls don't spawn duplicates.
fn launch_guard() -> &'static AsyncMutex<Option<tokio::process::Child>> {
    static G: OnceLock<AsyncMutex<Option<tokio::process::Child>>> = OnceLock::new();
    G.get_or_init(|| AsyncMutex::new(None))
}

/// Split a `http://host:port` base into `(host, port)`.
fn parse_base(base: &str) -> (String, u16) {
    let trimmed = base
        .trim()
        .trim_end_matches('/')
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let authority = trimmed.split('/').next().unwrap_or(trimmed);
    match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().unwrap_or(DEFAULT_PORT)),
        None => (authority.to_string(), DEFAULT_PORT),
    }
}

/// Whether a host refers to this machine (so we may launch Chrome for it).
fn is_local_host(host: &str) -> bool {
    matches!(
        host,
        "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "[::1]"
    )
}

/// Whether the CDP endpoint at `base` answers within a short timeout.
async fn reachable(base: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .get(format!("{base}/json/version"))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Ensure a CDP endpoint at `base` is reachable, provisioning a local Chrome if
/// needed and permitted. Best-effort: on failure it returns an actionable error
/// but the browser tool will also surface its own connect error.
pub async fn ensure_chrome(base: &str) -> Result<()> {
    if reachable(base).await {
        return Ok(());
    }
    let (host, port) = parse_base(base);
    if !is_local_host(&host) {
        // Can't provision a remote browser; let the caller's connect fail clearly.
        return Ok(());
    }

    let mut guard = launch_guard().lock().await;
    // Another task may have launched while we waited on the lock.
    if reachable(base).await {
        return Ok(());
    }

    let bin = match resolve_or_install_chrome().await {
        Some(b) => b,
        None => {
            return Err(CoreError::Message(format!(
                "no Chrome/Chromium found and auto-install is off or failed; install Chrome or set \
                 FLEETY_CHROME_BIN, or run a Chrome with --remote-debugging-port={port}"
            )))
        }
    };

    let child = launch_headless(&bin, port)?;
    *guard = Some(child);

    // Poll until the freshly launched Chrome answers (up to ~20s).
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if reachable(base).await {
            tracing::info!(binary = %bin.display(), port, "launched Chrome for CDP");
            return Ok(());
        }
    }
    Err(CoreError::Message(format!(
        "launched {} but its CDP endpoint never came up on port {port}",
        bin.display()
    )))
}

/// Launch a headless Chrome on `port` with an isolated profile.
fn launch_headless(bin: &Path, port: u16) -> Result<tokio::process::Child> {
    let profile = std::env::temp_dir().join(format!("fleety-chrome-{port}"));
    let is_headless_shell = bin
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.contains("headless-shell"))
        .unwrap_or(false);

    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-gpu")
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // chrome-headless-shell is headless by nature; full Chrome needs the flag.
    if !is_headless_shell {
        cmd.arg("--headless=new");
    }
    // Sandbox can't initialise as root in many containers; pragmatic for our
    // controlled CDP use.
    if cfg!(target_os = "linux") {
        cmd.arg("--no-sandbox");
    }
    cmd.spawn()
        .map_err(|e| CoreError::Message(format!("cannot launch Chrome '{}': {e}", bin.display())))
}

/// Find an existing Chrome, else (when allowed) install one via the package
/// manager or a chrome-for-testing download. Returns the binary path.
async fn resolve_or_install_chrome() -> Option<PathBuf> {
    if let Some(b) = find_chrome_binary() {
        return Some(b);
    }
    if !auto_install_enabled() {
        return None;
    }
    if try_package_manager_install().await {
        if let Some(b) = find_chrome_binary() {
            return Some(b);
        }
    }
    download_chrome_for_testing().await
}

/// Resolve an explicit override, a previously-downloaded managed binary, or a
/// platform/PATH-installed Chrome/Chromium.
fn find_chrome_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FLEETY_CHROME_BIN") {
        if !p.is_empty() && Path::new(&p).is_file() {
            return Some(PathBuf::from(p));
        }
    }
    if let Some(b) = newest_managed_binary() {
        return Some(b);
    }
    for cand in chrome_candidates() {
        if Path::new(&cand).is_file() {
            return Some(PathBuf::from(cand));
        }
        if let Some(found) = which(&cand) {
            return Some(found);
        }
    }
    None
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Platform-specific Chrome/Chromium binary names and well-known absolute paths.
fn chrome_candidates() -> Vec<String> {
    if cfg!(target_os = "windows") {
        let mut v = vec![
            "chrome.exe".to_string(),
            "chrome-headless-shell.exe".to_string(),
        ];
        for base in [
            std::env::var("ProgramFiles").ok(),
            std::env::var("ProgramFiles(x86)").ok(),
            std::env::var("LocalAppData").ok(),
        ]
        .into_iter()
        .flatten()
        {
            v.push(format!("{base}\\Google\\Chrome\\Application\\chrome.exe"));
        }
        v
    } else if cfg!(target_os = "macos") {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            "google-chrome".to_string(),
            "chromium".to_string(),
            "chrome-headless-shell".to_string(),
        ]
    } else {
        vec![
            "google-chrome".to_string(),
            "google-chrome-stable".to_string(),
            "chromium".to_string(),
            "chromium-browser".to_string(),
            "chrome".to_string(),
            "chrome-headless-shell".to_string(),
        ]
    }
}

/// Best-effort OS package-manager install. Returns true on a successful run.
async fn try_package_manager_install() -> bool {
    let attempts: &[(&str, &[&str])] = if cfg!(target_os = "windows") {
        &[(
            "winget",
            &[
                "install",
                "-e",
                "--id",
                "Google.Chrome",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ],
        )]
    } else if cfg!(target_os = "macos") {
        &[("brew", &["install", "--cask", "google-chrome"])]
    } else {
        // Linux: chromium is in most default repos; needs root for apt.
        &[
            ("apt-get", &["install", "-y", "chromium"]),
            ("apt-get", &["install", "-y", "chromium-browser"]),
            ("snap", &["install", "chromium"]),
        ]
    };
    for (cmd, args) in attempts {
        tracing::info!(
            installer = cmd,
            "trying to install Chrome via package manager"
        );
        match tokio::process::Command::new(cmd)
            .args(*args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
        {
            Ok(s) if s.success() => return true,
            Ok(_) | Err(_) => continue,
        }
    }
    false
}

// --- chrome-for-testing managed download ---

/// The managed cache dir where downloaded chrome-for-testing builds live.
fn managed_dir() -> PathBuf {
    if let Ok(d) = std::env::var("FLEETY_CHROME_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join(".fleety").join("chrome")
}

/// The chrome-for-testing platform key for this host.
fn cft_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("win64"),
        ("windows", "x86") => Some("win32"),
        ("macos", "aarch64") => Some("mac-arm64"),
        ("macos", "x86_64") => Some("mac-x64"),
        ("linux", "x86_64") => Some("linux64"),
        _ => None,
    }
}

/// Find the newest already-downloaded managed `chrome-headless-shell` binary.
fn newest_managed_binary() -> Option<PathBuf> {
    let dir = managed_dir();
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // Lexical sort is close enough to pick a recent one; we only keep latest.
    versions.sort();
    for v in versions.into_iter().rev() {
        if let Some(bin) = find_headless_shell_in(&v) {
            return Some(bin);
        }
    }
    None
}

/// Locate the `chrome-headless-shell[.exe]` binary inside an extracted CfT dir.
fn find_headless_shell_in(dir: &Path) -> Option<PathBuf> {
    let exe = if cfg!(target_os = "windows") {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    };
    // CfT zips extract to `chrome-headless-shell-<platform>/<exe>`.
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        let direct = p.join(exe);
        if direct.is_file() {
            return Some(direct);
        }
    }
    let flat = dir.join(exe);
    if flat.is_file() {
        Some(flat)
    } else {
        None
    }
}

/// Parse the CfT versions JSON for this platform's `chrome-headless-shell`
/// `(version, download_url)`.
fn parse_cft_download(json: &Value, platform: &str) -> Option<(String, String)> {
    let stable = json.get("channels")?.get("Stable")?;
    let version = stable.get("version")?.as_str()?.to_string();
    let downloads = stable
        .get("downloads")?
        .get("chrome-headless-shell")?
        .as_array()?;
    let url = downloads
        .iter()
        .find(|d| d.get("platform").and_then(Value::as_str) == Some(platform))?
        .get("url")?
        .as_str()?
        .to_string();
    Some((version, url))
}

/// Download + unzip the latest chrome-for-testing `chrome-headless-shell` into
/// the managed dir and return the binary path.
async fn download_chrome_for_testing() -> Option<PathBuf> {
    let platform = cft_platform()?;
    tracing::info!(
        platform,
        "downloading chrome-for-testing (chrome-headless-shell)"
    );
    let json: Value = reqwest::get(CFT_VERSIONS_URL)
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let (version, url) = parse_cft_download(&json, platform)?;

    let dest_dir = managed_dir().join(&version);
    if let Some(bin) = find_headless_shell_in(&dest_dir) {
        return Some(bin); // already have this version
    }
    std::fs::create_dir_all(&dest_dir).ok()?;

    let bytes = reqwest::get(&url).await.ok()?.bytes().await.ok()?;
    extract_zip(&bytes, &dest_dir).ok()?;

    let bin = find_headless_shell_in(&dest_dir)?;
    make_executable(&bin);
    Some(bin)
}

/// Extract a zip archive (in memory) into `dest`.
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| CoreError::Message(format!("chrome download is not a valid zip: {e}")))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| CoreError::Message(format!("corrupt zip entry: {e}")))?;
        let Some(rel) = file.enclosed_name() else {
            continue; // skip unsafe paths (zip-slip guard)
        };
        let out = dest.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| CoreError::Message(format!("mk dir during unzip: {e}")))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("mk dir during unzip: {e}")))?;
        }
        let mut sink = std::fs::File::create(&out)
            .map_err(|e| CoreError::Message(format!("write during unzip: {e}")))?;
        std::io::copy(&mut file, &mut sink)
            .map_err(|e| CoreError::Message(format!("copy during unzip: {e}")))?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(perm.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// Best-effort update of a managed chrome-for-testing install to the latest
/// Stable build. No-op when we don't manage the install (a system / package-
/// manager Chrome self-updates, so there's nothing for us to do). Downloads the
/// new version alongside the old and prunes older managed versions on success.
pub async fn upgrade_managed_chrome() {
    if !auto_install_enabled() {
        return;
    }
    // Only relevant if we already manage a downloaded build.
    if newest_managed_binary().is_none() {
        return;
    }
    if let Some(bin) = download_chrome_for_testing().await {
        tracing::info!(binary = %bin.display(), "chrome-for-testing up to date");
        prune_old_managed(&bin);
    }
}

/// Remove managed version dirs other than the one holding `keep`.
fn prune_old_managed(keep: &Path) {
    let dir = managed_dir();
    let keep_top = keep
        .strip_prefix(&dir)
        .ok()
        .and_then(|r| r.components().next());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let is_keep = keep_top
            .map(|k| p.file_name() == Some(k.as_os_str()))
            .unwrap_or(false);
        if !is_keep {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_base_variants() {
        assert_eq!(
            parse_base("http://127.0.0.1:9222"),
            ("127.0.0.1".into(), 9222)
        );
        assert_eq!(
            parse_base("http://localhost:9333/"),
            ("localhost".into(), 9333)
        );
        assert_eq!(
            parse_base("http://example.com"),
            ("example.com".into(), DEFAULT_PORT)
        );
        assert_eq!(parse_base("127.0.0.1:9999"), ("127.0.0.1".into(), 9999));
    }

    #[test]
    fn local_host_detection() {
        assert!(is_local_host("127.0.0.1"));
        assert!(is_local_host("localhost"));
        assert!(!is_local_host("example.com"));
        assert!(!is_local_host("10.0.0.5"));
    }

    #[test]
    fn parse_cft_picks_matching_platform() {
        let doc = json!({
            "channels": {
                "Stable": {
                    "version": "126.0.6478.0",
                    "downloads": {
                        "chrome-headless-shell": [
                            { "platform": "linux64", "url": "https://x/linux.zip" },
                            { "platform": "win64", "url": "https://x/win.zip" }
                        ]
                    }
                }
            }
        });
        let (v, url) = parse_cft_download(&doc, "win64").expect("found");
        assert_eq!(v, "126.0.6478.0");
        assert_eq!(url, "https://x/win.zip");
        assert!(parse_cft_download(&doc, "mac-arm64").is_none());
    }

    #[test]
    fn cft_platform_known_or_none() {
        // On any CI host this is one of the known keys or None — must not panic.
        let _ = cft_platform();
    }
}
