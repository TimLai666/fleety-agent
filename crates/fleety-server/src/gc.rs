//! Periodic retention / GC of audit-trail surfaces that otherwise grow
//! unbounded:
//!
//! - `backups/<uuid>/…` — every mutate tool drops a copy; pruned by mtime
//! - `fleet/devices/<id>/history.jsonl` — rotated when it exceeds a size cap
//!
//! Conversation logs (`conversations/<id>.jsonl`) are NOT touched: rollback,
//! resume, and reconstruction all depend on the full chronology, and a single
//! conversation is rarely large compared to a long-running history file.
//!
//! Tunables (env, with sensible defaults):
//!
//! | env var | default | meaning |
//! |---|---|---|
//! | `FLEETY_GC_INTERVAL_SECS` | 6 h | how often to run a sweep |
//! | `FLEETY_BACKUPS_RETENTION_SECS` | 7 d | drop backups older than this |
//! | `FLEETY_HISTORY_ROTATE_BYTES` | 32 MiB | rotate `history.jsonl` past this size |
//! | `FLEETY_GC_DISABLED` | (unset) | set to anything to skip the loop entirely |

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_core::Result;

use crate::storage::Storage;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const DEFAULT_BACKUP_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DEFAULT_HISTORY_ROTATE_BYTES: u64 = 32 * 1024 * 1024;
const MIN_INTERVAL: Duration = Duration::from_secs(60);

fn env_duration_secs(key: &str, default: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
        .max(MIN_INTERVAL)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(default)
}

fn disabled() -> bool {
    std::env::var("FLEETY_GC_DISABLED").is_ok()
}

/// Number of backups deleted + number of history files rotated, per sweep.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub backups_deleted: u64,
    pub histories_rotated: u64,
}

/// Spawn the retention loop. No-op when `FLEETY_GC_DISABLED` is set.
pub fn spawn(storage: Arc<Storage>) {
    if disabled() {
        tracing::info!("FLEETY_GC_DISABLED set; retention loop will not run");
        return;
    }
    let interval = env_duration_secs("FLEETY_GC_INTERVAL_SECS", DEFAULT_INTERVAL);
    let backup_retention =
        env_duration_secs("FLEETY_BACKUPS_RETENTION_SECS", DEFAULT_BACKUP_RETENTION);
    let rotate_bytes = env_u64("FLEETY_HISTORY_ROTATE_BYTES", DEFAULT_HISTORY_ROTATE_BYTES);
    tracing::info!(
        interval_secs = interval.as_secs(),
        backup_retention_secs = backup_retention.as_secs(),
        rotate_bytes,
        "retention loop enabled"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            match run_sweep(&storage, backup_retention, rotate_bytes) {
                Ok(r) if r.backups_deleted > 0 || r.histories_rotated > 0 => {
                    tracing::info!(
                        backups_deleted = r.backups_deleted,
                        histories_rotated = r.histories_rotated,
                        "retention sweep finished"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(report = ?e.report(), "retention sweep failed (isolated)"),
            }
        }
    });
}

/// Run one synchronous sweep. Public so tests can drive it without spawning.
pub fn run_sweep(
    storage: &Storage,
    backup_retention: Duration,
    rotate_bytes: u64,
) -> Result<GcReport> {
    Ok(GcReport {
        backups_deleted: gc_backups(&storage.backups_dir(), backup_retention)?,
        histories_rotated: rotate_histories(&storage.devices_dir(), rotate_bytes)?,
    })
}

/// Walk `backups_dir/<uuid>` entries; delete those whose mtime is older than
/// `retention`. Each backup is a self-contained directory tree from the
/// fleety-tools `backup_existing` writer.
fn gc_backups(backups_dir: &Path, retention: Duration) -> Result<u64> {
    let entries = match std::fs::read_dir(backups_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(agent_core::CoreError::Message(format!(
                "read backups dir: {e}"
            )))
        }
    };
    let now = SystemTime::now();
    let mut deleted = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(now);
        let Ok(age) = now.duration_since(mtime) else {
            continue;
        };
        if age > retention && std::fs::remove_dir_all(&path).is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// For each device, if `history.jsonl` exceeds `rotate_bytes`, rename it to
/// `history.jsonl.<unix_ts>` (an archive a human can still grep) and let the
/// next append create a fresh file. We never delete archives — operators can
/// decide how to handle them; this just keeps the *live* file small enough to
/// scan for `audit list`.
fn rotate_histories(devices_dir: &Path, rotate_bytes: u64) -> Result<u64> {
    let entries = match std::fs::read_dir(devices_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(agent_core::CoreError::Message(format!(
                "read devices dir: {e}"
            )))
        }
    };
    let mut rotated = 0;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for entry in entries.flatten() {
        let history = entry.path().join("history.jsonl");
        let Ok(meta) = std::fs::metadata(&history) else {
            continue;
        };
        if !meta.is_file() || meta.len() <= rotate_bytes {
            continue;
        }
        let archive = entry.path().join(format!("history.jsonl.{now}"));
        if std::fs::rename(&history, &archive).is_ok() {
            rotated += 1;
        }
    }
    Ok(rotated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn temp_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fleety-gc-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn gc_backups_drops_only_aged_dirs() {
        let dir = temp_home();
        fs::create_dir_all(&dir).expect("mk");
        let old = dir.join("old");
        let fresh = dir.join("fresh");
        fs::create_dir_all(&old).expect("old");
        fs::create_dir_all(&fresh).expect("fresh");
        // Backdate the `old` dir 30 days into the past.
        let thirty_days = SystemTime::now() - Duration::from_secs(30 * 86_400);
        let _ = filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(thirty_days));

        let deleted = gc_backups(&dir, Duration::from_secs(7 * 86_400)).expect("gc");
        assert_eq!(deleted, 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_histories_archives_when_oversize() {
        let home = temp_home();
        let devices = home.join("devices");
        let dev = devices.join("d1");
        fs::create_dir_all(&dev).expect("mk dev");
        let history = dev.join("history.jsonl");
        // Write ~2 KiB so we can rotate with a 1 KiB threshold.
        let mut f = fs::File::create(&history).expect("touch");
        let line = "x".repeat(64);
        for _ in 0..40 {
            writeln!(f, "{line}").expect("write");
        }
        drop(f);
        assert!(fs::metadata(&history).unwrap().len() > 1024);

        let rotated = rotate_histories(&devices, 1024).expect("rotate");
        assert_eq!(rotated, 1);
        // The live file no longer exists; an archive does.
        assert!(!history.exists());
        let archives: Vec<_> = fs::read_dir(&dev)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("history.jsonl.")
            })
            .collect();
        assert_eq!(archives.len(), 1);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn disabled_short_circuits_spawn() {
        // We don't spawn here (would race the test runtime); just verify the
        // env-var guard reads as "disabled".
        std::env::set_var("FLEETY_GC_DISABLED", "1");
        assert!(disabled());
        std::env::remove_var("FLEETY_GC_DISABLED");
        assert!(!disabled());
    }
}
