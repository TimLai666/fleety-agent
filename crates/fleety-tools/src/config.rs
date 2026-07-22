//! Typed, file-backed configuration shared by the server, daemon, and CLI.
//!
//! A curated registry of known settings (each a `FLEETY_*` name) persists to
//! `~/.fleety/config.toml`, sectioned by scope. Read precedence is **env → config
//! → default**: an explicit environment variable always wins, so existing
//! env-based deployments are unaffected; config.toml only fills what env leaves
//! unset. The CLI edits this; the server/daemon seed their env from it at boot.
//!
//! A registry entry may carry a `validator`; `config set` and the interactive
//! editors check the value against it before writing, rejecting out-of-domain
//! values (bad enums, non-boolean `0|1`, non-numeric, non-`http(s)` URLs) so a
//! typo never lands silently in `config.toml`. Keys with no validator accept
//! any value (pass-through).

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_core::{CoreError, Result};
use clap::{Arg, ArgAction, Command as ClapCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Server,
    Daemon,
    Cli,
    Shared,
}

/// Runtime that owns persistence for a setting. Shared settings are daemon
/// owned because they affect the local device processes, never the server or
/// the invoking CLI process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Server,
    Daemon,
    Cli,
}

pub const SERVER_SCOPES: &[Scope] = &[Scope::Server];
pub const DAEMON_SCOPES: &[Scope] = &[Scope::Daemon, Scope::Shared];
pub const CLI_SCOPES: &[Scope] = &[Scope::Cli];

pub fn owner_for_scope(scope: Scope) -> Owner {
    match scope {
        Scope::Server => Owner::Server,
        Scope::Daemon | Scope::Shared => Owner::Daemon,
        Scope::Cli => Owner::Cli,
    }
}

pub fn owner_for_key(key: &str) -> Result<Owner> {
    if key == "FLEETY_DEVICE_ID" {
        return Err(CoreError::Message(
            "FLEETY_DEVICE_ID is stable device identity managed through connections.toml, not config.toml"
                .to_string(),
        ));
    }
    find(key)
        .map(|setting| owner_for_scope(setting.scope))
        .ok_or_else(|| CoreError::Message(format!("unknown setting '{key}'")))
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Server => "server",
            Scope::Daemon => "daemon",
            Scope::Cli => "cli",
            Scope::Shared => "shared",
        }
    }

    fn from_str(s: &str) -> Option<Scope> {
        match s {
            "server" => Some(Scope::Server),
            "daemon" => Some(Scope::Daemon),
            "cli" => Some(Scope::Cli),
            "shared" => Some(Scope::Shared),
            _ => None,
        }
    }
}

/// A value validator: rejects an out-of-domain write, returning a short
/// description of the accepted values (no key name — [`validate`] prepends it).
/// A bare `fn` pointer (not a closure) so [`Setting`] stays `Copy`.
pub type Validator = fn(&str) -> std::result::Result<(), String>;

/// A known setting: its canonical key (== its `FLEETY_*` env name), scope,
/// default, one-line description, whether it holds a secret (masked in
/// display), and an optional value validator run before a write.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub scope: Scope,
    pub default: &'static str,
    pub description: &'static str,
    pub secret: bool,
    /// Reject out-of-domain values before they reach `config.toml`; `None` means
    /// any value is accepted (pass-through).
    pub validator: Option<Validator>,
}

/// The single source of truth for known settings. Adding one = one entry here.
pub fn registry() -> &'static [Setting] {
    use Scope::*;
    &[
        Setting {
            key: "FLEETY_ADDR",
            scope: Server,
            default: "0.0.0.0:8787",
            description: "WebSocket listen address; defaults to all interfaces so it is reachable across devices (auth is required by default). Set 127.0.0.1:8787 for loopback-only.",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_WORKSPACE",
            scope: Server,
            default: "(cwd)",
            description: "Fallback workspace root for tools.",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_POLICY",
            scope: Server,
            default: "full_access",
            description: "full_access or require_approval.",
            secret: false,
            validator: Some(v_policy),
        },
        Setting {
            key: "FLEETY_REQUIRE_AUTH",
            scope: Server,
            default: "1",
            description: "Require a token to connect, on by default (1/0); set 0 to disable.",
            secret: false,
            validator: Some(v_bool),
        },
        Setting {
            key: "FLEETY_TOKEN",
            scope: Server,
            default: "",
            description: "Bootstrap admin token for first pairing.",
            secret: true,
            validator: None,
        },
        Setting {
            key: "FLEETY_MODEL_BASE_URL",
            scope: Server,
            default: "",
            description: "OpenAI-compatible model base URL.",
            secret: false,
            validator: Some(v_url),
        },
        Setting {
            key: "FLEETY_MODEL",
            scope: Server,
            default: "",
            description: "Main model name.",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_MODEL_KEY",
            scope: Server,
            default: "",
            description: "Main model API key.",
            secret: true,
            validator: None,
        },
        Setting {
            key: "FLEETY_MODEL_RETRIES",
            scope: Server,
            default: "3",
            description: "Model-call retry attempts on transient failure (0 = no retry).",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_MODEL_RETRY_BASE_MS",
            scope: Server,
            default: "500",
            description: "Base backoff (ms) for model-call retries.",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_MODEL_RETRY_CAP_MS",
            scope: Server,
            default: "30000",
            description: "Max backoff (ms) for model-call retries.",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_MODEL_MODALITIES",
            scope: Server,
            default: "(heuristic)",
            description: "Main model input modalities, e.g. text,image,audio,pdf (overrides the name heuristic).",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_CHEAP_MODEL_MODALITIES",
            scope: Server,
            default: "(heuristic)",
            description: "Economy model input modalities (same form as FLEETY_MODEL_MODALITIES).",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_MODEL_EFFORT",
            scope: Server,
            default: "(none)",
            description: "Default reasoning effort for the main model: low/medium/high (only models that support effort).",
            secret: false,
            validator: Some(v_effort),
        },
        Setting {
            key: "FLEETY_CHEAP_MODEL_EFFORT",
            scope: Server,
            default: "(none)",
            description: "Default reasoning effort for the economy model (low/medium/high).",
            secret: false,
            validator: Some(v_effort),
        },
        Setting {
            key: "FLEETY_AUTO_EFFORT",
            scope: Server,
            default: "on",
            description: "Auto-select reasoning effort per top-level message by difficulty when the agent hasn't pinned one (on/off). Adds one cheap classification call per such message.",
            secret: false,
            validator: Some(v_onoff),
        },
        Setting {
            key: "FLEETY_VOICE_AUDIO",
            scope: Cli,
            default: "auto",
            description: "Voice transport: auto (send audio iff model accepts it) / on / off (local STT).",
            secret: false,
            validator: Some(v_voice_audio),
        },
        Setting {
            key: "FLEETY_CLI_AUTO_UPDATE",
            scope: Cli,
            default: "on",
            description: "When the CLI connects to a NEWER server, self-update to the server's version (forward-only) and re-run the command (on/off).",
            secret: false,
            validator: Some(v_onoff),
        },
        Setting {
            key: "FLEETY_VOICE_AUDIO_MAX_KB",
            scope: Cli,
            default: "2048",
            description: "Max voice-audio payload (KB) before falling back to local STT.",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_CHEAP_MODEL",
            scope: Server,
            default: "",
            description: "Economy/cheap model name (subagents, housekeeping).",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_TZ",
            scope: Shared,
            default: "(device)",
            description: "Fallback timezone for display (IANA). Unset → the host device's local timezone, then UTC.",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_FS_SCOPE",
            scope: Shared,
            default: "full",
            description: "full or workspace (path confinement).",
            secret: false,
            validator: Some(v_fs_scope),
        },
        Setting {
            key: "FLEETY_CMD_TIMEOUT_SECS",
            scope: Shared,
            default: "120",
            description: "Default wall-clock limit for run_command / ssh_exec (0 = no limit); a per-call timeout_secs overrides it.",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_TRANSFER_MAX_BYTES",
            scope: Shared,
            default: "67108864",
            description: "Max bytes for a single read_file_bytes / write_file_bytes / transfer_file (default 64 MiB). A larger file is refused (whole-file base64 has no chunking yet).",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_AUTO_INSTALL_DEPS",
            scope: Shared,
            default: "1",
            description: "Auto-install missing dependencies at boot (1/0).",
            secret: false,
            validator: Some(v_bool),
        },
        Setting {
            key: "FLEETY_CRV_AUTO_INSTALL",
            scope: Server,
            default: "1",
            description: "Auto-install the claude-real-video (`crv`) engine behind `video_extract` (1/0).",
            secret: false,
            validator: Some(v_bool),
        },
        Setting {
            key: "FLEETY_FFMPEG_AUTO_INSTALL",
            scope: Server,
            default: "1",
            description: "Auto-install ffmpeg (needed by `video_extract`) via the platform package manager (1/0).",
            secret: false,
            validator: Some(v_bool),
        },
        Setting {
            key: "FLEETY_VIDEO_WHISPER",
            scope: Server,
            default: "off",
            description: "Enable Whisper audio transcription in `video_extract` — installs the heavy transcription stack; keeps it off otherwise (on/off).",
            secret: false,
            validator: Some(v_onoff),
        },
        // FLEETY_AGENT_URL is deliberately NOT a registry setting: the connection
        // target lives in ~/.fleety/connections.toml and is managed via
        // `fleety server` (see fleety_tools::connection). It survives only as a
        // transient env override in the shared resolver, never seeded from
        // config.toml — so there is no config.json / config.toml / env precedence
        // trap. `config set FLEETY_AGENT_URL` is therefore an unknown key.
        // FLEETY_DEVICE_ID is deliberately NOT a registry setting. Stable
        // identity is resolved from the process override/machine id and stored
        // with connection state, so accepting it here would create a setting
        // that fleetyd never actually consumes.
        Setting {
            key: "FLEETY_FORCE_SSE",
            scope: Shared,
            default: "0",
            description: "Always use the SSE+POST transport, skipping WebSocket (1/0).",
            secret: false,
            validator: Some(v_bool),
        },
        Setting {
            key: "FLEETY_DISABLE_SSE",
            scope: Shared,
            default: "0",
            description: "Disable the SSE+POST fallback; WebSocket only (1/0).",
            secret: false,
            validator: Some(v_bool),
        },
        Setting {
            key: "FLEETY_SSE_TIMEOUT_SECS",
            scope: Shared,
            default: "45",
            description: "SSE half-open timeout: reconnect if no event/keep-alive arrives.",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_WS_PING_SECS",
            scope: Server,
            default: "20",
            description: "Seconds between server keepalive Pings on each WebSocket connection.",
            secret: false,
            validator: Some(v_pos_uint),
        },
        Setting {
            key: "FLEETY_WS_TIMEOUT_SECS",
            scope: Shared,
            default: "60",
            description: "WebSocket liveness deadline: a connection with no inbound frame for this many seconds is considered dead (use at least twice FLEETY_WS_PING_SECS).",
            secret: false,
            validator: Some(v_pos_uint),
        },
        Setting {
            key: "FLEETY_BACKUP_REPO",
            scope: Server,
            default: "",
            description: "Private GitHub repo for auto-backup (owner/repo or URL); unset disables.",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_BACKUP_TOKEN",
            scope: Server,
            default: "",
            description: "PAT for the backup repo (push + private-visibility check).",
            secret: true,
            validator: None,
        },
        Setting {
            key: "FLEETY_BACKUP_INTERVAL_SECS",
            scope: Server,
            default: "3600",
            description: "Seconds between scheduled backups.",
            secret: false,
            validator: Some(v_uint),
        },
        Setting {
            key: "FLEETY_PRESENCE",
            scope: Daemon,
            default: "off",
            description: "Presence tracking: report this device's co-location (on/off).",
            secret: false,
            validator: Some(v_presence),
        },
        Setting {
            key: "FLEETY_PRESENCE_INTERVAL_SECS",
            scope: Daemon,
            default: "300",
            description: "Seconds between co-location reports (floored at 60).",
            secret: false,
            validator: Some(v_uint),
        },
        // The Codex OAuth client id and endpoints are NOT config knobs — they are
        // OpenAI's fixed public values, hardcoded in `fleety_tools::oauth`
        // (`CODEX_CLIENT_ID`/`CODEX_AUTHORIZE_URL`/`CODEX_TOKEN_URL`/
        // `CODEX_BACKEND_URL`), like the loopback port. So they are intentionally
        // absent from the registry (not shown/editable in `fleety config`).
    ]
}

// ---- registry value validators ----
//
// Each validator inspects a to-be-written value and, on rejection, returns a
// short description of the accepted values (no key name — [`validate`] prepends
// it). Kept as bare `fn` pointers so `Setting` stays `Copy`. `validate` treats
// an empty value as unset and never calls a validator with it.

/// Accept only one of a fixed set of members; the error lists them.
fn check_enum(value: &str, allowed: &[&str]) -> std::result::Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("accepted values: {}", allowed.join(", ")))
    }
}

fn v_policy(value: &str) -> std::result::Result<(), String> {
    check_enum(value, &["full_access", "require_approval"])
}

fn v_fs_scope(value: &str) -> std::result::Result<(), String> {
    check_enum(value, &["full", "workspace"])
}

fn v_voice_audio(value: &str) -> std::result::Result<(), String> {
    check_enum(value, &["auto", "on", "off"])
}

fn v_presence(value: &str) -> std::result::Result<(), String> {
    check_enum(value, &["on", "off"])
}

fn v_effort(value: &str) -> std::result::Result<(), String> {
    check_enum(value, &["low", "medium", "high"])
}

fn v_onoff(value: &str) -> std::result::Result<(), String> {
    check_enum(value, &["on", "off"])
}

/// Accept only the boolean forms `0` or `1`.
fn v_bool(value: &str) -> std::result::Result<(), String> {
    match value {
        "0" | "1" => Ok(()),
        _ => Err("accepted values: 0 or 1".to_string()),
    }
}

/// Accept a non-negative integer (parses as `u64`, so no sign, decimal, or text).
fn v_uint(value: &str) -> std::result::Result<(), String> {
    value
        .parse::<u64>()
        .map(|_| ())
        .map_err(|_| "expected a non-negative integer".to_string())
}

/// Accept a positive integer (like [`v_uint`] but also rejecting `0` — for
/// periods/deadlines where zero is meaningless rather than "disabled").
fn v_pos_uint(value: &str) -> std::result::Result<(), String> {
    match value.parse::<u64>() {
        Ok(n) if n > 0 => Ok(()),
        _ => Err("expected a positive integer".to_string()),
    }
}

/// Accept only an `http://` or `https://` URL (scheme check, not full parse).
fn v_url(value: &str) -> std::result::Result<(), String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(())
    } else {
        Err("expected an http:// or https:// URL".to_string())
    }
}

/// Validate a to-be-written `value` for `setting`. A setting with no validator
/// accepts anything (pass-through); an empty value means unset and is never
/// validated. Otherwise the registry validator decides, and a rejection is
/// wrapped as `CoreError::Message` naming the key and the accepted values so the
/// user can correct it without reading source. Pure (no I/O).
pub fn validate(setting: &Setting, value: &str) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    let Some(check) = setting.validator else {
        return Ok(());
    };
    check(value)
        .map_err(|why| CoreError::Message(format!("invalid value for {}: {why}", setting.key)))
}

/// Find a setting by key (unknown keys are rejected by callers).
pub fn find(key: &str) -> Option<&'static Setting> {
    registry().iter().find(|s| s.key == key)
}

/// The config file path (`FLEETY_CONFIG` override, else `~/.fleety/config.toml`).
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_CONFIG") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".fleety").join("config.toml")
}

/// Stored config: (scope, key) → value.
pub type ConfigMap = HashMap<(Scope, String), String>;

/// Load config from `path`; missing → empty, corrupt → empty (fail soft, env +
/// defaults still work).
pub fn load(path: &std::path::Path) -> ConfigMap {
    let mut out = ConfigMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return out;
    };
    for (section, value) in table {
        let Some(scope) = Scope::from_str(&section) else {
            continue;
        };
        if let Some(t) = value.as_table() {
            for (k, v) in t {
                if let Some(s) = v.as_str() {
                    out.insert((scope, k.clone()), s.to_string());
                }
            }
        }
    }
    out
}

/// Persist config to `path` (TOML, sectioned by scope).
pub fn save(path: &std::path::Path, map: &ConfigMap) -> Result<()> {
    // Serialize all writes: config.toml is written rarely + small, so one lock
    // across the tmp+rename keeps a concurrent apply from interleaving.
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut root = toml::Table::new();
    for ((scope, key), value) in map {
        let section = root
            .entry(scope.as_str().to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(t) = section.as_table_mut() {
            t.insert(key.clone(), toml::Value::String(value.clone()));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create config dir: {e}")))?;
    }
    let text = toml::to_string_pretty(&root)
        .map_err(|e| CoreError::Message(format!("serialize config: {e}")))?;
    // Atomic: write a temp file in the same dir, then rename over the target, so
    // a crash / concurrent read never sees a half-written config.toml.
    let tmp_name = format!(".config-{}.tmp", uuid::Uuid::new_v4());
    let tmp = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(d) => d.join(tmp_name),
        None => std::path::PathBuf::from(tmp_name),
    };
    std::fs::write(&tmp, text).map_err(|e| CoreError::Message(format!("write config: {e}")))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Message(format!("replace config: {e}")));
    }
    Ok(())
}

struct MutationLock {
    path: PathBuf,
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Serialize read-modify-write across processes. The lock file is a short-lived
/// lease; an abandoned lease older than 30 seconds is reclaimed so a killed
/// process cannot permanently brick configuration changes.
fn acquire_mutation_lock(path: &std::path::Path) -> Result<MutationLock> {
    let lock_path = path.with_extension("toml.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create config dir: {e}")))?;
    }
    let started = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => return Ok(MutationLock { path: lock_path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let abandoned = std::fs::metadata(&lock_path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(30));
                if abandoned {
                    let _ = std::fs::remove_file(&lock_path);
                    continue;
                }
                if started.elapsed() >= Duration::from_secs(5) {
                    return Err(CoreError::Message(format!(
                        "timed out waiting for config mutation lock {}",
                        lock_path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                return Err(CoreError::Message(format!(
                    "cannot lock config {}: {e}",
                    lock_path.display()
                )))
            }
        }
    }
}

/// Run one complete Server-owned configuration transaction under a shared
/// cross-process lease. Both direct `fleety-server config` commands and remote
/// snapshot/apply paths use this boundary, so revision checks, reads, and the
/// eventual owner write cannot interleave with another Server mutation.
fn server_transaction_resource(config: &std::path::Path) -> PathBuf {
    config.with_file_name("server-configuration-transaction")
}

pub fn with_server_transaction<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let resource = server_transaction_resource(&config_path());
    let _lease = acquire_mutation_lock(&resource)?;
    operation()
}

/// Strict, cross-process-safe read-modify-write primitive for config owners.
pub fn mutate_strict(
    path: &std::path::Path,
    mutation: impl FnOnce(&mut ConfigMap) -> Result<()>,
) -> Result<()> {
    let _lock = acquire_mutation_lock(path)?;
    let mut map = load_strict(path)?;
    mutation(&mut map)?;
    save(path, &map)
}

/// Like [`mutate_strict`], but rejects a stale optimistic-lock revision while
/// still holding the same cross-process mutation lease used for the write.
pub fn mutate_strict_at_revision(
    path: &std::path::Path,
    expected_revision: &str,
    mutation: impl FnOnce(&mut ConfigMap) -> Result<()>,
) -> Result<()> {
    let _lock = acquire_mutation_lock(path)?;
    let current = revision(path);
    if current != expected_revision {
        return Err(CoreError::Message(format!(
            "config revision conflict: expected {expected_revision}, current {current}"
        )));
    }
    let mut map = load_strict(path)?;
    mutation(&mut map)?;
    save(path, &map)
}

/// Replace only the supplied owner's scopes while preserving every foreign
/// scope written by other runtimes since the panel was opened.
pub fn replace_scopes_strict(
    path: &std::path::Path,
    scopes: &[Scope],
    replacement: &ConfigMap,
) -> Result<()> {
    mutate_strict(path, |current| {
        current.retain(|(scope, _), _| !scopes.contains(scope));
        current.extend(
            replacement
                .iter()
                .filter(|((scope, _), _)| scopes.contains(scope))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Ok(())
    })
}

/// Load config, erroring on a present-but-broken file rather than fail-softing
/// to empty (which the boot-time [`load`] does). Used by the remote-config apply
/// path so a corrupt `config.toml` is a clear error, not a silent revert to
/// defaults (design §8, M3).
pub fn load_strict(path: &std::path::Path) -> Result<ConfigMap> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ConfigMap::new()),
        Err(e) => return Err(CoreError::Message(format!("cannot read config.toml: {e}"))),
    };
    let table = text.parse::<toml::Table>().map_err(|e| {
        CoreError::Message(format!(
            "config.toml is present but unparseable ({e}); fix it — the server will not \
             silently apply defaults over a broken file"
        ))
    })?;
    let mut out = ConfigMap::new();
    for (section, value) in table {
        let Some(scope) = Scope::from_str(&section) else {
            continue;
        };
        if let Some(t) = value.as_table() {
            for (k, v) in t {
                if let Some(s) = v.as_str() {
                    out.insert((scope, k.clone()), s.to_string());
                }
            }
        }
    }
    Ok(out)
}

/// A revision string for optimistic locking: a stable content hash of the config
/// file's bytes (a missing/empty file hashes to a fixed marker). Deterministic
/// across a program run, so a snapshot's revision compared to the current one
/// detects any concurrent write. The server combines it with its boot id.
pub fn revision(path: &std::path::Path) -> String {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// A structured snapshot of one setting for the remote-config panel. A secret's
/// `value` is empty (only `is_set` is reported). The server maps this to the
/// protocol `ConfigEntry` (keeping this crate protocol-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub key: &'static str,
    pub scope: Scope,
    pub value: String,
    pub default: &'static str,
    pub description: &'static str,
    pub secret: bool,
    pub is_set: bool,
    pub effect: Option<ConfigEffect>,
    pub choices: Vec<&'static str>,
}

/// The enumerated valid choices for a setting (a display hint for the panel;
/// the registry validator remains the source of truth for rejection). Empty for
/// free-form keys.
pub fn setting_choices(key: &str) -> Vec<&'static str> {
    match key {
        "FLEETY_POLICY" => vec!["full_access", "require_approval"],
        "FLEETY_FS_SCOPE" => vec!["full", "workspace"],
        "FLEETY_VOICE_AUDIO" => vec!["auto", "on", "off"],
        "FLEETY_PRESENCE" => vec!["on", "off"],
        "FLEETY_AUTO_EFFORT" => vec!["on", "off"],
        "FLEETY_VIDEO_WHISPER" => vec!["on", "off"],
        "FLEETY_CLI_AUTO_UPDATE" => vec!["on", "off"],
        "FLEETY_MODEL_EFFORT" | "FLEETY_CHEAP_MODEL_EFFORT" => vec!["low", "medium", "high"],
        "FLEETY_REQUIRE_AUTH"
        | "FLEETY_AUTO_INSTALL_DEPS"
        | "FLEETY_CRV_AUTO_INSTALL"
        | "FLEETY_FFMPEG_AUTO_INSTALL"
        | "FLEETY_FORCE_SSE"
        | "FLEETY_DISABLE_SSE" => vec!["0", "1"],
        _ => vec![],
    }
}

/// Structured snapshot of the registry settings (optionally filtered to
/// `scopes`), for the remote-config panel. A secret's value is omitted (only
/// `is_set`); flat registry keys are env-seeded so their effect is `Restart`.
pub fn snapshot_entries(map: &ConfigMap, scopes: Option<&[Scope]>) -> Vec<SnapshotEntry> {
    registry()
        .iter()
        .filter(|s| scopes.map(|sc| sc.contains(&s.scope)).unwrap_or(true))
        .filter_map(|s| {
            let r = resolve(s.key, map)?;
            Some(SnapshotEntry {
                key: s.key,
                scope: s.scope,
                value: if s.secret { String::new() } else { r.value },
                default: s.default,
                description: s.description,
                secret: s.secret,
                is_set: r.source != Source::Default,
                effect: Some(ConfigEffect::Restart),
                choices: setting_choices(s.key),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Config,
    Default,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub value: String,
    pub source: Source,
}

/// Resolve a known key: env (non-empty) → config (its scope) → registry default.
pub fn resolve(key: &str, map: &ConfigMap) -> Option<Resolved> {
    let setting = find(key)?;
    if let Ok(v) = std::env::var(key) {
        if !v.is_empty() {
            return Some(Resolved {
                value: v,
                source: Source::Env,
            });
        }
    }
    if let Some(v) = map.get(&(setting.scope, key.to_string())) {
        return Some(Resolved {
            value: v.clone(),
            source: Source::Config,
        });
    }
    Some(Resolved {
        value: setting.default.to_string(),
        source: Source::Default,
    })
}

/// Seed env from config: for each known setting that is unset in the env but
/// present in config, set the env var. Env always wins (we never overwrite a set
/// var), so existing env deployments are unaffected. Call once, early at boot.
pub fn seed_env_from_config(map: &ConfigMap) {
    let mut explicit = std::collections::HashSet::new();
    for setting in registry() {
        let already = std::env::var(setting.key)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if already {
            // A real env var (not our seeding) — remember it so `set` can warn
            // that the env keeps overriding the config value.
            explicit.insert(setting.key.to_string());
            continue;
        }
        if let Some(v) = map.get(&(setting.scope, setting.key.to_string())) {
            std::env::set_var(setting.key, v);
        }
    }
    let _ = EXPLICIT_ENV.set(explicit);
}

/// Registry keys that were present as real environment variables at process
/// start (before boot seeding). Empty until `seed_env_from_config` runs.
static EXPLICIT_ENV: std::sync::OnceLock<std::collections::HashSet<String>> =
    std::sync::OnceLock::new();

/// Whether `key` was an explicit env var at process start. Such a var takes
/// precedence over the config file, so a `config set` of it never bites.
pub fn explicitly_in_env(key: &str) -> bool {
    EXPLICIT_ENV.get().map(|s| s.contains(key)).unwrap_or(false)
}

/// Mask a value for display when its setting is secret.
pub fn display_value(setting: &Setting, value: &str) -> String {
    if setting.secret && !value.is_empty() {
        "********".to_string()
    } else {
        value.to_string()
    }
}

/// When a config operation takes effect. Local mirror of the wire `Effect`
/// (this crate doesn't depend on the protocol crate); the server maps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigEffect {
    /// Picked up on the next client connection (providers.toml is re-read per connection).
    NextConnection,
    /// Needs a restart (flat settings are env-seeded at boot and env takes precedence).
    Restart,
}

/// Classify when a config operation takes effect, by its verb. `None` means a
/// read (no change). A mutating `provider`/`group`/`role` op rewrites
/// `providers.toml`, which the provider registry re-reads on the next
/// connection; a flat `set`/`unset` is shadowed by the boot-seeded environment
/// until a restart. Pure.
pub fn config_effect(args: &[String]) -> Option<ConfigEffect> {
    match args.first().map(String::as_str) {
        Some("provider" | "model") => match args.get(1).map(String::as_str) {
            // A read sub-verb changes nothing.
            Some("list") | None => None,
            _ => Some(ConfigEffect::NextConnection),
        },
        Some("set" | "unset") => Some(ConfigEffect::Restart),
        // list / get / edit / unknown → read or no-op.
        _ => None,
    }
}

// ---- command dispatch, shared by `fleety`, `fleety-server`, and `fleetyd` ----

/// Shared, side-effect-free command grammar used by `fleety`, `fleetyd`, and
/// `fleety-server`. Keeping this tree here prevents one binary from accepting
/// config input that another rejects or from falling through to file handling
/// before help and usage errors are resolved.
pub fn clap_command() -> ClapCommand {
    config_clap_command(false, true, false, false, true)
}

/// Daemon direct config excludes Server-owned provider/model commands.
pub fn clap_command_for_daemon() -> ClapCommand {
    config_clap_command(false, false, false, false, false)
}

/// CLI variant adds the historical `config provider login|logout|status`
/// aliases. Daemon and Server direct config commands intentionally exclude
/// these browser/client-side OAuth actions.
pub fn clap_command_for_cli() -> ClapCommand {
    // `fleety` already has a global `--url` connection override, so exposing
    // the Provider alias here would make Clap register two different --url
    // options. The CLI uses canonical --base-url; direct fleety-server keeps the
    // legacy --url alias because it has no global selector.
    config_clap_command(true, true, true, true, false)
}

fn config_clap_command(
    provider_auth: bool,
    providers: bool,
    provider_edit: bool,
    model_catalog: bool,
    provider_url_alias: bool,
) -> ClapCommand {
    let mut command = ClapCommand::new("config")
        .about("Inspect or edit owner configuration")
        .subcommands([
            ClapCommand::new("list").about("List settings"),
            ClapCommand::new("get")
                .about("Read a setting")
                .arg(Arg::new("key").required(true)),
            ClapCommand::new("set")
                .about("Set a setting")
                .arg(Arg::new("key").required(true))
                .arg(Arg::new("value").required(true)),
            ClapCommand::new("unset")
                .about("Unset a setting")
                .arg(Arg::new("key").required(true)),
            ClapCommand::new("open")
                .about("Open the shared Settings workspace")
                .visible_alias("edit"),
        ]);
    if providers {
        command = command.subcommands([
            provider_clap_command(provider_auth, provider_edit, provider_url_alias),
            model_clap_command(model_catalog),
        ]);
    }
    command
}

fn provider_clap_command(
    include_auth: bool,
    include_edit: bool,
    include_url_alias: bool,
) -> ClapCommand {
    let mut command = ClapCommand::new("provider")
        .about("Manage Server-owned providers")
        .subcommand_required(true)
        .subcommands([
            provider_fields(
                ClapCommand::new("add")
                    .about("Add a provider")
                    .arg(Arg::new("name").required(true)),
                true,
                include_url_alias,
            ),
            provider_fields(
                ClapCommand::new("set")
                    .about("Update a provider")
                    .arg(Arg::new("name").required(true)),
                false,
                include_url_alias,
            ),
            ClapCommand::new("remove")
                .about("Remove a provider")
                .arg(Arg::new("name").required(true)),
            ClapCommand::new("list").about("List providers"),
        ]);
    if include_edit {
        command = command.subcommand(ClapCommand::new("edit").about("Open the provider editor"));
    }
    if include_auth {
        command = command.subcommands([
            ClapCommand::new("login")
                .about("Sign in an OAuth provider")
                .arg(Arg::new("provider").required(true))
                .arg(
                    Arg::new("no-browser")
                        .long("no-browser")
                        .action(ArgAction::SetTrue),
                ),
            ClapCommand::new("logout")
                .about("Sign out an OAuth provider")
                .arg(Arg::new("provider").required(true)),
            ClapCommand::new("status")
                .about("Show OAuth status")
                .arg(Arg::new("provider")),
        ]);
    }
    command
}

fn provider_fields(
    command: ClapCommand,
    kind_required: bool,
    include_url_alias: bool,
) -> ClapCommand {
    let base_url = Arg::new("base-url").long("base-url").value_name("URL");
    let base_url = if include_url_alias {
        base_url.visible_alias("url")
    } else {
        base_url
    };
    command
        .arg(
            Arg::new("type")
                .long("type")
                .value_name("TYPE")
                .required(kind_required),
        )
        .arg(base_url)
        .arg(Arg::new("key").long("key").value_name("KEY"))
}

fn model_clap_command(include_catalog: bool) -> ClapCommand {
    let mut command = ClapCommand::new("model")
        .about("Manage Server-owned model roles")
        .subcommand_required(true)
        .subcommands([
            ClapCommand::new("list").about("List model roles"),
            ClapCommand::new("show")
                .about("Show a model role")
                .arg(Arg::new("role")),
            ClapCommand::new("unset")
                .about("Unset a model role")
                .arg(Arg::new("role").required(true)),
            ClapCommand::new("set")
                .about("Set a model role")
                .arg(Arg::new("role").required(true))
                .arg(
                    Arg::new("member")
                        .long("member")
                        .value_name("PROVIDER/MODEL")
                        .required(true)
                        .action(ArgAction::Append),
                )
                .arg(Arg::new("stream").long("stream").action(ArgAction::Count))
                .arg(
                    Arg::new("modalities")
                        .long("modalities")
                        .value_name("LIST")
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("effort")
                        .long("effort")
                        .value_name("LEVEL")
                        .action(ArgAction::Append),
                )
                .arg(Arg::new("strategy").long("strategy").value_name("STRATEGY")),
        ]);
    if include_catalog {
        command = command.subcommand(
            ClapCommand::new("catalog")
                .about("Fetch a Provider's available model IDs")
                .arg(Arg::new("provider").required(true))
                .arg(Arg::new("role").long("role").value_name("ROLE")),
        );
    }
    command
}

/// A parsed `config` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    List,
    Get(String),
    Set(String, String),
    Unset(String),
    Edit,
    Help,
}

/// Parse `config <args...>`. Pure and unit-testable.
pub fn parse(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("list") | None => Command::List,
        Some("get") => args
            .get(1)
            .map(|k| Command::Get(k.clone()))
            .unwrap_or(Command::Help),
        Some("set") => match (args.get(1), args.get(2)) {
            (Some(k), Some(v)) => Command::Set(k.clone(), v.clone()),
            _ => Command::Help,
        },
        Some("unset") => args
            .get(1)
            .map(|k| Command::Unset(k.clone()))
            .unwrap_or(Command::Help),
        Some("open" | "edit") => Command::Edit,
        _ => Command::Help,
    }
}

fn source_label(s: Source) -> &'static str {
    match s {
        Source::Env => "env",
        Source::Config => "config",
        Source::Default => "default",
    }
}

/// Display rows for `list`: (key, scope, shown value [secrets masked], source).
pub fn rows(map: &ConfigMap) -> Vec<(String, String, String, String)> {
    rows_in_scopes(
        map,
        &[Scope::Server, Scope::Daemon, Scope::Cli, Scope::Shared],
    )
}

/// The scopes a local CLI edits — its own device behavior. Server/Daemon
/// settings are edited on their own hosts (the server remotely, the daemon via
/// `fleetyd config`), not through `fleety config --target local`.
pub const LOCAL_SCOPES: &[Scope] = CLI_SCOPES;

/// Display rows restricted to `scopes` (same shape as [`rows`]).
pub fn rows_in_scopes(map: &ConfigMap, scopes: &[Scope]) -> Vec<(String, String, String, String)> {
    registry()
        .iter()
        .filter(|s| scopes.contains(&s.scope))
        .filter_map(|s| {
            let r = resolve(s.key, map)?;
            Some((
                s.key.to_string(),
                s.scope.as_str().to_string(),
                display_value(s, &r.value),
                source_label(r.source).to_string(),
            ))
        })
        .collect()
}

/// Reject editing `key` when it isn't in `scopes` — it belongs to another host.
/// The error names where to edit it (the server, or the daemon) so a local
/// `--target local` edit of a foreign key is redirected, not silently a no-op.
/// An unknown key gets the usual unknown-setting error.
pub fn ensure_scope(key: &str, scopes: &[Scope]) -> Result<()> {
    if key == "FLEETY_DEVICE_ID" {
        return owner_for_key(key).map(|_| ());
    }
    match find(key) {
        Some(s) if scopes.contains(&s.scope) => Ok(()),
        Some(s) => {
            let where_to = match s.scope {
                Scope::Server => "on the server (the default `fleety config set` targets it)",
                Scope::Daemon | Scope::Shared => "through the selected daemon",
                Scope::Cli => "through the CLI owner",
            };
            Err(CoreError::Message(format!(
                "'{key}' is a {} setting — edit it {where_to}, not via `--target local`",
                s.scope.as_str()
            )))
        }
        None => Err(CoreError::Message(format!("unknown setting '{key}'"))),
    }
}

/// Run a `config` subcommand against the config file (unfiltered — server/daemon
/// on their own hosts). `open` is the line-based host-local loop for direct
/// Server/Daemon binaries; `fleety` routes it to the shared Settings workspace.
pub fn run(args: &[String]) -> Result<()> {
    run_scoped(args, None)
}

/// Like [`run`] but restricting the flat-key subcommands to `scopes` when
/// `Some` — the CLI's local target passes [`LOCAL_SCOPES`] so it edits only this
/// device's own settings. `None` is unfiltered (server/daemon on their host).
pub fn run_scoped(args: &[String], scopes: Option<&[Scope]>) -> Result<()> {
    // Interactive flat-key edit stays local + line-based; everything else
    // renders to text (so the same code serves the remote handler).
    let is_providers = matches!(args.first().map(String::as_str), Some("provider" | "model"));
    if !is_providers && matches!(parse(args), Command::Edit) {
        return edit_line_based(&config_path());
    }
    let out = run_rendered_scoped(args, scopes)?;
    let out = out.trim_end_matches('\n');
    if !out.is_empty() {
        println!("{}", crate::transport::terminal_safe_multiline(out));
    }
    Ok(())
}

/// Run a `config` subcommand and return its rendered text instead of printing,
/// so the remote handler can send it over the wire (unfiltered — all scopes).
pub fn run_rendered(args: &[String]) -> Result<String> {
    run_rendered_scoped(args, None)
}

/// Like [`run_rendered`] but restricting the flat-key subcommands to `scopes`
/// when `Some`: `list` shows only those scopes, and `get`/`set`/`unset` of a
/// key outside them is refused (see [`ensure_scope`]). `provider`/`model`
/// manage the structured providers.toml; `edit` is interactive (an error here).
pub fn run_rendered_scoped(args: &[String], scopes: Option<&[Scope]>) -> Result<String> {
    if matches!(args.first().map(String::as_str), Some("provider" | "model"))
        && scopes.is_some_and(|allowed| !allowed.contains(&Scope::Server))
    {
        return Err(CoreError::Message(
            "provider and model configuration is owned by fleety-server".to_string(),
        ));
    }
    if matches!(args.first().map(String::as_str), Some("provider" | "model")) {
        return run_providers_at(&pc::providers_path(), args)
            .map(|out| crate::transport::redact_urls_in_text(&out));
    }
    let path = config_path();
    let rendered = match parse(args) {
        Command::List => {
            let map = load(&path);
            let displayed = match scopes {
                Some(sc) => rows_in_scopes(&map, sc),
                None => rows(&map),
            };
            let header = if scopes.is_some() {
                "This device's settings (env → config → default; secrets masked):\n\n"
            } else {
                "Settings (env → config → default; secrets masked):\n\n"
            };
            let mut out = String::from(header);
            for (key, scope, value, source) in displayed {
                out.push_str(&format!("  [{scope:6}] {key:<26} = {value}  ({source})\n"));
            }
            out.push_str(&format!(
                "\nEdit with: config set <KEY> <VALUE>   (file: {})",
                path.display()
            ));
            out
        }
        Command::Get(key) => {
            if let Some(sc) = scopes {
                ensure_scope(&key, sc)?;
            }
            let setting =
                find(&key).ok_or_else(|| CoreError::Message(format!("unknown setting '{key}'")))?;
            let map = load(&path);
            match resolve(&key, &map) {
                Some(r) => format!(
                    "{key} = {}  ({})",
                    display_value(setting, &r.value),
                    source_label(r.source)
                ),
                None => String::new(),
            }
        }
        Command::Set(key, value) => {
            if key == "FLEETY_DEVICE_ID" {
                owner_for_key(&key)?;
            }
            if let Some(sc) = scopes {
                ensure_scope(&key, sc)?;
            }
            let setting = find(&key).ok_or_else(|| {
                CoreError::Message(format!(
                    "unknown setting '{key}'. Run `config list` to see valid keys."
                ))
            })?;
            // Reject out-of-domain values before touching the file, so a typo
            // never lands silently (server/daemon and remote `--target` all
            // funnel through here).
            validate(setting, &value)?;
            mutate_strict(&path, |map| {
                map.insert((setting.scope, key.clone()), value.clone());
                Ok(())
            })?;
            let mut out = format!("set {key} (scope {})", setting.scope.as_str());
            // Tell the user which process must restart for the change to bite —
            // "takes effect after a restart" alone leaves them guessing.
            out.push_str(match setting.scope {
                Scope::Server => " — restart the server to apply (`fleety-server restart`)",
                Scope::Daemon => " — restart the daemon to apply (`fleetyd restart`)",
                Scope::Cli => " — applies on the next fleety command",
                Scope::Shared => " — restart the affected fleety process(es) to apply",
            });
            if explicitly_in_env(&key) {
                out.push_str(&format!(
                    "\nnote: {key} is currently set as an environment variable, which takes \
                     precedence over this config value — it only wins once the env var is removed"
                ));
            }
            out
        }
        Command::Unset(key) => {
            if key == "FLEETY_DEVICE_ID" {
                owner_for_key(&key)?;
            }
            if let Some(sc) = scopes {
                ensure_scope(&key, sc)?;
            }
            let setting =
                find(&key).ok_or_else(|| CoreError::Message(format!("unknown setting '{key}'")))?;
            mutate_strict(&path, |map| {
                map.remove(&(setting.scope, key.clone()));
                Ok(())
            })?;
            format!("unset {key} (reverts to env/default)")
        }
        Command::Edit => {
            return Err(CoreError::Message(
                "`config open` is interactive — run it locally on a TTY".to_string(),
            ))
        }
        Command::Help => {
            "usage: config [list | get <KEY> | set <KEY> <VALUE> | unset <KEY> | open]".to_string()
        }
    };
    Ok(crate::transport::redact_urls_in_text(&rendered))
}

// ---- providers.toml subcommands (provider / group / role) ----

use crate::providers_config::{self as pc, Member, ModelPool, Provider, Strategy};

/// A parsed `provider` / `model` subcommand over `providers.toml` (the two-tier
/// model: `type`-tagged providers and `main`/`cheap` member pools).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCmd {
    ProviderAdd {
        name: String,
        kind: String,
        base_url: Option<String>,
        key: Option<String>,
    },
    /// Update a named provider's fields (only `Some` fields change).
    ProviderSet {
        name: String,
        kind: Option<String>,
        base_url: Option<String>,
        key: Option<String>,
        clear_key: bool,
    },
    ProviderRemove(String),
    ProviderList,
    /// Set a model role (`main`/`cheap`) to a member pool with a strategy.
    ModelSet {
        role: String,
        members: Vec<Member>,
        strategy: Strategy,
    },
    ModelShow(Option<String>),
    ModelUnset(String),
    ModelList,
}

fn strategy_from(s: &str) -> Result<Strategy> {
    match s {
        "single" => Ok(Strategy::Single),
        "round_robin" => Ok(Strategy::RoundRobin),
        "failover" => Ok(Strategy::Failover),
        _ => Err(CoreError::Message(format!(
            "invalid strategy '{s}' (expected single, round_robin, or failover)"
        ))),
    }
}

fn normalize_provider_model_value_options(args: &[String]) -> Vec<String> {
    const VALUE_OPTIONS: &[&str] = &[
        "--type",
        "--base-url",
        "--url",
        "--key",
        "--member",
        "--modalities",
        "--effort",
        "--strategy",
    ];

    let mut normalized = Vec::with_capacity(args.len());
    let mut options_enabled = true;
    for arg in args {
        if options_enabled && arg == "--" {
            options_enabled = false;
            normalized.push(arg.clone());
            continue;
        }

        if options_enabled {
            if let Some((flag, value)) = arg.split_once('=') {
                if VALUE_OPTIONS.contains(&flag) {
                    normalized.push(if flag == "--url" {
                        "--base-url".to_string()
                    } else {
                        flag.to_string()
                    });
                    normalized.push(value.to_string());
                    continue;
                }
            }
            if arg == "--url" {
                normalized.push("--base-url".to_string());
                continue;
            }
        }

        normalized.push(arg.clone());
    }
    normalized
}

/// Split one provider name and its `--flag value` fields without imposing an
/// order. This matches clap's generated usage, which permits options before or
/// after the positional name. `--` ends option parsing, matching clap.
fn split_provider_args(
    args: &[String],
    verb: &str,
) -> Result<(String, HashMap<String, String>, HashSet<String>)> {
    let mut kv = HashMap::new();
    let mut switches = HashSet::new();
    let mut provider_name = None;
    let mut i = 0;
    let mut options_enabled = true;
    while i < args.len() {
        if options_enabled && args[i] == "--" {
            options_enabled = false;
            i += 1;
        } else if options_enabled {
            let Some(name) = args[i].strip_prefix("--") else {
                if provider_name.is_none() {
                    provider_name = Some(args[i].clone());
                    i += 1;
                    continue;
                }
                return Err(CoreError::Message(format!(
                    "provider {verb} needs exactly one provider name"
                )));
            };
            if name == "clear-key" {
                if !switches.insert(name.to_string()) {
                    return Err(CoreError::Message(
                        "flag --clear-key cannot be used multiple times".to_string(),
                    ));
                }
                i += 1;
                continue;
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| CoreError::Message(format!("flag --{name} needs a value")))?;
            if kv.insert(name.to_string(), value.clone()).is_some() {
                return Err(CoreError::Message(format!(
                    "flag --{name} cannot be used multiple times"
                )));
            }
            i += 2;
        } else if provider_name.is_none() {
            provider_name = Some(args[i].clone());
            i += 1;
        } else {
            return Err(CoreError::Message(format!(
                "provider {verb} needs exactly one provider name"
            )));
        }
    }
    let name = provider_name
        .ok_or_else(|| CoreError::Message(format!("provider {verb} needs a provider name")))?;
    Ok((name, kv, switches))
}

/// Error if any flags were given but not consumed (catches typos/unknown flags).
fn no_unknown_flags(kv: &HashMap<String, String>) -> Result<()> {
    if let Some(k) = kv.keys().next() {
        return Err(CoreError::Message(format!("unknown flag --{k}")));
    }
    Ok(())
}

/// Parse `model set <role> --member <provider>/<model> [--stream] [--modalities
/// <list>] [--effort <level>] [--member …] --strategy <s>`. The trait flags
/// attach to the most recent `--member`; `--strategy` is pool-level (defaulting
/// to `single` for one member, else `failover`).
fn parse_model_set(role: String, rest: &[String]) -> Result<ProviderCmd> {
    let mut members: Vec<Member> = Vec::new();
    let mut strategy: Option<Strategy> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--member" => {
                let spec = rest.get(i + 1).ok_or_else(|| {
                    CoreError::Message("--member needs <provider>/<model>".to_string())
                })?;
                let (provider, model) = spec.split_once('/').ok_or_else(|| {
                    CoreError::Message(format!("--member '{spec}' must be <provider>/<model>"))
                })?;
                members.push(Member {
                    provider: provider.to_string(),
                    model: model.to_string(),
                    stream: false,
                    modalities: None,
                    effort: None,
                });
                i += 2;
            }
            "--stream" => {
                members
                    .last_mut()
                    .ok_or_else(|| {
                        CoreError::Message("--stream must follow a --member".to_string())
                    })?
                    .stream = true;
                i += 1;
            }
            "--modalities" => {
                let v = rest
                    .get(i + 1)
                    .ok_or_else(|| CoreError::Message("--modalities needs a value".to_string()))?
                    .clone();
                members
                    .last_mut()
                    .ok_or_else(|| {
                        CoreError::Message("--modalities must follow a --member".to_string())
                    })?
                    .modalities = Some(v);
                i += 2;
            }
            "--effort" => {
                let v = rest
                    .get(i + 1)
                    .ok_or_else(|| CoreError::Message("--effort needs a value".to_string()))?
                    .clone();
                members
                    .last_mut()
                    .ok_or_else(|| {
                        CoreError::Message("--effort must follow a --member".to_string())
                    })?
                    .effort = Some(v);
                i += 2;
            }
            "--strategy" => {
                let v = rest
                    .get(i + 1)
                    .ok_or_else(|| CoreError::Message("--strategy needs a value".to_string()))?;
                if strategy.replace(strategy_from(v)?).is_some() {
                    return Err(CoreError::Message(
                        "--strategy cannot be used multiple times".to_string(),
                    ));
                }
                i += 2;
            }
            other => {
                return Err(CoreError::Message(format!(
                    "unknown flag '{other}' for `model set`"
                )))
            }
        }
    }
    if members.is_empty() {
        return Err(CoreError::Message(
            "model set needs at least one --member <provider>/<model>".to_string(),
        ));
    }
    let strategy = strategy.unwrap_or(if members.len() == 1 {
        Strategy::Single
    } else {
        Strategy::Failover
    });
    Ok(ProviderCmd::ModelSet {
        role,
        members,
        strategy,
    })
}

fn parse_model_set_with_flexible_role(rest: &[String]) -> Result<ProviderCmd> {
    let mut role = None;
    let mut flags = Vec::new();
    let mut index = 0;
    let mut options_enabled = true;
    while index < rest.len() {
        if options_enabled && rest[index] == "--" {
            options_enabled = false;
            index += 1;
            continue;
        }
        match (options_enabled, rest[index].as_str()) {
            (true, flag @ ("--member" | "--modalities" | "--effort" | "--strategy")) => {
                flags.push(flag.to_string());
                let value = rest
                    .get(index + 1)
                    .ok_or_else(|| CoreError::Message(format!("{flag} needs a value")))?;
                flags.push(value.clone());
                index += 2;
            }
            (true, "--stream") => {
                flags.push("--stream".to_string());
                index += 1;
            }
            (_, value) if (!options_enabled || !value.starts_with('-')) && role.is_none() => {
                role = Some(value.to_string());
                index += 1;
            }
            (_, value) if !options_enabled || !value.starts_with('-') => {
                return Err(CoreError::Message(
                    "model set needs exactly one model role".to_string(),
                ));
            }
            (_, other) => {
                return Err(CoreError::Message(format!(
                    "unknown flag '{other}' for `model set`"
                )))
            }
        }
    }
    let role =
        role.ok_or_else(|| CoreError::Message("missing model role (main|cheap)".to_string()))?;
    parse_model_set(role, &flags)
}

/// Parse a `provider` / `model` subcommand. Pure and unit-testable; an unknown
/// verb, missing required field, bad strategy, or unknown flag is an error.
pub fn parse_providers(args: &[String]) -> Result<ProviderCmd> {
    let args = normalize_provider_model_value_options(args);
    let kind = args.first().map(String::as_str);
    let verb = args.get(1).map(String::as_str);
    let rest = if args.len() > 2 { &args[2..] } else { &[][..] };
    let need = |o: Option<&String>, what: &str| -> Result<String> {
        o.cloned()
            .ok_or_else(|| CoreError::Message(format!("missing {what}")))
    };
    match (kind, verb) {
        (Some("provider"), Some("list")) if rest.is_empty() => Ok(ProviderCmd::ProviderList),
        (Some("provider"), Some("remove")) if rest.len() == 1 => Ok(ProviderCmd::ProviderRemove(
            need(rest.first(), "provider name")?,
        )),
        (Some("provider"), Some("add")) => {
            let (name, mut kv, switches) = split_provider_args(rest, "add")?;
            let kind = kv.remove("type").ok_or_else(|| {
                CoreError::Message("missing --type (api|oauth:codex)".to_string())
            })?;
            let base_url = kv.remove("base-url");
            let key = kv.remove("key");
            no_unknown_flags(&kv)?;
            if switches.contains("clear-key") {
                return Err(CoreError::Message(
                    "--clear-key is only valid with `provider set`".to_string(),
                ));
            }
            Ok(ProviderCmd::ProviderAdd {
                name,
                kind,
                base_url,
                key,
            })
        }
        (Some("provider"), Some("set")) => {
            let (name, mut kv, switches) = split_provider_args(rest, "set")?;
            let kind = kv.remove("type");
            let base_url = kv.remove("base-url");
            let key = kv.remove("key");
            no_unknown_flags(&kv)?;
            let clear_key = switches.contains("clear-key");
            if clear_key && key.is_some() {
                return Err(CoreError::Message(
                    "--key and --clear-key cannot be used together".to_string(),
                ));
            }
            Ok(ProviderCmd::ProviderSet {
                name,
                kind,
                base_url,
                key,
                clear_key,
            })
        }
        (Some("model"), Some("list")) if rest.is_empty() => Ok(ProviderCmd::ModelList),
        (Some("model"), Some("show")) if rest.len() <= 1 => {
            Ok(ProviderCmd::ModelShow(rest.first().cloned()))
        }
        (Some("model"), Some("unset")) if rest.len() == 1 => {
            Ok(ProviderCmd::ModelUnset(need(rest.first(), "model role")?))
        }
        (Some("model"), Some("set")) => parse_model_set_with_flexible_role(rest),
        _ => Err(CoreError::Message(
            "usage: config provider <add|set|remove|list> | model <set|show|unset|list>"
                .to_string(),
        )),
    }
}

fn mask_key(key: &Option<String>) -> &'static str {
    match key {
        Some(k) if !k.is_empty() => "********",
        _ => "(none)",
    }
}

/// Execute a `provider` / `model` subcommand against `providers.toml`: load
/// (empty when missing, error when present-but-broken), apply, validate, and
/// write atomically.
pub fn run_providers(args: &[String]) -> Result<()> {
    let out = run_providers_at(&pc::providers_path(), args)?;
    let out = out.trim_end_matches('\n');
    if !out.is_empty() {
        println!("{}", crate::transport::terminal_safe_multiline(out));
    }
    Ok(())
}

/// Render one model role's members for `model show` / list.
fn render_model_role(role: &str, pool: &ModelPool) -> String {
    let mut s = format!("model role '{role}' [{:?}]\n", pool.strategy);
    for m in &pool.members {
        s.push_str(&format!(
            "  {}/{}  stream={}  modalities={}  effort={}\n",
            m.provider,
            m.model,
            m.stream,
            m.modalities.as_deref().unwrap_or("(auto)"),
            m.effort.as_deref().unwrap_or("(none)")
        ));
    }
    s
}

/// Like [`run_providers`] but against an explicit `providers.toml` path and
/// returning the rendered text instead of printing (so the remote handler and
/// the interactive editor reuse it). Tests use this directly.
pub fn run_providers_at(path: &std::path::Path, args: &[String]) -> Result<String> {
    let cmd = parse_providers(args)?;
    match cmd {
        ProviderCmd::ProviderList => {
            let cfg = pc::load_or_default(path)?;
            let mut out = String::new();
            if cfg.providers.is_empty() {
                out.push_str(&format!("(no providers defined in {})\n", path.display()));
            }
            for (name, p) in &cfg.providers {
                match p.base_url.as_deref() {
                    Some(url) => out.push_str(&format!(
                        "  {name:<16} [{}]  {}  key={}\n",
                        p.kind,
                        crate::transport::redact_endpoint(url),
                        mask_key(&p.key)
                    )),
                    None => out.push_str(&format!(
                        "  {name:<16} [{}]  (token via `fleety auth login {name}`)\n",
                        p.kind
                    )),
                }
            }
            Ok(out)
        }
        ProviderCmd::ProviderAdd {
            name,
            kind,
            base_url,
            key,
        } => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.provider(&name).is_some() {
                return Err(CoreError::Message(format!(
                    "provider '{name}' already exists (use `provider set` to change it)"
                )));
            }
            cfg.providers.insert(
                name.clone(),
                Provider {
                    kind,
                    base_url,
                    key,
                },
            );
            // `write_providers` validates type field rules before persisting.
            pc::write_providers(path, &cfg)?;
            Ok(format!("added provider '{name}'"))
        }
        ProviderCmd::ProviderSet {
            name,
            kind,
            base_url,
            key,
            clear_key,
        } => {
            let mut cfg = pc::load_or_default(path)?;
            {
                let p = cfg
                    .providers
                    .get_mut(&name)
                    .ok_or_else(|| CoreError::Message(format!("no such provider '{name}'")))?;
                if let Some(v) = kind {
                    p.kind = v;
                }
                if let Some(v) = base_url {
                    p.base_url = Some(v);
                }
                if clear_key {
                    p.key = None;
                } else if key.is_some() {
                    p.key = key;
                }
            }
            pc::write_providers(path, &cfg)?;
            Ok(format!("updated provider '{name}'"))
        }
        ProviderCmd::ProviderRemove(name) => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.provider(&name).is_none() {
                return Err(CoreError::Message(format!("no such provider '{name}'")));
            }
            if let Some(role) = cfg.role_referencing(&name) {
                return Err(CoreError::Message(format!(
                    "cannot remove provider '{name}': model role '{role}' references it (change that role first)"
                )));
            }
            cfg.providers.remove(&name);
            pc::write_providers(path, &cfg)?;
            Ok(format!("removed provider '{name}'"))
        }
        ProviderCmd::ModelList => {
            let cfg = pc::load_or_default(path)?;
            let mut out = String::new();
            if cfg.models.is_empty() {
                out.push_str(&format!("(no model roles defined in {})\n", path.display()));
            }
            for (role, pool) in &cfg.models {
                let members = pool
                    .members
                    .iter()
                    .map(|m| format!("{}/{}", m.provider, m.model))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("  {role:<8} [{:?}]  {members}\n", pool.strategy));
            }
            Ok(out)
        }
        ProviderCmd::ModelShow(role) => {
            let cfg = pc::load_or_default(path)?;
            match role {
                Some(r) => {
                    let pool = cfg
                        .model(&r)
                        .ok_or_else(|| CoreError::Message(format!("no such model role '{r}'")))?;
                    Ok(render_model_role(&r, pool))
                }
                None => {
                    let mut out = String::new();
                    for (r, pool) in &cfg.models {
                        out.push_str(&render_model_role(r, pool));
                    }
                    if out.is_empty() {
                        out.push_str("(no model roles defined)\n");
                    }
                    Ok(out)
                }
            }
        }
        ProviderCmd::ModelSet {
            role,
            members,
            strategy,
        } => {
            let mut cfg = pc::load_or_default(path)?;
            cfg.models
                .insert(role.clone(), ModelPool { strategy, members });
            // `write_providers` validates member references + single≠1 before persisting.
            pc::write_providers(path, &cfg)?;
            Ok(format!("set model role '{role}'"))
        }
        ProviderCmd::ModelUnset(role) => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.models.remove(&role).is_none() {
                return Err(CoreError::Message(format!("no such model role '{role}'")));
            }
            pc::write_providers(path, &cfg)?;
            Ok(format!("unset model role '{role}'"))
        }
    }
}

/// Line-based interactive editor (the non-TTY fallback path).
pub fn edit_line_based(path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    let mut map = load(path);
    loop {
        println!("\nSettings (enter a number to edit, blank to finish):");
        for (i, (key, scope, value, source)) in rows(&map).iter().enumerate() {
            println!(
                "{}",
                crate::transport::terminal_safe_multiline(&format!(
                    "  {i:>2}) [{scope:6}] {key:<26} = {value}  ({source})"
                ))
            );
        }
        print!("> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let Ok(idx) = line.parse::<usize>() else {
            println!("not a number");
            continue;
        };
        let Some(setting) = registry().get(idx) else {
            println!("out of range");
            continue;
        };
        print!("new value for {} (blank to cancel): ", setting.key);
        std::io::stdout().flush().ok();
        let mut val = String::new();
        if std::io::stdin().read_line(&mut val).is_err() {
            break;
        }
        let val = val.trim().to_string();
        if val.is_empty() {
            continue;
        }
        // Reject invalid values without saving; keep looping so the user can
        // retry (same rules as `config set`).
        if let Err(e) = validate(setting, &val) {
            println!(
                "{}",
                crate::transport::terminal_safe_multiline(&e.to_string())
            );
            continue;
        }
        map.insert((setting.scope, setting.key.to_string()), val);
        save(path, &map)?;
        println!("saved {}", setting.key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_transaction_lease_is_distinct_from_the_config_file_mutation_lease() {
        let config = PathBuf::from("root").join("config.toml");
        let config_lock = config.with_extension("toml.lock");
        let server_lock = server_transaction_resource(&config).with_extension("toml.lock");
        assert_ne!(server_lock, config_lock);
        assert!(server_lock.ends_with("server-configuration-transaction.toml.lock"));
    }

    #[test]
    fn parse_commands() {
        let v = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&v(&[])), Command::List);
        assert_eq!(
            parse(&v(&["get", "FLEETY_ADDR"])),
            Command::Get("FLEETY_ADDR".into())
        );
        assert_eq!(
            parse(&v(&["set", "FLEETY_MODEL", "gpt-4o"])),
            Command::Set("FLEETY_MODEL".into(), "gpt-4o".into())
        );
        assert_eq!(
            parse(&v(&["unset", "FLEETY_TZ"])),
            Command::Unset("FLEETY_TZ".into())
        );
        assert_eq!(parse(&v(&["edit"])), Command::Edit);
        assert_eq!(parse(&v(&["open"])), Command::Edit);
        assert_eq!(parse(&v(&["get"])), Command::Help); // missing operand
        assert_eq!(parse(&v(&["set", "X"])), Command::Help);
    }

    #[test]
    fn scoped_execution_rejects_foreign_provider_and_flat_settings() {
        let v = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(run_rendered_scoped(&v(&["provider", "list"]), Some(DAEMON_SCOPES)).is_err());
        assert!(run_rendered_scoped(
            &v(&["set", "FLEETY_ADDR", "127.0.0.1:9999"]),
            Some(DAEMON_SCOPES)
        )
        .is_err());
        assert!(
            run_rendered_scoped(&v(&["set", "FLEETY_PRESENCE", "on"]), Some(SERVER_SCOPES))
                .is_err()
        );
    }

    #[test]
    #[serial_test::serial]
    fn rows_cover_registry() {
        // `rows` reads the real process env, so isolate from other env-mutating
        // tests: run serially and clear any registry keys the ambient
        // environment (or a prior test) may have set, so every row is `default`.
        for s in registry() {
            std::env::remove_var(s.key);
        }
        let r = rows(&ConfigMap::new());
        assert_eq!(r.len(), registry().len());
        assert!(r.iter().all(|(_, _, _, source)| source == "default"));
    }

    #[test]
    fn codex_oauth_endpoints_are_hardcoded_not_config_keys() {
        // The Codex client id and endpoints are fixed constants (OpenAI's public
        // values), not config keys — they must NOT be in the registry, so
        // `fleety config` never shows or edits them.
        assert!(find("FLEETY_CODEX_CLIENT_ID").is_none());
        assert!(find("FLEETY_CODEX_AUTHORIZE_URL").is_none());
        assert!(find("FLEETY_CODEX_TOKEN_URL").is_none());
        assert!(find("FLEETY_CODEX_BACKEND_URL").is_none());
        // The values live as constants in `oauth`.
        assert!(crate::oauth::CODEX_CLIENT_ID.starts_with("app_"));
        assert!(crate::oauth::CODEX_AUTHORIZE_URL.starts_with("https://"));
        assert!(crate::oauth::CODEX_TOKEN_URL.starts_with("https://"));
        assert!(crate::oauth::CODEX_BACKEND_URL.starts_with("https://"));
    }

    #[test]
    fn presence_settings_registered_with_defaults() {
        let presence = find("FLEETY_PRESENCE").expect("FLEETY_PRESENCE registered");
        assert_eq!(presence.scope, Scope::Daemon);
        assert_eq!(presence.default, "off");
        let interval = find("FLEETY_PRESENCE_INTERVAL_SECS").expect("interval registered");
        assert_eq!(interval.default, "300");

        // Unset → resolves to the default (source = default).
        std::env::remove_var("FLEETY_PRESENCE");
        let resolved = resolve("FLEETY_PRESENCE", &ConfigMap::new()).expect("resolve");
        assert_eq!(resolved.value, "off");
        assert_eq!(resolved.source, Source::Default);
    }

    #[test]
    fn ws_liveness_settings_registered_and_validated() {
        // WS keepalive timing: the server's ping period, and the shared
        // liveness deadline (server reclaim + client read deadline).
        let ping = find("FLEETY_WS_PING_SECS").expect("FLEETY_WS_PING_SECS registered");
        assert_eq!(ping.scope, Scope::Server);
        assert_eq!(ping.default, "20");
        let timeout = find("FLEETY_WS_TIMEOUT_SECS").expect("FLEETY_WS_TIMEOUT_SECS registered");
        assert_eq!(timeout.scope, Scope::Shared);
        assert_eq!(timeout.default, "60");
        // Both take a positive integer: zero, negative, and non-numeric values
        // are rejected at write time (a zero ping period / deadline is
        // meaningless — runtime parsing falls back to the defaults instead).
        for s in [ping, timeout] {
            assert!(validate(s, "45").is_ok(), "{}: '45' should pass", s.key);
            for bad in ["0", "-5", "abc"] {
                let err = validate(s, bad).unwrap_err().to_string();
                assert!(
                    err.contains(s.key),
                    "{}: '{bad}' rejection should name the key, got: {err}",
                    s.key
                );
            }
        }
    }

    #[test]
    fn unknown_key_is_rejected() {
        assert!(find("FLEETY_NOPE").is_none());
        assert!(find("FLEETY_ADDR").is_some());
    }

    #[test]
    fn rows_in_scopes_and_ensure_scope_restrict_to_cli_owner() {
        // rows_in_scopes lists only the requested scopes.
        let rows = rows_in_scopes(&ConfigMap::new(), LOCAL_SCOPES);
        assert!(rows.iter().all(|(_, scope, _, _)| scope == "cli"));
        assert!(rows.iter().any(|(k, _, _, _)| k == "FLEETY_VOICE_AUDIO")); // a Cli key
        assert!(!rows.iter().any(|(k, _, _, _)| k == "FLEETY_ADDR")); // a Server key excluded
                                                                      // ensure_scope: a Cli/Shared key passes; a Server/Daemon key is refused
                                                                      // with direction; an unknown key is the usual unknown error.
        assert!(ensure_scope("FLEETY_VOICE_AUDIO", LOCAL_SCOPES).is_ok());
        let shared = ensure_scope("FLEETY_TZ", LOCAL_SCOPES)
            .unwrap_err()
            .to_string();
        assert!(shared.contains("daemon"), "shared key redirected: {shared}");
        let err = ensure_scope("FLEETY_ADDR", LOCAL_SCOPES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("server"), "server key redirected: {err}");
        assert!(find("FLEETY_DEVICE_ID").is_none());
        assert!(ensure_scope("FLEETY_NOPE", LOCAL_SCOPES).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn run_rendered_scoped_local_hides_and_guards_server_keys() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        std::env::set_var("FLEETY_CONFIG", &path);
        std::env::remove_var("FLEETY_ADDR");
        let v = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // Local list omits Server/Daemon/Shared keys and includes only Cli.
        let list = run_rendered_scoped(&v(&["list"]), Some(LOCAL_SCOPES)).unwrap();
        assert!(!list.contains("FLEETY_ADDR"), "server key hidden: {list}");
        assert!(!list.contains("FLEETY_TZ"), "shared key hidden: {list}");
        // Unfiltered list (server/daemon) still shows everything.
        assert!(run_rendered_scoped(&v(&["list"]), None)
            .unwrap()
            .contains("FLEETY_ADDR"));
        // Local set of a Server key is refused and writes nothing.
        assert!(run_rendered_scoped(
            &v(&["set", "FLEETY_ADDR", "0.0.0.0:8787"]),
            Some(LOCAL_SCOPES)
        )
        .is_err());
        assert!(
            !path.exists(),
            "a refused local set must not create the file"
        );
        // Local set of a Cli key works.
        run_rendered_scoped(
            &v(&["set", "FLEETY_VOICE_AUDIO", "auto"]),
            Some(LOCAL_SCOPES),
        )
        .unwrap();
        assert_eq!(
            load(&path)
                .get(&(Scope::Cli, "FLEETY_VOICE_AUDIO".to_string()))
                .map(String::as_str),
            Some("auto")
        );
        std::env::remove_var("FLEETY_CONFIG");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn owner_scope_sets_are_disjoint_and_complete() {
        for setting in registry() {
            let owner = owner_for_key(setting.key).unwrap();
            let scopes = match owner {
                Owner::Server => SERVER_SCOPES,
                Owner::Daemon => DAEMON_SCOPES,
                Owner::Cli => CLI_SCOPES,
            };
            assert!(
                scopes.contains(&setting.scope),
                "{} owner mismatch",
                setting.key
            );
        }
        assert_eq!(owner_for_key("FLEETY_ADDR").unwrap(), Owner::Server);
        assert_eq!(owner_for_key("FLEETY_TZ").unwrap(), Owner::Daemon);
        assert_eq!(owner_for_key("FLEETY_VOICE_AUDIO").unwrap(), Owner::Cli);
        assert!(owner_for_key("FLEETY_DEVICE_ID").is_err());
    }

    #[test]
    fn mutating_corrupt_config_never_overwrites() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        let original = b"{ definitely not toml";
        std::fs::write(&path, original).unwrap();
        let result = mutate_strict(&path, |map| {
            map.insert((Scope::Cli, "FLEETY_VOICE_AUDIO".into()), "auto".into());
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn concurrent_config_mutations_preserve_both_values() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        let a = path.clone();
        let b = path.clone();
        let first = std::thread::spawn(move || {
            mutate_strict(&a, |map| {
                map.insert((Scope::Cli, "FLEETY_VOICE_AUDIO".into()), "auto".into());
                Ok(())
            })
        });
        let second = std::thread::spawn(move || {
            mutate_strict(&b, |map| {
                map.insert((Scope::Daemon, "FLEETY_PRESENCE".into()), "off".into());
                Ok(())
            })
        });
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        let map = load_strict(&path).unwrap();
        assert_eq!(map.len(), 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[serial_test::serial]
    fn snapshot_entries_revision_and_strict_load() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        std::env::remove_var("FLEETY_TOKEN");
        std::env::remove_var("FLEETY_POLICY");

        // Snapshot: a secret carries no value, only is_set; enums carry choices.
        let entries = snapshot_entries(&ConfigMap::new(), None);
        let tok = entries
            .iter()
            .find(|e| e.key == "FLEETY_TOKEN")
            .expect("token entry");
        assert!(
            tok.secret && tok.value.is_empty() && !tok.is_set,
            "secret unset, no value"
        );
        let policy = entries
            .iter()
            .find(|e| e.key == "FLEETY_POLICY")
            .expect("policy entry");
        assert_eq!(policy.choices, vec!["full_access", "require_approval"]);
        assert_eq!(policy.effect, Some(ConfigEffect::Restart));

        // Revision: stable for identical content, changes when content changes.
        let mut m = ConfigMap::new();
        save(&path, &m).unwrap();
        let r1 = revision(&path);
        assert_eq!(r1, revision(&path), "stable for same content");
        m.insert(
            (Scope::Server, "FLEETY_POLICY".into()),
            "require_approval".into(),
        );
        save(&path, &m).unwrap();
        assert_ne!(revision(&path), r1, "changes when content changes");

        // load_strict round-trips; a broken file errors (not a fail-soft empty).
        assert_eq!(
            load_strict(&path)
                .unwrap()
                .get(&(Scope::Server, "FLEETY_POLICY".into()))
                .map(String::as_str),
            Some("require_approval")
        );
        std::fs::write(&path, "{ not toml ::").unwrap();
        assert!(
            load_strict(&path).is_err(),
            "broken file errors under load_strict"
        );
        assert!(load(&path).is_empty(), "load() stays fail-soft");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[serial_test::serial]
    fn addr_defaults_to_all_interfaces() {
        // Reachable across devices out of the box (paired with auth-on default).
        std::env::remove_var("FLEETY_ADDR");
        let r = resolve("FLEETY_ADDR", &ConfigMap::new()).expect("resolve");
        assert_eq!(r.value, "0.0.0.0:8787");
        assert_eq!(r.source, Source::Default);
    }

    #[test]
    #[serial_test::serial]
    fn require_auth_defaults_on() {
        // Connection auth is required by default now: unset → resolves to "1".
        std::env::remove_var("FLEETY_REQUIRE_AUTH");
        let r = resolve("FLEETY_REQUIRE_AUTH", &ConfigMap::new()).expect("resolve");
        assert_eq!(r.value, "1");
        assert_eq!(r.source, Source::Default);
        // The validator still only accepts 0/1.
        let s = find("FLEETY_REQUIRE_AUTH").expect("registered");
        assert!(validate(s, "1").is_ok());
        assert!(validate(s, "0").is_ok());
        assert!(validate(s, "yes").is_err());
    }

    #[test]
    #[serial_test::serial]
    fn agent_url_is_not_a_registry_key() {
        // The connection target moved to connections.toml (managed by
        // `fleety server`); FLEETY_AGENT_URL is no longer a registry setting.
        assert!(find("FLEETY_AGENT_URL").is_none());
        assert!(!registry().iter().any(|s| s.key == "FLEETY_AGENT_URL"));
        // `config set FLEETY_AGENT_URL <url>` is rejected as an unknown key …
        let args: Vec<String> = ["set", "FLEETY_AGENT_URL", "ws://x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = run_rendered(&args).unwrap_err().to_string();
        assert!(err.contains("unknown setting"), "got: {err}");
        // … and it is never seeded into the env from config.toml.
        std::env::remove_var("FLEETY_AGENT_URL");
        let mut map = ConfigMap::new();
        map.insert(
            (Scope::Daemon, "FLEETY_AGENT_URL".into()),
            "ws://seeded".into(),
        );
        seed_env_from_config(&map);
        assert!(
            std::env::var("FLEETY_AGENT_URL").is_err(),
            "must not seed a non-registry key"
        );
    }

    #[test]
    #[serial_test::serial]
    fn precedence_env_then_config_then_default() {
        let mut map = ConfigMap::new();
        map.insert((Scope::Server, "FLEETY_ADDR".into()), "0.0.0.0:9000".into());
        // config value used when env unset.
        std::env::remove_var("FLEETY_ADDR");
        let r = resolve("FLEETY_ADDR", &map).unwrap();
        assert_eq!(r.value, "0.0.0.0:9000");
        assert_eq!(r.source, Source::Config);
        // env wins.
        std::env::set_var("FLEETY_ADDR", "1.2.3.4:5");
        let r = resolve("FLEETY_ADDR", &map).unwrap();
        assert_eq!(r.value, "1.2.3.4:5");
        assert_eq!(r.source, Source::Env);
        std::env::remove_var("FLEETY_ADDR");
        // default when neither.
        let r = resolve("FLEETY_POLICY", &ConfigMap::new()).unwrap();
        assert_eq!(r.source, Source::Default);
        assert_eq!(r.value, "full_access");
    }

    #[test]
    #[serial_test::serial]
    fn seed_only_fills_unset_env() {
        let mut map = ConfigMap::new();
        map.insert((Scope::Shared, "FLEETY_TZ".into()), "Asia/Taipei".into());
        map.insert(
            (Scope::Server, "FLEETY_POLICY".into()),
            "require_approval".into(),
        );
        std::env::remove_var("FLEETY_TZ");
        std::env::set_var("FLEETY_POLICY", "full_access"); // already set → must not change
        seed_env_from_config(&map);
        assert_eq!(std::env::var("FLEETY_TZ").unwrap(), "Asia/Taipei");
        assert_eq!(std::env::var("FLEETY_POLICY").unwrap(), "full_access");
        std::env::remove_var("FLEETY_TZ");
        std::env::remove_var("FLEETY_POLICY");
    }

    #[test]
    fn save_load_roundtrip_and_corrupt_is_empty() {
        let dir = std::env::temp_dir().join(format!("fleety-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        let mut map = ConfigMap::new();
        map.insert((Scope::Server, "FLEETY_ADDR".into()), "0.0.0.0:9000".into());
        map.insert((Scope::Cli, "FLEETY_VOICE_AUDIO".into()), "off".into());
        save(&path, &map).unwrap();
        let loaded = load(&path);
        assert_eq!(
            loaded
                .get(&(Scope::Server, "FLEETY_ADDR".into()))
                .map(String::as_str),
            Some("0.0.0.0:9000")
        );
        assert_eq!(
            loaded
                .get(&(Scope::Cli, "FLEETY_VOICE_AUDIO".into()))
                .map(String::as_str),
            Some("off")
        );
        // corrupt → empty, no panic.
        std::fs::write(&path, "{ not toml ::").unwrap();
        assert!(load(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secrets_are_masked() {
        let token = find("FLEETY_TOKEN").unwrap();
        assert_eq!(display_value(token, "supersecret"), "********");
        let addr = find("FLEETY_ADDR").unwrap();
        assert_eq!(display_value(addr, "1.2.3.4:5"), "1.2.3.4:5");
    }

    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_providers_verbs_and_flags() {
        // provider add (api) → a typed provider.
        assert_eq!(
            parse_providers(&v(&[
                "provider",
                "add",
                "openai1",
                "--type",
                "api",
                "--base-url",
                "https://x/v1",
                "--key",
                "sk",
            ]))
            .expect("add parses"),
            ProviderCmd::ProviderAdd {
                name: "openai1".into(),
                kind: "api".into(),
                base_url: Some("https://x/v1".into()),
                key: Some("sk".into()),
            }
        );
        assert_eq!(
            parse_providers(&v(&["provider", "set", "openai1", "--clear-key"]))
                .expect("clear-key parses"),
            ProviderCmd::ProviderSet {
                name: "openai1".into(),
                kind: None,
                base_url: None,
                key: None,
                clear_key: true,
            }
        );
        assert!(parse_providers(&v(&[
            "provider",
            "set",
            "openai1",
            "--key",
            "sk",
            "--clear-key",
        ]))
        .is_err());
        // provider add (oauth) → no base_url/key.
        assert_eq!(
            parse_providers(&v(&["provider", "add", "codex1", "--type", "oauth:codex"])).unwrap(),
            ProviderCmd::ProviderAdd {
                name: "codex1".into(),
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            }
        );
        assert_eq!(
            parse_providers(&v(&["provider", "add", "--type", "oauth:codex", "codex1"]))
                .expect("options-first provider parses"),
            parse_providers(&v(&["provider", "add", "codex1", "--type", "oauth:codex"]))
                .expect("positional-first provider parses"),
            "generated options-first usage and positional-first usage must agree"
        );
        assert_eq!(
            parse_providers(&v(&["model", "set", "--member", "openai1/gpt-4o", "main",]))
                .expect("options-first model parses"),
            parse_providers(&v(&["model", "set", "main", "--member", "openai1/gpt-4o",]))
                .expect("positional-first model parses"),
            "model role placement must match generated usage"
        );
        // model set: per-member traits attach to the preceding --member; strategy pool-level.
        match parse_providers(&v(&[
            "model",
            "set",
            "main",
            "--member",
            "openai1/gpt-4o",
            "--stream",
            "--modalities",
            "text,image",
            "--member",
            "codex1/gpt-5",
            "--strategy",
            "failover",
        ]))
        .expect("model set parses")
        {
            ProviderCmd::ModelSet {
                role,
                members,
                strategy,
            } => {
                assert_eq!(role, "main");
                assert_eq!(strategy, Strategy::Failover);
                assert_eq!(members.len(), 2);
                assert_eq!(members[0].provider, "openai1");
                assert_eq!(members[0].model, "gpt-4o");
                assert!(members[0].stream);
                assert_eq!(members[0].modalities.as_deref(), Some("text,image"));
                assert_eq!(members[1].provider, "codex1");
                assert!(!members[1].stream);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // one member with no --strategy defaults to single.
        match parse_providers(&v(&[
            "model",
            "set",
            "cheap",
            "--member",
            "openai1/gpt-4o-mini",
        ]))
        .unwrap()
        {
            ProviderCmd::ModelSet {
                strategy, members, ..
            } => {
                assert_eq!(strategy, Strategy::Single);
                assert_eq!(members.len(), 1);
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // Errors: unknown verb, missing --type, bad strategy, bad --member, no members.
        assert!(parse_providers(&v(&["provider", "frobnicate"])).is_err());
        assert!(parse_providers(&v(&["provider", "add", "p"])).is_err()); // missing --type
        assert!(parse_providers(&v(&[
            "model",
            "set",
            "main",
            "--member",
            "p/m",
            "--strategy",
            "random"
        ]))
        .is_err());
        assert!(parse_providers(&v(&["model", "set", "main", "--member", "no-slash"])).is_err());
        assert!(parse_providers(&v(&["model", "set", "main", "--strategy", "single"])).is_err());
    }

    #[test]
    fn config_effect_by_verb() {
        let eff = |p: &[&str]| config_effect(&v(p));
        // providers.toml mutations → next connection.
        assert_eq!(
            eff(&["provider", "add", "p", "--type", "api", "--base-url", "u"]),
            Some(ConfigEffect::NextConnection)
        );
        assert_eq!(
            eff(&[
                "model",
                "set",
                "main",
                "--member",
                "p/m",
                "--strategy",
                "single"
            ]),
            Some(ConfigEffect::NextConnection)
        );
        // flat set/unset → restart.
        assert_eq!(
            eff(&["set", "FLEETY_MODEL", "gpt-5"]),
            Some(ConfigEffect::Restart)
        );
        assert_eq!(eff(&["unset", "FLEETY_ADDR"]), Some(ConfigEffect::Restart));
        assert_eq!(
            eff(&["set", "FLEETY_POLICY", "require_approval"]),
            Some(ConfigEffect::Restart)
        );
        // reads → none.
        assert_eq!(eff(&["list"]), None);
        assert_eq!(eff(&["get", "FLEETY_MODEL"]), None);
        assert_eq!(eff(&["provider", "list"]), None);
        assert_eq!(eff(&["model", "list"]), None);
    }

    #[test]
    fn run_providers_add_model_and_guard_removal() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        // Add two api providers, then a model role pooling them.
        run_providers_at(
            &path,
            &v(&[
                "provider",
                "add",
                "p1",
                "--type",
                "api",
                "--base-url",
                "https://u1/v1",
            ]),
        )
        .unwrap();
        run_providers_at(
            &path,
            &v(&[
                "provider",
                "add",
                "p2",
                "--type",
                "api",
                "--base-url",
                "https://u2/v1",
            ]),
        )
        .unwrap();
        run_providers_at(
            &path,
            &v(&[
                "model",
                "set",
                "main",
                "--member",
                "p1/gpt-4o",
                "--member",
                "p2/gpt-4o",
                "--strategy",
                "failover",
            ]),
        )
        .unwrap();
        // Adding a duplicate name fails.
        assert!(run_providers_at(
            &path,
            &v(&[
                "provider",
                "add",
                "p1",
                "--type",
                "api",
                "--base-url",
                "https://x/v1"
            ])
        )
        .is_err());
        // A model member referencing an undefined provider is rejected on write.
        assert!(run_providers_at(
            &path,
            &v(&[
                "model",
                "set",
                "cheap",
                "--member",
                "ghost/x",
                "--strategy",
                "single"
            ])
        )
        .is_err());
        // A referenced provider can't be removed (the main role names it).
        let err = run_providers_at(&path, &v(&["provider", "remove", "p1"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("model role 'main'"), "got: {err}");
        // The file still parses with both providers + the role.
        let cfg = pc::load_from(&path).expect("re-read");
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.models.len(), 1);
        assert_eq!(cfg.model("main").unwrap().members.len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn registry_validators_accept_and_reject() {
        // Every key that carries a validator accepts a representative good value
        // and rejects a bad one, and the rejection names the key.
        let cases: &[(&str, &str, &str)] = &[
            ("FLEETY_POLICY", "require_approval", "nope"),
            ("FLEETY_FS_SCOPE", "workspace", "ful"),
            ("FLEETY_VOICE_AUDIO", "on", "loud"),
            ("FLEETY_PRESENCE", "on", "maybe"),
            ("FLEETY_MODEL_EFFORT", "high", "extreme"),
            ("FLEETY_CHEAP_MODEL_EFFORT", "low", "extreme"),
            ("FLEETY_REQUIRE_AUTH", "1", "abc"),
            ("FLEETY_AUTO_INSTALL_DEPS", "0", "true"),
            ("FLEETY_FORCE_SSE", "1", "yes"),
            ("FLEETY_DISABLE_SSE", "0", "no"),
            ("FLEETY_MODEL_RETRIES", "3", "-1"),
            ("FLEETY_MODEL_RETRY_BASE_MS", "500", "fast"),
            ("FLEETY_MODEL_RETRY_CAP_MS", "30000", "1.5"),
            ("FLEETY_CMD_TIMEOUT_SECS", "120", "notanumber"),
            ("FLEETY_SSE_TIMEOUT_SECS", "45", "-5"),
            ("FLEETY_WS_PING_SECS", "20", "0"),
            ("FLEETY_WS_TIMEOUT_SECS", "60", "-5"),
            ("FLEETY_BACKUP_INTERVAL_SECS", "3600", "hourly"),
            ("FLEETY_PRESENCE_INTERVAL_SECS", "300", "5m"),
            ("FLEETY_VOICE_AUDIO_MAX_KB", "2048", "big"),
            ("FLEETY_MODEL_BASE_URL", "https://api.x/v1", "notaurl"),
        ];
        for &(key, good, bad) in cases {
            let s = find(key).unwrap_or_else(|| panic!("{key} registered"));
            assert!(s.validator.is_some(), "{key} should carry a validator");
            assert!(validate(s, good).is_ok(), "{key}: '{good}' should pass");
            let err = validate(s, bad).unwrap_err().to_string();
            assert!(
                err.contains(key),
                "{key}: error should name the key, got: {err}"
            );
        }
        // A key with no validator accepts anything (pass-through).
        let tz = find("FLEETY_TZ").unwrap();
        assert!(tz.validator.is_none());
        assert!(validate(tz, "Anything/Here").is_ok());
    }

    #[test]
    fn validate_error_names_accepted_values() {
        // Enum rejection lists the members …
        let voice = find("FLEETY_VOICE_AUDIO").unwrap();
        let err = validate(voice, "loud").unwrap_err().to_string();
        assert!(err.contains("FLEETY_VOICE_AUDIO"), "got: {err}");
        assert!(
            err.contains("auto") && err.contains("on") && err.contains("off"),
            "got: {err}"
        );
        // … and URL rejection states the required scheme.
        let url = find("FLEETY_MODEL_BASE_URL").unwrap();
        let err = validate(url, "notaurl").unwrap_err().to_string();
        assert!(err.contains("FLEETY_MODEL_BASE_URL"), "got: {err}");
        assert!(err.contains("http"), "got: {err}");
        // Pass-through (no validator) and unset (empty) both accept silently.
        let model = find("FLEETY_MODEL").unwrap();
        assert!(validate(model, "anything").is_ok());
        assert!(validate(voice, "").is_ok());
    }

    #[test]
    #[serial_test::serial]
    fn run_rendered_set_validates_before_writing() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        std::env::set_var("FLEETY_CONFIG", &path);
        // Don't let a real env var shadow the keys we read back.
        std::env::remove_var("FLEETY_REQUIRE_AUTH");
        std::env::remove_var("FLEETY_POLICY");

        // Invalid value → error, and the file is never created.
        let err = run_rendered(&v(&["set", "FLEETY_REQUIRE_AUTH", "abc"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("FLEETY_REQUIRE_AUTH"), "got: {err}");
        assert!(
            !path.exists(),
            "an invalid set must not create the config file"
        );

        // Valid value → written under the setting's scope and readable back.
        run_rendered(&v(&["set", "FLEETY_POLICY", "require_approval"])).unwrap();
        let map = load(&path);
        assert_eq!(
            map.get(&(Scope::Server, "FLEETY_POLICY".to_string()))
                .map(String::as_str),
            Some("require_approval")
        );

        std::env::remove_var("FLEETY_CONFIG");
        let _ = std::fs::remove_file(&path);
    }
}
