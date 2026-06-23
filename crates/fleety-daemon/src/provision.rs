//! Provision the bundled sidecars onto this device:
//!
//! * `fleety-insyra` — Insyra DSL sidecar, raw per-target binary
//! * `codebase-memory-mcp` — built-in MCP code knowledge graph, distributed as
//!   tar.gz/zip with the binary inside
//!
//! Both land next to the fleetyd executable so the on-device tools (which look
//! beside the current exe) find them. Run from `fleetyd install` (fetch if
//! missing) and `fleetyd update` (refresh). Best-effort: a failure here leaves
//! the dependent tool returning an actionable error — it does not stop fleetyd
//! from running. Downloads are exercised manually (same posture as the
//! self-update download); the pure URL/target logic is unit-tested.

use std::path::{Path, PathBuf};
use std::process::Command;

use agent_core::{CoreError, Result};

const REPO: &str = "TimLai666/fleety-agent";
const CBM_REPO: &str = "DeusData/codebase-memory-mcp";

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
    Ok(exe_dir()?.join(sidecar_filename()))
}

/// Directory the running fleetyd lives in — where all bundled sidecars land.
fn exe_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| CoreError::Message("current exe has no parent directory".to_string()))?;
    Ok(dir.to_path_buf())
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

// ----------------------------------------------------------------------------
// codebase-memory-mcp — built-in MCP code knowledge graph
// ----------------------------------------------------------------------------

/// `(asset_basename, archive_extension)` for this build, or None if unsupported.
/// The release also publishes `-portable` (musl-static) variants for Linux; we
/// pick those so the binary runs on any glibc.
fn cbm_asset() -> Option<(String, &'static str)> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => Some((
            "codebase-memory-mcp-linux-amd64-portable".to_string(),
            "tar.gz",
        )),
        ("aarch64", "linux") => Some((
            "codebase-memory-mcp-linux-arm64-portable".to_string(),
            "tar.gz",
        )),
        ("aarch64", "macos") => Some(("codebase-memory-mcp-darwin-arm64".to_string(), "tar.gz")),
        ("x86_64", "macos") => Some(("codebase-memory-mcp-darwin-amd64".to_string(), "tar.gz")),
        ("x86_64", "windows") => Some(("codebase-memory-mcp-windows-amd64".to_string(), "zip")),
        _ => None,
    }
}

fn cbm_binary_name() -> &'static str {
    if cfg!(windows) {
        "codebase-memory-mcp.exe"
    } else {
        "codebase-memory-mcp"
    }
}

fn cbm_dest_path() -> Result<PathBuf> {
    Ok(exe_dir()?.join(cbm_binary_name()))
}

/// Download URL for this platform's codebase-memory-mcp archive (env override
/// for tests/mirrors).
fn cbm_url() -> Result<String> {
    if let Ok(url) = std::env::var("FLEETY_CBM_URL") {
        return Ok(url);
    }
    let (asset, ext) = cbm_asset().ok_or_else(|| {
        CoreError::Message(format!(
            "no prebuilt codebase-memory-mcp for {}/{}",
            std::env::consts::ARCH,
            std::env::consts::OS
        ))
    })?;
    Ok(format!(
        "https://github.com/{CBM_REPO}/releases/latest/download/{asset}.{ext}"
    ))
}

/// Find the codebase-memory-mcp binary inside an extracted archive (it may sit
/// at the root or one directory deep depending on the release layout).
fn find_cbm_binary(root: &Path) -> Result<PathBuf> {
    let target = cbm_binary_name();
    fn walk(dir: &Path, target: &str, depth: u32) -> Option<PathBuf> {
        if depth > 4 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let ty = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ty.is_file() && path.file_name().map(|n| n == target).unwrap_or(false) {
                return Some(path);
            }
            if ty.is_dir() {
                if let Some(found) = walk(&path, target, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(root, target, 0).ok_or_else(|| {
        CoreError::Message(format!(
            "could not find '{target}' inside the extracted archive at {}",
            root.display()
        ))
    })
}

/// Ensure the codebase-memory-mcp binary sits next to fleetyd; download and
/// extract it when missing (or when `force`, e.g. on `update`).
pub async fn ensure_codebase_memory(force: bool) -> Result<()> {
    let dest = cbm_dest_path()?;
    if dest.is_file() && !force {
        tracing::info!(path = %dest.display(), "codebase-memory-mcp already present");
        return Ok(());
    }
    let url = cbm_url()?;
    tracing::info!(%url, "downloading codebase-memory-mcp");
    let bytes = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("download codebase-memory-mcp failed: {e}")))?
        .error_for_status()
        .map_err(|e| CoreError::Provider(format!("download codebase-memory-mcp failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| CoreError::Provider(format!("read codebase-memory-mcp failed: {e}")))?;

    // Stage in a unique temp dir so a partial extract never leaves debris next
    // to fleetyd.
    let tmp = std::env::temp_dir().join(format!("fleety-cbm-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)
        .map_err(|e| CoreError::Message(format!("cannot create cbm temp dir: {e}")))?;
    let cleanup = TempDirGuard(tmp.clone());

    // Choose an archive filename so `tar` infers the format. Modern Windows
    // ships bsdtar, which handles both tar.gz and zip via `tar -xf`.
    let (_, ext) = cbm_asset()
        .ok_or_else(|| CoreError::Message("unsupported target for codebase-memory-mcp".into()))?;
    let archive = tmp.join(format!("cbm.{ext}"));
    std::fs::write(&archive, &bytes)
        .map_err(|e| CoreError::Message(format!("cannot stage cbm archive: {e}")))?;

    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .map_err(|e| {
            CoreError::Message(format!(
                "cannot run 'tar' to extract codebase-memory-mcp: {e}"
            ))
        })?;
    if !status.success() {
        return Err(CoreError::Message(format!(
            "'tar' failed to extract codebase-memory-mcp (exit {:?})",
            status.code()
        )));
    }

    let extracted = find_cbm_binary(&tmp)?;
    let staged = dest.with_extension("new");
    std::fs::copy(&extracted, &staged)
        .map_err(|e| CoreError::Message(format!("cannot stage cbm binary: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| CoreError::Message(format!("cannot mark cbm executable: {e}")))?;
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&staged, &dest)
        .map_err(|e| CoreError::Message(format!("cannot install cbm binary: {e}")))?;
    drop(cleanup);
    tracing::info!(path = %dest.display(), bytes = bytes.len(), "codebase-memory-mcp installed");
    Ok(())
}

/// Best-effort temp-dir cleanup on drop, so an error path doesn't strand
/// extracted archives.
struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

    // One test (not two) so the shared FLEETY_INSYRA_URL env var can't race
    // between tests running on parallel threads.
    #[test]
    fn url_override_then_default() {
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
    fn cbm_asset_and_filename_are_consistent() {
        let _ = cbm_asset();
        if cfg!(windows) {
            assert_eq!(cbm_binary_name(), "codebase-memory-mcp.exe");
        } else {
            assert_eq!(cbm_binary_name(), "codebase-memory-mcp");
        }
    }

    // Single test so the shared FLEETY_CBM_URL env var can't race between
    // parallel threads (mirrors the insyra pattern above).
    #[test]
    fn cbm_url_override_then_default() {
        std::env::set_var("FLEETY_CBM_URL", "https://example.test/cbm.tar.gz");
        assert_eq!(cbm_url().unwrap(), "https://example.test/cbm.tar.gz");

        std::env::remove_var("FLEETY_CBM_URL");
        if let Some((asset, ext)) = cbm_asset() {
            let url = cbm_url().expect("url");
            assert!(url.contains(&format!("{asset}.{ext}")));
            assert!(url.starts_with("https://github.com/"));
        }
    }

    #[test]
    fn find_cbm_binary_walks_nested_dirs() {
        let dir = std::env::temp_dir().join(format!("fleety-cbm-walk-{}", uuid::Uuid::new_v4()));
        let nested = dir.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("mk nested");
        let target = nested.join(cbm_binary_name());
        std::fs::write(&target, b"dummy").expect("write dummy");
        let found = find_cbm_binary(&dir).expect("found");
        assert_eq!(found, target);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
