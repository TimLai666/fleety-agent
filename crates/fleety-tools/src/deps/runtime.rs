//! Managed portable runtimes: install node / python into a fleety-owned dir and
//! prepend their bin dirs to *this process's* PATH (so spawned MCP servers and
//! skill scripts find them) — without root and without touching the user's
//! system PATH.
//!
//! - **node**: the official portable build from nodejs.org/dist.
//! - **python**: provisioned via `uv` (Astral's standalone binary), which fetches
//!   a managed CPython and builds a venv.
//!
//! The platform → URL / path / PATH-string mapping is pure and unit-tested. The
//! actual download + extract + venv build is best-effort and verified manually
//! (network/disk dependent), like the insyra sidecar and the model provider.

use std::ffi::OsString;
use std::path::PathBuf;

use agent_core::{CoreError, Result};

/// Default pinned node version (override with `FLEETY_NODE_VERSION`).
const DEFAULT_NODE_VERSION: &str = "v20.18.0";

/// The managed runtimes root, `~/.fleety/runtimes/` (override `FLEETY_RUNTIMES_DIR`).
pub fn runtimes_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FLEETY_RUNTIMES_DIR") {
        return PathBuf::from(dir);
    }
    crate::device::home_dir().join(".fleety").join("runtimes")
}

fn node_version() -> String {
    std::env::var("FLEETY_NODE_VERSION").unwrap_or_else(|_| DEFAULT_NODE_VERSION.to_string())
}

// ---- pure platform mapping (node) ----

/// node's (os, arch, archive-ext) tokens for a Rust `(os, arch)`, or None when
/// unsupported. `os` is `std::env::consts::OS`, `arch` is `..::ARCH`.
pub fn node_platform_for(
    os: &str,
    arch: &str,
) -> Option<(&'static str, &'static str, &'static str)> {
    let nos = match os {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win",
        _ => return None,
    };
    let narch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let ext = if os == "windows" { "zip" } else { "tar.xz" };
    Some((nos, narch, ext))
}

/// The node archive stem, e.g. `node-v20.18.0-linux-x64`.
pub fn node_stem_for(os: &str, arch: &str, version: &str) -> Option<String> {
    let (nos, narch, _) = node_platform_for(os, arch)?;
    Some(format!("node-{version}-{nos}-{narch}"))
}

/// The node download URL for a platform/version, or None if unsupported.
pub fn node_url_for(os: &str, arch: &str, version: &str) -> Option<String> {
    let (_, _, ext) = node_platform_for(os, arch)?;
    let stem = node_stem_for(os, arch, version)?;
    Some(format!("https://nodejs.org/dist/{version}/{stem}.{ext}"))
}

/// Where node's bin dir lands under `runtimes` (unix: `<stem>/bin`; the Windows
/// build puts `node.exe` at the archive root).
pub fn node_bin_dir_for(
    runtimes: &std::path::Path,
    os: &str,
    arch: &str,
    version: &str,
) -> Option<PathBuf> {
    let stem = node_stem_for(os, arch, version)?;
    let base = runtimes.join("node").join(&stem);
    Some(if os == "windows" {
        base
    } else {
        base.join("bin")
    })
}

// ---- pure platform mapping (uv / python) ----

/// uv's release target triple for a Rust `(os, arch)`, or None when unsupported.
pub fn uv_target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (arch, os) {
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("x86_64", "windows") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// uv download URL for a platform, or None if unsupported.
pub fn uv_url_for(os: &str, arch: &str) -> Option<String> {
    let target = uv_target_for(os, arch)?;
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Some(format!(
        "https://github.com/astral-sh/uv/releases/latest/download/uv-{target}.{ext}"
    ))
}

/// The managed python venv's bin (unix `bin`, Windows `Scripts`).
pub fn python_bin_dir(runtimes: &std::path::Path) -> PathBuf {
    let venv = runtimes.join("py");
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}

// ---- pure PATH composition ----

/// Prepend `dirs` to `current` PATH, returning the joined value. Pure.
pub fn path_prepend(dirs: &[PathBuf], current: Option<OsString>) -> OsString {
    let mut paths: Vec<PathBuf> = dirs.to_vec();
    if let Some(cur) = &current {
        paths.extend(std::env::split_paths(cur));
    }
    std::env::join_paths(paths).unwrap_or_else(|_| current.unwrap_or_default())
}

/// Prepend `dirs` to this process's PATH (children inherit it). Does not touch
/// the user's system PATH.
pub fn inject_path(dirs: &[PathBuf]) {
    let joined = path_prepend(dirs, std::env::var_os("PATH"));
    std::env::set_var("PATH", joined);
}

// ---- best-effort install (manual-verify) ----

/// Whether a usable node is on PATH.
pub async fn has_node() -> bool {
    runs("node").await
}

/// Whether a usable python is on PATH.
pub async fn has_python() -> bool {
    runs("python3").await || runs("python").await
}

/// Whether `<cmd> --version` succeeds on the current PATH.
async fn runs(cmd: &str) -> bool {
    tokio::process::Command::new(cmd)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn download(url: &str, dest: &std::path::Path) -> Result<()> {
    let bytes = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("download {url} failed: {e}")))?
        .error_for_status()
        .map_err(|e| CoreError::Provider(format!("download {url} failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| CoreError::Provider(format!("read {url} failed: {e}")))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(dest, &bytes)
        .map_err(|e| CoreError::Message(format!("cannot write {}: {e}", dest.display())))?;
    Ok(())
}

/// Extract an archive into `into`. `.zip` via the `zip` crate; `.tar.*` by
/// shelling out to `tar -xf` (present on Linux/macOS and modern Windows).
fn extract(archive: &std::path::Path, into: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(into)
        .map_err(|e| CoreError::Message(format!("cannot create {}: {e}", into.display())))?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let file = std::fs::File::open(archive)
            .map_err(|e| CoreError::Message(format!("open {}: {e}", archive.display())))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| CoreError::Message(format!("read zip {}: {e}", archive.display())))?;
        zip.extract(into)
            .map_err(|e| CoreError::Message(format!("extract zip: {e}")))?;
        Ok(())
    } else {
        let status = std::process::Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(into)
            .status()
            .map_err(|e| CoreError::Message(format!("tar not available to extract {name}: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(CoreError::Message(format!("tar failed to extract {name}")))
        }
    }
}

/// Ensure a managed node is installed and on this process's PATH (best-effort).
pub async fn ensure_node() -> Result<()> {
    if runs("node").await {
        return Ok(());
    }
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let version = node_version();
    let url = node_url_for(os, arch, &version)
        .ok_or_else(|| CoreError::Message(format!("no portable node for {os}/{arch}")))?;
    let runtimes = runtimes_dir();
    let node_root = runtimes.join("node");
    let (_, _, ext) = node_platform_for(os, arch).unwrap_or(("", "", "tar.xz"));
    let archive = node_root.join(format!("node-download.{ext}"));
    tracing::info!(%url, "downloading managed node");
    download(&url, &archive).await?;
    extract(&archive, &node_root)?;
    let _ = std::fs::remove_file(&archive);
    if let Some(bin) = node_bin_dir_for(&runtimes, os, arch, &version) {
        inject_path(&[bin]);
    }
    if runs("node").await {
        Ok(())
    } else {
        Err(CoreError::Message(
            "installed managed node but it is still not runnable on PATH".to_string(),
        ))
    }
}

/// Ensure a managed python (via uv) is installed and on PATH (best-effort).
pub async fn ensure_python() -> Result<()> {
    if runs("python3").await || runs("python").await {
        return Ok(());
    }
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let runtimes = runtimes_dir();
    // 1. Ensure uv itself (standalone binary) under the runtimes dir.
    let uv_dir = runtimes.join("uv");
    let uv_bin = uv_dir.join(if cfg!(windows) { "uv.exe" } else { "uv" });
    if !uv_bin.is_file() {
        let url = uv_url_for(os, arch)
            .ok_or_else(|| CoreError::Message(format!("no uv build for {os}/{arch}")))?;
        let ext = if os == "windows" { "zip" } else { "tar.gz" };
        let archive = uv_dir.join(format!("uv-download.{ext}"));
        tracing::info!(%url, "downloading uv to provision python");
        download(&url, &archive).await?;
        extract(&archive, &uv_dir)?;
        let _ = std::fs::remove_file(&archive);
    }
    // uv's archives may nest the binary in a subdir; add the uv dir to PATH and
    // also try the direct path.
    inject_path(std::slice::from_ref(&uv_dir));
    let uv_cmd = if uv_bin.is_file() {
        uv_bin.to_string_lossy().into_owned()
    } else {
        "uv".to_string()
    };
    // 2. Build a managed venv (uv fetches a managed CPython if none present).
    let venv = runtimes.join("py");
    let status = tokio::process::Command::new(&uv_cmd)
        .arg("venv")
        .arg(&venv)
        .arg("--python")
        .arg("3.12")
        .status()
        .await
        .map_err(|e| CoreError::Message(format!("uv venv failed to run: {e}")))?;
    if !status.success() {
        return Err(CoreError::Message(
            "uv venv did not succeed; managed python not provisioned".to_string(),
        ));
    }
    inject_path(&[python_bin_dir(&runtimes)]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_platform_mapping_known_and_unknown() {
        assert_eq!(
            node_platform_for("linux", "x86_64"),
            Some(("linux", "x64", "tar.xz"))
        );
        assert_eq!(
            node_platform_for("macos", "aarch64"),
            Some(("darwin", "arm64", "tar.xz"))
        );
        assert_eq!(
            node_platform_for("windows", "x86_64"),
            Some(("win", "x64", "zip"))
        );
        assert_eq!(node_platform_for("plan9", "x86_64"), None);
        assert_eq!(node_platform_for("linux", "sparc"), None);
    }

    #[test]
    fn node_url_shapes() {
        assert_eq!(
            node_url_for("linux", "x86_64", "v20.18.0").as_deref(),
            Some("https://nodejs.org/dist/v20.18.0/node-v20.18.0-linux-x64.tar.xz")
        );
        assert_eq!(
            node_url_for("windows", "x86_64", "v20.18.0").as_deref(),
            Some("https://nodejs.org/dist/v20.18.0/node-v20.18.0-win-x64.zip")
        );
        assert_eq!(node_url_for("plan9", "x86_64", "v20.18.0"), None);
    }

    #[test]
    fn node_bin_dir_layout() {
        let r = PathBuf::from("/r");
        let unix = node_bin_dir_for(&r, "linux", "x86_64", "v20.18.0").unwrap();
        assert!(unix.ends_with("node/node-v20.18.0-linux-x64/bin"));
        let win = node_bin_dir_for(&r, "windows", "x86_64", "v20.18.0").unwrap();
        assert!(win.ends_with("node/node-v20.18.0-win-x64"));
    }

    #[test]
    fn uv_mapping_known_and_unknown() {
        assert_eq!(
            uv_target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            uv_target_for("macos", "aarch64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(uv_target_for("plan9", "x86_64"), None);
        assert_eq!(
            uv_url_for("windows", "x86_64").as_deref(),
            Some("https://github.com/astral-sh/uv/releases/latest/download/uv-x86_64-pc-windows-msvc.zip")
        );
        assert!(uv_url_for("linux", "x86_64").unwrap().ends_with(".tar.gz"));
    }

    #[test]
    fn path_prepend_puts_dirs_first() {
        let dirs = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let cur = std::env::join_paths([PathBuf::from("/usr/bin")]).ok();
        let joined = path_prepend(&dirs, cur);
        let parts: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(parts[0], PathBuf::from("/a"));
        assert_eq!(parts[1], PathBuf::from("/b"));
        assert!(parts.iter().any(|p| p == &PathBuf::from("/usr/bin")));
    }
}
