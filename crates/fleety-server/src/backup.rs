//! Auto-backup of the server's non-regenerable state to a **user-configured**
//! private GitHub repo, plus restore.
//!
//! The truth of a Fleet lives on the server host. This module keeps a local git
//! mirror (`<agent_home>/../backup-mirror`) of the state that can't be
//! regenerated — conversations, memory, wiki, devices, sites, schedules, the
//! installed/authored skill tiers, auth/mcp/cookies, plus `config.toml` and
//! `providers.toml` (which carry model API keys, in cleartext, per the user's
//! explicit choice) — and, on a schedule or on demand, commits and pushes it.
//!
//! Design choices worth stating up front:
//! - **Nothing is hardcoded.** The repo and token come from the environment
//!   (`FLEETY_BACKUP_REPO`, `FLEETY_BACKUP_TOKEN`). With no repo set, the whole
//!   feature is inert: no loop, no mirror.
//! - **Exclude-set, not include-list.** We copy the agent home *minus* a small
//!   set of regenerable/oversized paths (downloaded models, the builtin & synced
//!   skill tiers, the rollback backups store), so new state dirs are picked up
//!   automatically instead of being silently missed.
//! - **Refuse to push to a non-private repo.** Before pushing cleartext secrets
//!   we confirm via the GitHub API that the repo is private; if it isn't (or we
//!   can't tell), we keep the local commit and refuse to push.
//! - **Never crash.** Any copy/git/network/API failure is surfaced as an error
//!   the caller logs as a warning; the previous local mirror is left intact.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_core::{CoreError, Result};

/// Default interval between scheduled backups (1h), when
/// `FLEETY_BACKUP_INTERVAL_SECS` is unset/invalid.
const DEFAULT_INTERVAL_SECS: u64 = 3600;

/// Top-level (relative to the agent home) paths excluded from backup: they are
/// either regenerated at boot (skills builtin/synced), re-downloadable (models),
/// or a large local-only store (the rollback backups). Matched as path prefixes.
const EXCLUDED: &[&[&str]] = &[
    &["models"],
    &["skills", "builtin"],
    &["skills", "synced"],
    &["fleet", "backups"],
];

/// Outcome of a single backup attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupOutcome {
    /// State unchanged since the last backup — no commit, no push.
    NothingChanged,
    /// New snapshot committed and pushed to the (verified-private) repo.
    Pushed,
    /// New snapshot committed locally, but the repo is not private (or its
    /// visibility couldn't be determined) so nothing was pushed.
    RefusedNotPrivate,
}

/// The backup settings resolved from the environment.
pub struct BackupEnv {
    pub repo: String,
    pub token: String,
    pub interval: u64,
}

/// Read backup settings from the environment. Returns `None` when
/// `FLEETY_BACKUP_REPO` is unset/blank — the signal that backup is disabled.
pub fn config_from_env() -> Option<BackupEnv> {
    let repo = std::env::var("FLEETY_BACKUP_REPO")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    let token = std::env::var("FLEETY_BACKUP_TOKEN").unwrap_or_default();
    let interval = std::env::var("FLEETY_BACKUP_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    Some(BackupEnv {
        repo,
        token,
        interval,
    })
}

// ---------------------------------------------------------------------------
// Pure functions (unit-tested; no I/O, no network)
// ---------------------------------------------------------------------------

/// Whether a path (relative to the agent home) is excluded from backup. Matches
/// any [`EXCLUDED`] prefix component-wise, so `models` excludes `models/x` but
/// not a sibling like `models-of-something`.
pub fn is_excluded(rel_under_home: &Path) -> bool {
    let comps: Vec<&str> = rel_under_home
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    EXCLUDED
        .iter()
        .any(|prefix| comps.len() >= prefix.len() && &comps[..prefix.len()] == *prefix)
}

fn is_repo_seg_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn is_repo_path_char(c: char) -> bool {
    is_repo_seg_char(c) || c == '/'
}

/// Build the tokenized push URL for a user-configured repo. Accepts either
/// `owner/repo` or a full `https://github.com/owner/repo[.git]` URL. Rejects
/// anything else (empty, whitespace, `@`, odd shapes) so a malformed value can't
/// break out of the URL we construct. **The token never appears in the error.**
pub fn remote_url(repo: &str, token: &str) -> Result<String> {
    let repo = repo.trim();
    let malformed = || CoreError::Message(format!("invalid FLEETY_BACKUP_REPO '{repo}'"));
    if repo.is_empty() || repo.contains(char::is_whitespace) || repo.contains('@') {
        return Err(malformed());
    }
    if let Some(rest) = repo.strip_prefix("https://github.com/") {
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() || !rest.chars().all(is_repo_path_char) {
            return Err(malformed());
        }
        return Ok(format!("https://x-access-token:{token}@github.com/{rest}"));
    }
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(is_repo_seg_char))
    {
        return Err(malformed());
    }
    Ok(format!(
        "https://x-access-token:{token}@github.com/{}/{}.git",
        parts[0], parts[1]
    ))
}

/// The `owner/repo` slug for the GitHub API, derived from either accepted repo
/// form (drops the `https://github.com/` prefix and any trailing `.git`).
pub fn repo_slug(repo: &str) -> String {
    let repo = repo.trim();
    let slug = repo
        .strip_prefix("https://github.com/")
        .unwrap_or(repo)
        .trim_end_matches('/');
    slug.strip_suffix(".git").unwrap_or(slug).to_string()
}

/// Whether the GitHub API repo JSON reports the repo as private. Missing field
/// or unparseable body → `false` (treated as "not confirmed private").
pub fn is_private_repo(api_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(api_json)
        .ok()
        .and_then(|v| v.get("private").and_then(|p| p.as_bool()))
        .unwrap_or(false)
}

/// The preserved (kept, not deleted) path for `orig` before a restore overwrites
/// it: `orig` with a `.pre-restore-<ts>` suffix appended to the whole path.
pub fn pre_restore_path(orig: &Path, ts: &str) -> PathBuf {
    let mut s = orig.as_os_str().to_os_string();
    s.push(format!(".pre-restore-{ts}"));
    PathBuf::from(s)
}

/// The commit message stamped on each backup snapshot.
pub fn commit_message(ts: &str) -> String {
    format!("fleety backup {ts}")
}

/// A filesystem timestamp string (local time) for commit messages / pre-restore
/// suffixes. Not used inside pure functions (kept out so those stay testable).
fn now_ts() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn redact(s: &str, token: &str) -> String {
    if token.is_empty() {
        s.to_string()
    } else {
        s.replace(token, "***")
    }
}

// ---------------------------------------------------------------------------
// Filesystem: build the mirror working tree
// ---------------------------------------------------------------------------

fn io_err(ctx: &str, e: std::io::Error) -> CoreError {
    CoreError::Message(format!("{ctx}: {e}"))
}

/// Recursively copy `src` into `dst_root`, preserving paths relative to
/// `src_root`. When `apply_exclude`, paths matching [`is_excluded`] are skipped.
fn copy_tree(src: &Path, src_root: &Path, dst_root: &Path, apply_exclude: bool) -> Result<()> {
    for entry in std::fs::read_dir(src).map_err(|e| io_err("read backup source dir", e))? {
        let entry = entry.map_err(|e| io_err("read backup source entry", e))?;
        let path = entry.path();
        let rel = match path.strip_prefix(src_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if apply_exclude && is_excluded(rel) {
            continue;
        }
        let target = dst_root.join(rel);
        if path.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| io_err("create mirror dir", e))?;
            copy_tree(&path, src_root, dst_root, apply_exclude)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io_err("create mirror dir", e))?;
            }
            std::fs::copy(&path, &target).map_err(|e| io_err("copy into mirror", e))?;
        }
    }
    Ok(())
}

/// Remove everything under `dir` except its `.git` directory. Used to rebuild
/// the mirror working tree from scratch each backup so deletions in the source
/// propagate to git (via `add -A`).
fn clear_worktree(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| io_err("read mirror dir", e))? {
        let entry = entry.map_err(|e| io_err("read mirror entry", e))?;
        let path = entry.path();
        if path.file_name().and_then(|s| s.to_str()) == Some(".git") {
            continue;
        }
        let res = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        res.map_err(|e| io_err("clear mirror worktree", e))?;
    }
    Ok(())
}

/// Rebuild the mirror working tree: wipe it (keeping `.git`), copy the agent
/// home minus the exclude-set, then drop `config.toml` / `providers.toml` at the
/// mirror root. Pure filesystem work — no git, no network (unit-tested).
pub fn build_mirror(
    agent_home: &Path,
    config_path: &Path,
    providers_path: &Path,
    mirror: &Path,
) -> Result<()> {
    std::fs::create_dir_all(mirror).map_err(|e| io_err("create mirror", e))?;
    clear_worktree(mirror)?;
    if agent_home.is_dir() {
        copy_tree(agent_home, agent_home, mirror, true)?;
    }
    if config_path.is_file() {
        std::fs::copy(config_path, mirror.join("config.toml"))
            .map_err(|e| io_err("copy config.toml into mirror", e))?;
    }
    if providers_path.is_file() {
        std::fs::copy(providers_path, mirror.join("providers.toml"))
            .map_err(|e| io_err("copy providers.toml into mirror", e))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Git (async, via the system `git` — an existing server assumption)
// ---------------------------------------------------------------------------

async fn run_git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| CoreError::Message(format!("cannot run git (is it installed?): {e}")))
}

/// Run a git command, erroring on non-zero exit. `token` (if non-empty) is
/// redacted from any error text so the PAT never lands in a log/error.
async fn run_git_ok(dir: &Path, args: &[&str], token: &str) -> Result<()> {
    let out = run_git(dir, args).await?;
    if !out.status.success() {
        let verb = args.first().copied().unwrap_or("git");
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CoreError::Message(format!(
            "git {verb} failed: {}",
            redact(stderr.trim(), token)
        )));
    }
    Ok(())
}

/// Ensure `dir` is a git repo with a commit identity (so `commit` works without
/// relying on a global git config).
async fn ensure_git(dir: &Path) -> Result<()> {
    if !dir.join(".git").exists() {
        run_git_ok(dir, &["init", "-q"], "").await?;
        run_git_ok(dir, &["config", "user.email", "fleety@localhost"], "").await?;
        run_git_ok(dir, &["config", "user.name", "Fleety"], "").await?;
    }
    Ok(())
}

/// Point `origin` at `url` (add if missing, else set-url). `url` carries the
/// token, so failures are redacted.
async fn set_remote(dir: &Path, url: &str, token: &str) -> Result<()> {
    let remotes = run_git(dir, &["remote"]).await?;
    let has_origin = String::from_utf8_lossy(&remotes.stdout)
        .lines()
        .any(|l| l.trim() == "origin");
    let args: Vec<&str> = if has_origin {
        vec!["remote", "set-url", "origin", url]
    } else {
        vec!["remote", "add", "origin", url]
    };
    run_git_ok(dir, &args, token).await
}

/// Stage everything and commit if there's a change. Returns whether a new commit
/// was made (`false` = nothing changed, git no-op). No network — testable with
/// real git.
pub async fn commit_mirror(mirror: &Path) -> Result<bool> {
    ensure_git(mirror).await?;
    run_git_ok(mirror, &["add", "-A"], "").await?;
    let status = run_git(mirror, &["status", "--porcelain"]).await?;
    if status.stdout.is_empty() {
        return Ok(false);
    }
    run_git_ok(
        mirror,
        &["commit", "-q", "-m", &commit_message(&now_ts())],
        "",
    )
    .await?;
    Ok(true)
}

/// GET `api.github.com/repos/<slug>` and report whether the repo is private.
/// Any failure (network, non-JSON, missing field) → `false`: we only push when
/// we can positively confirm the repo is private.
async fn check_private(slug: &str, token: &str) -> bool {
    let client = reqwest::Client::new();
    let mut req = client
        .get(format!("https://api.github.com/repos/{slug}"))
        .header("User-Agent", "fleety")
        .header("Accept", "application/vnd.github+json");
    if !token.is_empty() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    match req.send().await {
        Ok(resp) => match resp.text().await {
            Ok(body) => is_private_repo(&body),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// Run one backup: rebuild the mirror, commit if changed, verify the repo is
/// private, then push. Returns the [`BackupOutcome`]; any failure is an `Err`
/// the caller logs as a warning (the local mirror is left intact).
pub async fn run_backup(
    agent_home: &Path,
    config_path: &Path,
    providers_path: &Path,
    mirror: &Path,
    repo: &str,
    token: &str,
) -> Result<BackupOutcome> {
    // Validate the repo shape up front so a bad value fails before any work.
    let url = remote_url(repo, token)?;
    build_mirror(agent_home, config_path, providers_path, mirror)?;
    let committed = commit_mirror(mirror).await?;
    set_remote(mirror, &url, token).await?;
    if !committed {
        return Ok(BackupOutcome::NothingChanged);
    }
    if !check_private(&repo_slug(repo), token).await {
        return Ok(BackupOutcome::RefusedNotPrivate);
    }
    run_git_ok(mirror, &["push", "origin", "HEAD:main"], token).await?;
    Ok(BackupOutcome::Pushed)
}

/// Spawn the background backup loop, if a repo is configured. Ticks immediately
/// at boot, then every `interval`. Each tick's outcome/error is logged; the loop
/// never crashes the server.
pub fn spawn(agent_home: PathBuf, config_path: PathBuf, providers_path: PathBuf, mirror: PathBuf) {
    let env = match config_from_env() {
        Some(e) => e,
        None => {
            tracing::info!("FLEETY_BACKUP_REPO not set; auto-backup disabled");
            return;
        }
    };
    tracing::info!(repo = %env.repo, interval_secs = env.interval, "auto-backup enabled");
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(env.interval));
        loop {
            ticker.tick().await; // fires immediately on the first tick (boot backup)
            match run_backup(
                &agent_home,
                &config_path,
                &providers_path,
                &mirror,
                &env.repo,
                &env.token,
            )
            .await
            {
                Ok(BackupOutcome::Pushed) => tracing::info!(repo = %env.repo, "backup pushed"),
                Ok(BackupOutcome::NothingChanged) => {
                    tracing::debug!(repo = %env.repo, "backup: nothing changed")
                }
                Ok(BackupOutcome::RefusedNotPrivate) => tracing::warn!(
                    repo = %env.repo,
                    "backup refused: repo is not confirmed private; kept local mirror"
                ),
                Err(e) => tracing::warn!(
                    repo = %env.repo,
                    report = ?e.report(),
                    "backup failed; kept previous local mirror"
                ),
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

/// Move a path aside to its `.pre-restore-<ts>` sibling if it exists (kept, not
/// deleted). A missing source is a no-op.
fn preserve(orig: &Path, ts: &str) -> Result<()> {
    if orig.exists() {
        let kept = pre_restore_path(orig, ts);
        std::fs::rename(orig, &kept).map_err(|e| io_err("preserve existing data", e))?;
    }
    Ok(())
}

/// Put a cloned backup (`cloned` = the mirror layout: agent-home tree at the
/// root plus `config.toml`/`providers.toml`) into place. Existing agent home and
/// config files are first renamed to `.pre-restore-<ts>` (kept). No network —
/// unit-tested.
pub fn restore_from_dir(
    cloned: &Path,
    agent_home: &Path,
    config_path: &Path,
    providers_path: &Path,
    ts: &str,
) -> Result<()> {
    // Preserve existing data before overwriting anything.
    preserve(agent_home, ts)?;
    preserve(config_path, ts)?;
    preserve(providers_path, ts)?;

    // Agent home = everything in the clone except .git and the two config files.
    std::fs::create_dir_all(agent_home).map_err(|e| io_err("create restored agent home", e))?;
    for entry in std::fs::read_dir(cloned).map_err(|e| io_err("read clone dir", e))? {
        let entry = entry.map_err(|e| io_err("read clone entry", e))?;
        let name = entry.file_name();
        match name.to_str() {
            Some(".git") | Some("config.toml") | Some("providers.toml") => continue,
            _ => {}
        }
        let src = entry.path();
        let dst = agent_home.join(&name);
        if src.is_dir() {
            copy_tree(&src, &src, &dst, false)?;
        } else if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_err("create restore dir", e))?;
            std::fs::copy(&src, &dst).map_err(|e| io_err("restore file", e))?;
        }
    }
    // The two config files live above the agent home.
    let cfg = cloned.join("config.toml");
    if cfg.is_file() {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_err("create config dir", e))?;
        }
        std::fs::copy(&cfg, config_path).map_err(|e| io_err("restore config.toml", e))?;
    }
    let prov = cloned.join("providers.toml");
    if prov.is_file() {
        if let Some(parent) = providers_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_err("create providers dir", e))?;
        }
        std::fs::copy(&prov, providers_path).map_err(|e| io_err("restore providers.toml", e))?;
    }
    Ok(())
}

/// Clone the configured repo and restore it into place, preserving existing
/// data first. Clone happens *before* anything existing is touched, so a clone
/// failure leaves the host untouched. Run with the server stopped.
pub async fn restore(
    agent_home: &Path,
    config_path: &Path,
    providers_path: &Path,
    repo: &str,
    token: &str,
    ts: &str,
) -> Result<()> {
    let url = remote_url(repo, token)?;
    let tmp = std::env::temp_dir().join(format!("fleety-restore-{ts}"));
    let _ = std::fs::remove_dir_all(&tmp);
    // Clone first; if this fails we return before touching existing data.
    let out = tokio::process::Command::new("git")
        .arg("clone")
        .arg("-q")
        .arg(&url)
        .arg(&tmp)
        .output()
        .await
        .map_err(|e| CoreError::Message(format!("cannot run git (is it installed?): {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(CoreError::Message(format!(
            "backup clone failed: {}",
            redact(stderr.trim(), token)
        )));
    }
    let res = restore_from_dir(&tmp, agent_home, config_path, providers_path, ts);
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- pure functions (task 1.1) ---------------------------------------

    #[test]
    fn is_excluded_covers_regenerable_paths() {
        // Excluded (regenerable / oversized).
        assert!(is_excluded(Path::new("models/gemma/model.onnx")));
        assert!(is_excluded(Path::new("skills/builtin/insyra/SKILL.md")));
        assert!(is_excluded(Path::new("skills/synced/foo/SKILL.md")));
        assert!(is_excluded(Path::new("fleet/backups/2026/x.json")));
        // Kept (non-regenerable).
        assert!(!is_excluded(Path::new("wiki/note.md")));
        assert!(!is_excluded(Path::new("fleet/grants.json")));
        assert!(!is_excluded(Path::new("skills/installed/mine/SKILL.md")));
    }

    #[test]
    fn remote_url_owner_repo_and_rejects_bad_shapes() {
        assert_eq!(
            remote_url("owner/repo", "TOKENVAL").unwrap(),
            "https://x-access-token:TOKENVAL@github.com/owner/repo.git"
        );
        // Full URL form injects the token too.
        assert_eq!(
            remote_url("https://github.com/o/r.git", "TOKENVAL").unwrap(),
            "https://x-access-token:TOKENVAL@github.com/o/r.git"
        );
        // Illegal shapes error, and the token never leaks into the message.
        for bad in ["owner/repo/extra", "owner repo", "owner@repo", ""] {
            let err = remote_url(bad, "TOKENVAL").unwrap_err();
            let msg = format!("{:?}", err.report());
            assert!(!msg.contains("TOKENVAL"), "token leaked for {bad:?}: {msg}");
        }
    }

    #[test]
    fn is_private_repo_matches_visibility_table() {
        // Mirrors the spec example table.
        assert!(is_private_repo(r#"{"private": true}"#));
        assert!(!is_private_repo(r#"{"private": false}"#));
        assert!(!is_private_repo(r#"{"name": "r"}"#)); // missing
        assert!(!is_private_repo("not json")); // error
    }

    #[test]
    fn pre_restore_path_appends_suffix() {
        let p = pre_restore_path(Path::new("/x/agent"), "20260702-120000");
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("agent.pre-restore-20260702-120000")
        );
    }

    #[test]
    fn commit_message_carries_timestamp() {
        assert!(commit_message("20260702-120000").contains("20260702-120000"));
    }

    // --- backup copy + git no-op (task 2.1) ------------------------------

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fleety-{tag}-{}", uuid::Uuid::new_v4()))
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn build_mirror_excludes_and_commit_is_incremental() {
        let home = temp_dir("home");
        // Kept:
        write(&home.join("wiki").join("note.md"), "hi");
        write(&home.join("fleet").join("grants.json"), "{}");
        write(&home.join("skills").join("installed").join("s.md"), "x");
        // Excluded:
        write(&home.join("models").join("big.bin"), "0000");
        write(&home.join("skills").join("builtin").join("b.md"), "b");
        write(&home.join("skills").join("synced").join("y.md"), "y");
        write(&home.join("fleet").join("backups").join("r.json"), "{}");

        let cfg = temp_dir("cfg").join("config.toml");
        write(&cfg, "addr=1");
        let prov = temp_dir("prov").join("providers.toml");
        write(&prov, "key=secret");
        let mirror = temp_dir("mirror");

        build_mirror(&home, &cfg, &prov, &mirror).unwrap();

        // Kept + config files present.
        assert!(mirror.join("wiki").join("note.md").is_file());
        assert!(mirror.join("fleet").join("grants.json").is_file());
        assert!(mirror
            .join("skills")
            .join("installed")
            .join("s.md")
            .is_file());
        assert!(mirror.join("config.toml").is_file());
        assert!(mirror.join("providers.toml").is_file());
        // Excluded absent.
        assert!(!mirror.join("models").exists());
        assert!(!mirror.join("skills").join("builtin").exists());
        assert!(!mirror.join("skills").join("synced").exists());
        assert!(!mirror.join("fleet").join("backups").exists());

        // First commit makes a new commit; unchanged state is a no-op.
        assert!(commit_mirror(&mirror).await.unwrap(), "first commit");
        build_mirror(&home, &cfg, &prov, &mirror).unwrap();
        assert!(
            !commit_mirror(&mirror).await.unwrap(),
            "unchanged state must not re-commit"
        );

        for d in [
            &home,
            &mirror,
            cfg.parent().unwrap(),
            prov.parent().unwrap(),
        ] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    // --- scheduling gate (task 3.1) --------------------------------------

    #[test]
    #[serial_test::serial]
    fn config_from_env_disabled_without_repo() {
        std::env::remove_var("FLEETY_BACKUP_REPO");
        assert!(config_from_env().is_none());
        std::env::set_var("FLEETY_BACKUP_REPO", "owner/repo");
        std::env::remove_var("FLEETY_BACKUP_INTERVAL_SECS");
        let env = config_from_env().expect("configured");
        assert_eq!(env.repo, "owner/repo");
        assert_eq!(env.interval, DEFAULT_INTERVAL_SECS);
        std::env::remove_var("FLEETY_BACKUP_REPO");
    }

    // --- restore preserves existing data (task 4.1) ----------------------

    #[test]
    fn restore_from_dir_preserves_then_places() {
        let base = temp_dir("restore");
        let home = base.join("agent");
        let cfg = base.join("config.toml");
        let prov = base.join("providers.toml");
        // Existing state that must be preserved, not destroyed.
        write(&home.join("old.txt"), "OLD");
        write(&cfg, "OLDCFG");
        write(&prov, "OLDPROV");
        // A "cloned" backup to place.
        let cloned = base.join("clone");
        write(&cloned.join("wiki").join("note.md"), "NEW");
        write(&cloned.join("config.toml"), "NEWCFG");
        write(&cloned.join("providers.toml"), "NEWPROV");

        restore_from_dir(&cloned, &home, &cfg, &prov, "TS").unwrap();

        // Old data kept under .pre-restore-TS.
        assert!(pre_restore_path(&home, "TS").join("old.txt").is_file());
        assert_eq!(
            std::fs::read_to_string(pre_restore_path(&cfg, "TS")).unwrap(),
            "OLDCFG"
        );
        // New data in place.
        assert_eq!(
            std::fs::read_to_string(home.join("wiki").join("note.md")).unwrap(),
            "NEW"
        );
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "NEWCFG");
        assert_eq!(std::fs::read_to_string(&prov).unwrap(), "NEWPROV");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn restore_leaves_home_untouched_when_repo_invalid() {
        let base = temp_dir("restore-bad");
        let home = base.join("agent");
        let cfg = base.join("config.toml");
        let prov = base.join("providers.toml");
        write(&home.join("keep.txt"), "KEEP");

        // Invalid repo shape → remote_url errors before any clone/rename.
        let err = restore(&home, &cfg, &prov, "bad repo shape", "TOK", "TS").await;
        assert!(err.is_err());
        // Nothing preserved (rename never happened) and existing home intact.
        assert!(!pre_restore_path(&home, "TS").exists());
        assert_eq!(
            std::fs::read_to_string(home.join("keep.txt")).unwrap(),
            "KEEP"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
