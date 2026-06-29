//! Runtime skill sync: keep the `synced` skill tier in step with an external
//! repo (default `TimLai666/skills`) without waiting for a Fleety release.
//!
//! A background task (spawned at boot, then every `FLEETY_SKILLS_SYNC_INTERVAL_SECS`)
//! first checks the repo's latest commit SHA; only when it differs from the
//! locally recorded one does it download the branch zip, rebuild the synced tier
//! in a staging dir from the repo's top-level skill directories (each a dir with
//! a `SKILL.md`), and atomically swap it in — so additions/removals are mirrored
//! and a partially-synced state is never served. Any failure keeps the previous
//! copy and logs a warning; it never crashes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_core::{CoreError, Result};

const DEFAULT_REPO: &str = "TimLai666/skills";
const DEFAULT_INTERVAL_SECS: u64 = 3600;
const SHA_FILE: &str = ".synced-sha";

// ---- pure helpers (unit-tested) ----

/// Top-level subdirectories of `root` that contain a `SKILL.md` — the skills in
/// an extracted skill repo. Loose files at the root are ignored. Sorted.
pub fn skill_dirs_from_extracted(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            if let Some(name) = entry.file_name().to_str() {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out
}

/// Whether to download + apply: yes when there's no local SHA or it differs.
pub fn should_sync(remote_sha: &str, local_sha: Option<&str>) -> bool {
    match local_sha {
        Some(local) => local != remote_sha,
        None => true,
    }
}

/// Validate an `owner/repo` slug (defends the constructed URLs).
fn valid_repo(repo: &str) -> bool {
    let parts: Vec<&str> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

// ---- filesystem helpers ----

fn msg<E: std::fmt::Display>(ctx: &str) -> impl Fn(E) -> CoreError + '_ {
    move |e| CoreError::Message(format!("{ctx}: {e}"))
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
        // symlinks are skipped
    }
    Ok(())
}

/// Build the full new synced tier in `staging` from `repo_root`: copy each
/// top-level skill dir, then record the SHA. Removals are inherent — only the
/// repo's current skills end up in staging.
fn rebuild_into(staging: &Path, repo_root: &Path, sha: &str) -> Result<()> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(msg("create staging"))?;
    for name in skill_dirs_from_extracted(repo_root) {
        copy_dir_all(&repo_root.join(&name), &staging.join(&name)).map_err(msg("copy skill"))?;
    }
    std::fs::write(staging.join(SHA_FILE), sha).map_err(msg("write sha"))?;
    Ok(())
}

/// Atomically replace `synced_dir` with `staging` (same parent → a rename).
fn swap_into(synced_dir: &Path, staging: &Path) -> Result<()> {
    if let Some(parent) = synced_dir.parent() {
        std::fs::create_dir_all(parent).map_err(msg("create skills dir"))?;
    }
    let _ = std::fs::remove_dir_all(synced_dir);
    std::fs::rename(staging, synced_dir).map_err(msg("swap synced tier"))?;
    Ok(())
}

/// Unzip an in-memory archive into `dest` (zip-slip guarded).
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(msg("not a valid zip"))?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(msg("corrupt zip entry"))?;
        let Some(rel) = file.enclosed_name() else {
            continue; // zip-slip guard
        };
        let out = dest.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(msg("unzip mkdir"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(msg("unzip mkdir"))?;
        }
        let mut sink = std::fs::File::create(&out).map_err(msg("unzip create"))?;
        std::io::copy(&mut file, &mut sink).map_err(msg("unzip write"))?;
    }
    Ok(())
}

/// GitHub's archive wraps everything in a single `<repo>-<branch>/` directory;
/// return it (the real repo root inside the extracted tree).
fn repo_root_in(extracted: &Path) -> Option<PathBuf> {
    std::fs::read_dir(extracted)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
}

// ---- network ----

async fn fetch_latest_sha(repo: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{repo}/commits/main");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "fleety-skill-sync")
        .header("Accept", "application/vnd.github.sha")
        .send()
        .await
        .map_err(msg("fetch latest sha"))?;
    if !resp.status().is_success() {
        return Err(CoreError::Message(format!(
            "fetch latest sha: HTTP {} from {url}",
            resp.status()
        )));
    }
    let sha = resp
        .text()
        .await
        .map_err(msg("read sha"))?
        .trim()
        .to_string();
    if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CoreError::Message(format!(
            "unexpected commit-sha response: {sha:?}"
        )));
    }
    Ok(sha)
}

async fn download_zip(repo: &str) -> Result<Vec<u8>> {
    let url = format!("https://codeload.github.com/{repo}/zip/refs/heads/main");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "fleety-skill-sync")
        .send()
        .await
        .map_err(msg("download skills zip"))?;
    if !resp.status().is_success() {
        return Err(CoreError::Message(format!(
            "download skills zip: HTTP {} from {url}",
            resp.status()
        )));
    }
    Ok(resp.bytes().await.map_err(msg("read skills zip"))?.to_vec())
}

/// One sync pass. Returns `Ok(true)` if the synced tier was rebuilt, `Ok(false)`
/// if the repo was unchanged (nothing downloaded). Errors are returned (the
/// caller logs + swallows them); the existing synced tier is left intact on any
/// failure because the rebuild only swaps in after staging succeeds.
async fn sync_once(synced_dir: &Path, repo: &str) -> Result<bool> {
    if !valid_repo(repo) {
        return Err(CoreError::Message(format!(
            "invalid FLEETY_SKILLS_SYNC_REPO '{repo}' (expected owner/repo)"
        )));
    }
    let local_sha = std::fs::read_to_string(synced_dir.join(SHA_FILE))
        .ok()
        .map(|s| s.trim().to_string());
    let remote_sha = fetch_latest_sha(repo).await?;
    if !should_sync(&remote_sha, local_sha.as_deref()) {
        return Ok(false);
    }
    let bytes = download_zip(repo).await?;
    // Work + staging live next to synced_dir so the final swap is a same-fs rename.
    let parent = synced_dir
        .parent()
        .ok_or_else(|| CoreError::Message("synced dir has no parent".to_string()))?;
    std::fs::create_dir_all(parent).map_err(msg("create skills dir"))?;
    let work = parent.join(format!(".skill-sync-{}", uuid::Uuid::new_v4()));
    let result = stage_and_swap(&work, &bytes, synced_dir, &remote_sha);
    let _ = std::fs::remove_dir_all(&work);
    result?;
    Ok(true)
}

/// Extract the zip in `work`, rebuild the synced tier in a staging dir, and swap
/// it in. Isolated so the caller can always clean up `work` afterward.
fn stage_and_swap(work: &Path, bytes: &[u8], synced_dir: &Path, sha: &str) -> Result<()> {
    let extracted = work.join("extracted");
    extract_zip(bytes, &extracted)?;
    let repo_root = repo_root_in(&extracted)
        .ok_or_else(|| CoreError::Message("downloaded archive is empty".to_string()))?;
    let staging = work.join("staging");
    rebuild_into(&staging, &repo_root, sha)?;
    swap_into(synced_dir, &staging)
}

/// Spawn the background sync loop (server-side). Disabled by
/// `FLEETY_SKILLS_SYNC=0`. Syncs once immediately, then every
/// `FLEETY_SKILLS_SYNC_INTERVAL_SECS` (default 3600); the repo is
/// `FLEETY_SKILLS_SYNC_REPO` (default `TimLai666/skills`).
pub fn spawn(synced_dir: PathBuf) {
    if std::env::var("FLEETY_SKILLS_SYNC").as_deref() == Ok("0") {
        tracing::info!("FLEETY_SKILLS_SYNC=0; skill sync disabled");
        return;
    }
    let repo = std::env::var("FLEETY_SKILLS_SYNC_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let secs = std::env::var("FLEETY_SKILLS_SYNC_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    tracing::info!(%repo, interval_secs = secs, "skill sync enabled");
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(secs));
        loop {
            ticker.tick().await; // fires immediately on the first tick (boot sync)
            match sync_once(&synced_dir, &repo).await {
                Ok(true) => tracing::info!(%repo, "synced skills updated"),
                Ok(false) => tracing::debug!(%repo, "synced skills already up to date"),
                Err(e) => {
                    tracing::warn!(%repo, report = ?e.report(), "skill sync failed (kept previous copy)")
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-sksync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    fn write_skill(root: &Path, name: &str, body: &str) {
        let d = root.join(name);
        std::fs::create_dir_all(&d).expect("mk skill dir");
        std::fs::write(d.join("SKILL.md"), body).expect("write SKILL.md");
    }

    #[test]
    fn should_sync_table() {
        assert!(should_sync("abc123", None)); // none → sync
        assert!(!should_sync("abc123", Some("abc123"))); // same → skip
        assert!(should_sync("def456", Some("abc123"))); // differ → sync
    }

    #[test]
    fn skill_dirs_only_top_level_with_skill_md() {
        let root = temp();
        write_skill(&root, "a", "# A");
        write_skill(&root, "b", "# B");
        std::fs::create_dir_all(root.join("c")).expect("mk c"); // dir without SKILL.md
        std::fs::write(root.join("loose.md"), "x").expect("loose file");
        std::fs::write(root.join("script.py"), "y").expect("loose file");
        assert_eq!(
            skill_dirs_from_extracted(&root),
            vec!["a".to_string(), "b".to_string()]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rebuild_and_swap_mirrors_additions_and_removals() {
        let home = temp();
        let synced = home.join("synced");

        // First repo state: a + b.
        let repo1 = home.join("repo1");
        write_skill(&repo1, "a", "# A v1");
        write_skill(&repo1, "b", "# B");
        let staging1 = home.join(".stage1");
        rebuild_into(&staging1, &repo1, "sha1").expect("rebuild 1");
        swap_into(&synced, &staging1).expect("swap 1");
        assert!(synced.join("a").join("SKILL.md").is_file());
        assert!(synced.join("b").join("SKILL.md").is_file());
        assert_eq!(
            std::fs::read_to_string(synced.join(SHA_FILE)).unwrap_or_default(),
            "sha1"
        );

        // Second repo state: b removed, a updated, c added.
        let repo2 = home.join("repo2");
        write_skill(&repo2, "a", "# A v2");
        write_skill(&repo2, "c", "# C");
        let staging2 = home.join(".stage2");
        rebuild_into(&staging2, &repo2, "sha2").expect("rebuild 2");
        swap_into(&synced, &staging2).expect("swap 2");
        // Removal is inherent: b is gone; a updated; c added; sha updated.
        assert!(
            !synced.join("b").exists(),
            "removed-upstream skill should be gone"
        );
        assert!(synced.join("c").join("SKILL.md").is_file());
        assert_eq!(
            std::fs::read_to_string(synced.join("a").join("SKILL.md")).unwrap_or_default(),
            "# A v2"
        );
        assert_eq!(
            std::fs::read_to_string(synced.join(SHA_FILE)).unwrap_or_default(),
            "sha2"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn valid_repo_guards() {
        assert!(valid_repo("TimLai666/skills"));
        assert!(valid_repo("owner-1/repo.name_2"));
        assert!(!valid_repo("evil/../../etc"));
        assert!(!valid_repo("nogap"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("owner/repo;rm -rf"));
    }
}
