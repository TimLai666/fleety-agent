//! Runtime skill sync: keep the `synced` skill tier in step with an external
//! repo (default `TimLai666/skills`) without waiting for a Fleety release.
//!
//! A background task (spawned at boot, then every `FLEETY_SKILLS_SYNC_INTERVAL_SECS`)
//! first checks the repo's latest commit SHA; only when it differs from the
//! locally recorded one — or when the local tier holds no skills at all (fault
//! residue self-heals even under an unchanged SHA) — does it download the
//! branch zip, rebuild the synced tier
//! in a staging dir from the repo's skill directories, and atomically swap it in
//! — so additions/removals are mirrored and a partially-synced state is never
//! served. Skills are discovered by a pruned walk (the outermost directory with
//! a `SKILL.md` on each path; see [`skill_roots_from_extracted`]), which covers
//! flat repos and plugin-marketplace repos alike and keeps nested sub-skills
//! inside their parent. Any failure keeps the previous copy and logs a warning;
//! it never crashes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_core::{CoreError, Result};

const DEFAULT_REPO: &str = "TimLai666/skills";
const DEFAULT_INTERVAL_SECS: u64 = 3600;
const SHA_FILE: &str = ".synced-sha";

// ---- pure helpers (unit-tested) ----

/// Relative paths of the skill roots in an extracted skill repo, found by a
/// pruned top-down walk: dot-directories are skipped, and the first directory
/// along any path that contains a `SKILL.md` is a skill root — the walk does
/// not descend into it, so a `SKILL.md` nested deeper (a sub-skill shipped
/// inside a skill) stays part of that skill instead of becoming its own. The
/// repo root itself is never a skill; loose files at non-skill levels are
/// ignored. This covers the flat layout (top-level skill dirs) and plugin
/// marketplace layouts (plugins/<plugin>/skills/<skill>/) alike. Sorted by
/// relative path (which also decides who wins a duplicate-name collision).
pub fn skill_roots_from_extracted(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_skill_roots(root, Path::new(""), &mut out);
    out.sort();
    out
}

/// The recursive half of [`skill_roots_from_extracted`]: visit `dir` (at
/// repo-relative `rel`), record pruned skill roots into `out`.
fn walk_skill_roots(dir: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue; // dot-directories (.claude-plugin, .git, …) are never skills
        }
        let child_rel = rel.join(&name);
        if path.join("SKILL.md").is_file() {
            out.push(child_rel); // skill root — prune: don't descend into it
        } else {
            walk_skill_roots(&path, &child_rel, out);
        }
    }
}

/// Whether to download + apply: yes when there's no local SHA, when it
/// differs, or when the local tier holds no skills (the SHA short-circuit only
/// counts while the tier is non-empty, so an emptied tier self-heals).
pub fn should_sync(remote_sha: &str, local_sha: Option<&str>, local_has_skills: bool) -> bool {
    if !local_has_skills {
        return true; // empty tier: fault residue — always rebuild
    }
    match local_sha {
        Some(local) => local != remote_sha,
        None => true,
    }
}

/// Whether the synced tier currently holds at least one skill directory. A
/// missing dir, or one containing no subdirectories (e.g. only the SHA record
/// left behind by a fault), counts as empty.
pub fn synced_tier_has_skills(synced_dir: &Path) -> bool {
    std::fs::read_dir(synced_dir)
        .map(|entries| entries.flatten().any(|e| e.path().is_dir()))
        .unwrap_or(false)
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
/// discovered skill root (flattened by its directory name), then record the
/// SHA. Removals are inherent — only the repo's current skills end up in
/// staging. On a duplicate skill name the root earliest in path order wins;
/// the rest are skipped with a warning (the sync itself never fails on this).
fn rebuild_into(staging: &Path, repo_root: &Path, sha: &str) -> Result<()> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(msg("create staging"))?;
    for rel in skill_roots_from_extracted(repo_root) {
        let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let dst = staging.join(name);
        if dst.exists() {
            tracing::warn!(
                skill = name,
                path = %rel.display(),
                "duplicate skill name in repo; keeping the first in path order"
            );
            continue;
        }
        copy_dir_all(&repo_root.join(&rel), &dst).map_err(msg("copy skill"))?;
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
    if !should_sync(
        &remote_sha,
        local_sha.as_deref(),
        synced_tier_has_skills(synced_dir),
    ) {
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

    /// Write `body` at `rel` (creating parents) — for laying out nested repos.
    fn write_file(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mk parents");
        }
        std::fs::write(path, body).expect("write file");
    }

    /// The delta spec's "discovery across layouts" example table, row by row:
    /// flat skill found, plugin-layout skill found, nested SKILL.md pruned into
    /// its parent, dot-directory ignored, repo-root SKILL.md ignored.
    #[test]
    fn skill_roots_pruned_recursive_discovery() {
        let root = temp();
        write_skill(&root, "a", "# A");
        write_file(&root, "plugins/p1/skills/b/SKILL.md", "# B");
        write_file(&root, "plugins/p1/skills/b/sub/SKILL.md", "# sub of B");
        write_file(&root, ".claude-plugin/x/SKILL.md", "# hidden");
        write_file(&root, "SKILL.md", "# root is never a skill");
        std::fs::create_dir_all(root.join("plugins").join("p2")).expect("empty plugin");
        assert_eq!(
            skill_roots_from_extracted(&root),
            vec![
                PathBuf::from("a"),
                PathBuf::from("plugins").join("p1").join("skills").join("b"),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Plugin-layout skills flatten into the tier by directory name, and a
    /// nested sub-skill travels inside its parent at its original relative
    /// path instead of being split out as its own skill.
    #[test]
    fn rebuild_flattens_plugin_layout_and_keeps_nested_subskill() {
        let home = temp();
        let repo = home.join("repo");
        write_skill(&repo, "a", "# A");
        write_file(&repo, "plugins/p1/skills/b/SKILL.md", "# B");
        write_file(&repo, "plugins/p1/skills/b/sub/SKILL.md", "# sub of B");
        let staging = home.join("stage");
        rebuild_into(&staging, &repo, "sha1").expect("rebuild");
        assert!(staging.join("a").join("SKILL.md").is_file());
        assert!(staging.join("b").join("SKILL.md").is_file());
        assert!(
            staging.join("b").join("sub").join("SKILL.md").is_file(),
            "nested sub-skill must ship inside its parent"
        );
        assert!(
            !staging.join("sub").exists(),
            "nested sub-skill must not be split into its own skill"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Two skill roots with the same directory name: the one earliest in
    /// relative-path sort order wins, the sync does not fail.
    #[test]
    fn duplicate_skill_names_first_in_path_order_wins() {
        let home = temp();
        let repo = home.join("repo");
        write_file(&repo, "plugins/p1/skills/dup/SKILL.md", "# from p1");
        write_file(&repo, "plugins/p2/skills/dup/SKILL.md", "# from p2");
        let staging = home.join("stage");
        rebuild_into(&staging, &repo, "sha1").expect("rebuild");
        assert_eq!(
            std::fs::read_to_string(staging.join("dup").join("SKILL.md")).unwrap_or_default(),
            "# from p1",
            "earliest path order (p1 < p2) must win"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The delta spec's "sync decision" example table, row by row.
    #[test]
    fn should_sync_table() {
        assert!(should_sync("abc123", None, true)); // no local SHA → sync
        assert!(!should_sync("abc123", Some("abc123"), true)); // same + has skills → skip
        assert!(should_sync("def456", Some("abc123"), true)); // differ → sync
                                                              // Same SHA but the tier is empty (fault residue) → sync (self-heal).
        assert!(should_sync("abc123", Some("abc123"), false));
    }

    /// Empty-tier detection: missing dir and a dir holding only the SHA record
    /// are empty; any subdirectory (a skill) makes it non-empty.
    #[test]
    fn synced_tier_emptiness_check() {
        let home = temp();
        let synced = home.join("synced");
        assert!(!synced_tier_has_skills(&synced), "missing dir is empty");
        std::fs::create_dir_all(&synced).expect("mk synced");
        std::fs::write(synced.join(SHA_FILE), "abc123").expect("sha file");
        assert!(
            !synced_tier_has_skills(&synced),
            "only the SHA record left behind is still empty"
        );
        write_skill(&synced, "a", "# A");
        assert!(
            synced_tier_has_skills(&synced),
            "a skill dir makes it non-empty"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Flat-layout regression: top-level skill dirs are found exactly as
    /// before, and loose files at the root are not skills.
    #[test]
    fn flat_layout_discovery_unchanged() {
        let root = temp();
        write_skill(&root, "a", "# A");
        write_skill(&root, "b", "# B");
        std::fs::create_dir_all(root.join("c")).expect("mk c"); // dir without SKILL.md
        std::fs::write(root.join("loose.md"), "x").expect("loose file");
        std::fs::write(root.join("script.py"), "y").expect("loose file");
        assert_eq!(
            skill_roots_from_extracted(&root),
            vec![PathBuf::from("a"), PathBuf::from("b")]
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
