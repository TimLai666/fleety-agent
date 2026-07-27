//! `~/.fleety/connections.toml` — this device's server connection profiles.
//!
//! The single persistent source of "which server this device connects to" (and
//! the token for it), shared by the CLI and the daemon so the window (`fleety`)
//! and the hand (`fleetyd`) on one machine always target the same server. It
//! replaces the old three-way split — config.json (`agent_url`/`token`),
//! `config.toml`'s `FLEETY_AGENT_URL`, and each binary's own
//! env→mDNS→localhost guesswork — with one file and one [`resolve`] precedence.
//!
//! The file may hold tokens, so [`save_at`] writes it `0600` (owner-only) on
//! Unix. A missing file is a fresh device (empty [`Connections`]); a present but
//! unparseable file is a hard error — we never silently treat a corrupt file as
//! empty and drift off the configured server.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_core::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// The connect target when nothing is configured and mDNS finds nothing.
pub const DEFAULT_URL: &str = "ws://127.0.0.1:8787";

/// Validate an explicitly selected Server endpoint before it can replace a
/// profile URL or receive a pairing credential.
pub fn validate_ws_url(url: &str) -> Result<()> {
    if url.chars().any(char::is_control) {
        return Err(CoreError::Message(
            "server url cannot contain terminal control characters".to_string(),
        ));
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        CoreError::Message("server url is invalid (e.g. ws://192.168.1.10:8787)".to_string())
    })?;
    if matches!(parsed.scheme(), "http" | "https") {
        Err(CoreError::Message(
            "server url is http(s) — use the WebSocket scheme (ws:// or wss://)".to_string(),
        ))
    } else if !matches!(parsed.scheme(), "ws" | "wss") || parsed.host_str().is_none() {
        Err(CoreError::Message(
            "server url must contain a host and use ws:// or wss:// (e.g. ws://192.168.1.10:8787)"
                .to_string(),
        ))
    } else if !parsed.username().is_empty() || parsed.password().is_some() {
        Err(CoreError::Message(
            "server url cannot contain credentials; pair the profile to store authentication safely"
                .to_string(),
        ))
    } else if parsed.fragment().is_some() {
        Err(CoreError::Message(
            "server url cannot contain a fragment; use only the WebSocket endpoint path and optional query"
                .to_string(),
        ))
    } else {
        Ok(())
    }
}

/// One named server: its WebSocket URL, optional paired token, human label,
/// server fingerprint pinned at enrollment, and opaque lifecycle generation.
/// A credentialed, URL-less profile requires explicit endpoint selection and
/// re-pairing; unsigned mDNS metadata cannot restore its credential binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generation: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            url: String::new(),
            token: None,
            label: None,
            fingerprint: None,
            generation: uuid::Uuid::new_v4().to_string(),
        }
    }
}

/// The whole `connections.toml`: this device's id, the current profile name, and
/// the named profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connections {
    #[serde(default)]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
}

impl Connections {
    /// The current profile, if `current` names an existing one.
    pub fn current_profile(&self) -> Option<&Profile> {
        self.current.as_ref().and_then(|n| self.profiles.get(n))
    }

    /// This device's id: the stored value if set, else the machine-derived id
    /// (so a fresh device with no `connections.toml` still has a stable id).
    pub fn effective_device_id(&self) -> String {
        if self.device_id.is_empty() {
            crate::device::device_id()
        } else {
            self.device_id.clone()
        }
    }
}

/// The `connections.toml` path (`FLEETY_CONNECTIONS` override, else
/// `~/.fleety/connections.toml`).
pub fn connections_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_CONNECTIONS") {
        return PathBuf::from(p);
    }
    fleety_dir().join("connections.toml")
}

/// `~/.fleety` (HOME/USERPROFILE based), the shared per-user Fleety directory.
pub fn fleety_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".fleety")
}

/// Load connections from `path`. A missing file yields an empty [`Connections`]
/// (a fresh device); a present-but-unparseable file is an **error** — we never
/// silently treat a corrupt file as empty and drift off the configured server.
pub fn load_at(path: &Path) -> Result<Connections> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| {
            CoreError::Message(format!(
                "invalid connections.toml at {} ({e}); fix or remove it — Fleety will not \
                 guess a server while it is unreadable",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Connections::default()),
        Err(e) => Err(CoreError::Message(format!(
            "cannot read connections.toml at {}: {e}",
            path.display()
        ))),
    }
}

/// Load the connections from [`connections_path`] (see [`load_at`]).
pub fn load() -> Result<Connections> {
    load_at(&connections_path())
}

fn sync_published_connections(_path: &Path, dir: Option<&Path>) -> std::io::Result<()> {
    #[cfg(windows)]
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(_path)?
        .sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(dir.unwrap_or_else(|| Path::new(".")))?.sync_all()?;
    Ok(())
}

enum SaveStatus {
    Durable,
    PublishedNotDurable(CoreError),
}

fn save_at_with_sync_status<S, P>(
    path: &Path,
    conns: &Connections,
    sync_staged: S,
    sync_published: P,
) -> Result<SaveStatus>
where
    S: FnOnce(&std::fs::File) -> std::io::Result<()>,
    P: FnOnce(&Path, Option<&Path>) -> std::io::Result<()>,
{
    let text = toml::to_string_pretty(conns)
        .map_err(|e| CoreError::Message(format!("serialize connections.toml: {e}")))?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(d) = dir {
        std::fs::create_dir_all(d)
            .map_err(|e| CoreError::Message(format!("cannot create ~/.fleety: {e}")))?;
    }
    let tmp_name = format!(".connections-{}.tmp", uuid::Uuid::new_v4());
    let tmp = match dir {
        Some(d) => d.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| CoreError::Message(format!("create connections.toml temp file: {e}")))?;
    set_owner_only(&tmp);
    use std::io::Write;
    if let Err(error) = file
        .write_all(text.as_bytes())
        .and_then(|()| sync_staged(&file))
    {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Message(format!(
            "write and sync connections.toml: {error}"
        )));
    }
    drop(file);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Message(format!("replace connections.toml: {e}")));
    }
    match sync_published(path, dir) {
        Ok(()) => Ok(SaveStatus::Durable),
        Err(error) => Ok(SaveStatus::PublishedNotDurable(CoreError::Message(
            format!("sync published connections.toml: {error}"),
        ))),
    }
}

fn save_at_with_sync<S, P>(
    path: &Path,
    conns: &Connections,
    sync_staged: S,
    sync_published: P,
) -> Result<()>
where
    S: FnOnce(&std::fs::File) -> std::io::Result<()>,
    P: FnOnce(&Path, Option<&Path>) -> std::io::Result<()>,
{
    match save_at_with_sync_status(path, conns, sync_staged, sync_published)? {
        SaveStatus::Durable => Ok(()),
        SaveStatus::PublishedNotDurable(error) => Err(error),
    }
}

/// Persist connections to `path` atomically and durably: sync a private temp
/// file before replacement, then sync the published file/directory metadata.
/// Unix permissions remain `0600` because the file may hold bearer tokens.
pub fn save_at(path: &Path, conns: &Connections) -> Result<()> {
    let mut conns = conns.clone();
    ensure_profile_generations(&mut conns);
    save_at_with_sync(
        path,
        &conns,
        std::fs::File::sync_all,
        sync_published_connections,
    )
}

/// Persist connections to [`connections_path`] (see [`save_at`]).
pub fn save(conns: &Connections) -> Result<()> {
    save_at(&connections_path(), conns)
}

/// Retry the durability barrier for an already-published connections file.
/// Callers use this after a post-rename sync error before exposing a success
/// that depends on the visible credential generation.
pub fn sync_connections_publication_at(path: &Path) -> Result<()> {
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    sync_published_connections(path, dir).map_err(|error| {
        CoreError::Message(format!(
            "sync published connections.toml at {}: {error}",
            path.display()
        ))
    })
}

pub fn sync_connections_publication() -> Result<()> {
    sync_connections_publication_at(&connections_path())
}

/// Retry the durability barrier only while the canonical profile still
/// matches the exact generation that became visible in the original commit.
/// The mutation lease closes the check-to-sync race with other Fleety writers.
pub fn sync_resolved_profile_publication(target: &Resolved) -> Result<()> {
    sync_resolved_profile_publication_at(&connections_path(), target)
}

pub fn sync_resolved_profile_publication_at(path: &Path, target: &Resolved) -> Result<()> {
    let _lease = acquire_mutation_lease(path)?;
    let connections = load_at(path)?;
    validate_resolved_profile_owner(
        &connections,
        target,
        "credential publication durability retry",
    )?;
    sync_connections_publication_at(path)
}

struct MutationLease {
    path: PathBuf,
    owner: String,
}

impl Drop for MutationLease {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|owner| owner == self.owner) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn acquire_mutation_lease(path: &Path) -> Result<MutationLease> {
    let lock_path = path.with_extension("toml.lock");
    let owner = format!("{}:{}", std::process::id(), uuid::Uuid::new_v4());
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CoreError::Message(format!("cannot create connection dir: {error}"))
        })?;
    }
    let started = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                use std::io::Write;
                if let Err(error) = file
                    .write_all(owner.as_bytes())
                    .and_then(|()| file.sync_all())
                {
                    let _ = std::fs::remove_file(&lock_path);
                    return Err(CoreError::Message(format!(
                        "cannot publish connection lock owner: {error}"
                    )));
                }
                return Ok(MutationLease {
                    path: lock_path,
                    owner: owner.clone(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if started.elapsed() >= Duration::from_secs(5) {
                    return Err(CoreError::Message(format!(
                        "timed out waiting to update {} — another Fleety process is changing \
                         connection profiles; remove the lock only after confirming its owner \
                         process has exited",
                        path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(CoreError::Message(format!(
                    "cannot lock {} for update: {error}",
                    path.display()
                )))
            }
        }
    }
}

fn ensure_profile_generations(connections: &mut Connections) -> bool {
    let mut changed = false;
    for profile in connections.profiles.values_mut() {
        if profile.generation.trim().is_empty() {
            profile.generation = uuid::Uuid::new_v4().to_string();
            changed = true;
        }
    }
    changed
}

/// Cross-process-safe read-modify-write for the shared connection profile
/// store. Callers must update only the fields they own and validate any
/// expected URL/current preconditions inside `mutation`.
pub fn mutate_at<T>(
    path: &Path,
    mutation: impl FnOnce(&mut Connections) -> Result<T>,
) -> Result<T> {
    let _lease = acquire_mutation_lease(path)?;
    let mut connections = load_at(path)?;
    let result = mutation(&mut connections)?;
    save_at(path, &connections)?;
    Ok(result)
}

fn mutate_at_with_status_with_sync<T, P>(
    path: &Path,
    mutation: impl FnOnce(&mut Connections) -> Result<T>,
    sync_published: P,
) -> Result<(T, SaveStatus)>
where
    P: FnOnce(&Path, Option<&Path>) -> std::io::Result<()>,
{
    let _lease = acquire_mutation_lease(path)?;
    let mut connections = load_at(path)?;
    let result = mutation(&mut connections)?;
    ensure_profile_generations(&mut connections);
    let status =
        save_at_with_sync_status(path, &connections, std::fs::File::sync_all, sync_published)?;
    Ok((result, status))
}

pub fn mutate<T>(mutation: impl FnOnce(&mut Connections) -> Result<T>) -> Result<T> {
    mutate_at(&connections_path(), mutation)
}

/// Persist a lifecycle generation only for the durable profile that an
/// upcoming resolution may own. Raw URL and environment targets deliberately
/// bypass this upgrade so transient or failing commands remain side-effect
/// free with respect to saved profiles.
pub fn ensure_resolvable_profile_generation(
    target: &Target,
    environment_url_present: bool,
) -> Result<bool> {
    ensure_resolvable_profile_generation_at(&connections_path(), target, environment_url_present)
}

pub fn ensure_resolvable_profile_generation_at(
    path: &Path,
    target: &Target,
    environment_url_present: bool,
) -> Result<bool> {
    let existing = load_at(path)?;
    let existing_name = match target {
        Target::Named(name) => Some(name.as_str()),
        Target::Current if !environment_url_present => existing.current.as_deref(),
        Target::Current | Target::Url(_) => None,
    };
    let Some(existing_name) = existing_name else {
        return Ok(false);
    };
    if existing
        .profiles
        .get(existing_name)
        .is_none_or(|profile| !profile.generation.trim().is_empty())
    {
        return Ok(false);
    }

    let _lease = acquire_mutation_lease(path)?;
    let mut connections = load_at(path)?;
    let name = match target {
        Target::Named(name) => Some(name.clone()),
        Target::Current if !environment_url_present => connections.current.clone(),
        Target::Current | Target::Url(_) => None,
    };
    let Some(name) = name else {
        return Ok(false);
    };
    let Some(profile) = connections.profiles.get_mut(&name) else {
        return Ok(false);
    };
    if !profile.generation.trim().is_empty() {
        return Ok(false);
    }
    profile.generation = uuid::Uuid::new_v4().to_string();
    save_at_with_sync(
        path,
        &connections,
        std::fs::File::sync_all,
        sync_published_connections,
    )?;
    Ok(true)
}

/// Result of a connection-store mutation whose atomic replacement became
/// visible but whose final file/directory publication sync may have failed.
/// Callers must not tell users to repeat a one-time external action when
/// `PublishedNotDurable` is returned: the new generation is already readable.
#[derive(Debug)]
pub enum MutationCommit<T> {
    Durable(T),
    PublishedNotDurable { value: T, error: CoreError },
}

pub fn mutate_recoverable<T>(
    mutation: impl FnOnce(&mut Connections) -> Result<T>,
) -> Result<MutationCommit<T>> {
    let (value, status) =
        mutate_at_with_status_with_sync(&connections_path(), mutation, sync_published_connections)?;
    Ok(match status {
        SaveStatus::Durable => MutationCommit::Durable(value),
        SaveStatus::PublishedNotDurable(error) => {
            MutationCommit::PublishedNotDurable { value, error }
        }
    })
}

/// Inspect the connection store while holding the same cross-process lease used
/// by mutations. This lets callers make an external commit conditional on an
/// owner snapshot without a mutation slipping between the check and commit.
pub fn inspect_locked_at<T>(
    path: &Path,
    inspection: impl FnOnce(&Connections) -> Result<T>,
) -> Result<T> {
    let _lease = acquire_mutation_lease(path)?;
    let connections = load_at(path)?;
    inspection(&connections)
}

pub fn inspect_locked<T>(inspection: impl FnOnce(&Connections) -> Result<T>) -> Result<T> {
    inspect_locked_at(&connections_path(), inspection)
}

/// The outcome of a [`migrate_from_config_json_at`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Migration {
    /// A `config.json` was migrated into a fresh `connections.toml`.
    Migrated,
    /// `connections.toml` already existed (or another concurrent starter won the
    /// latch) — nothing was migrated. Idempotent.
    AlreadyPresent,
    /// No `config.json` to migrate (a fresh device); nothing was written.
    NothingToMigrate,
}

/// One-time, idempotent migration of the legacy `~/.fleety/config.json`
/// (`agent_url`/`token`/`device_id`) and `~/.fleety/fleetyd.token` (the daemon's
/// persisted token) into `connections.toml`, in `dir`.
///
/// - **Idempotent:** if `connections.toml` already exists, returns
///   [`Migration::AlreadyPresent`] without touching anything.
/// - **device_id lock:** the new file's `device_id` is taken verbatim from
///   `config.json` when present, so it is never overwritten by a
///   hostname/machine-derived guess; only a `config.json` without one falls back
///   to the machine id.
/// - **Token consolidation:** the token is `config.json`'s `token` if present,
///   else the `fleetyd.token` contents — so a daemon-only-paired device keeps
///   authenticating after the migration.
/// - **url-less records:** a token but no `agent_url` yields a `default` profile
///   with an **empty** `url`. The token is preserved for recovery, but resolution
///   requires an explicit endpoint selection and re-pair instead of mDNS.
/// - **Backup, not delete:** the old `config.json` / `fleetyd.token` are renamed
///   to `*.migrated` (kept for rollback), never removed.
/// - **Concurrency-safe:** `connections.toml` is created with an `O_EXCL`
///   (`create_new`) latch, so the CLI and a same-host daemon starting at once
///   cannot each migrate and mint two device ids — the loser leaves the winner's
///   file untouched.
pub fn migrate_from_config_json_at(dir: &Path) -> Result<Migration> {
    let conns_path = dir.join("connections.toml");
    // Migration participates in the same cross-process transaction as every
    // normal profile mutation. Without this lease, a starter could read the
    // legacy files while another process creates/updates connections.toml and
    // then replace that newer state with its migration snapshot.
    let _lease = acquire_mutation_lease(&conns_path)?;
    if conns_path.exists() {
        load_at(&conns_path)?;
        return Ok(Migration::AlreadyPresent);
    }
    let config_json = dir.join("config.json");
    let daemon_token_path = dir.join("fleetyd.token");
    let cfg_text = std::fs::read_to_string(&config_json).ok();
    let daemon_token = std::fs::read_to_string(&daemon_token_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if cfg_text.is_none() && daemon_token.is_none() {
        return Ok(Migration::NothingToMigrate);
    }

    // Best-effort parse: a broken config.json still migrates to a device_id +
    // default profile rather than blocking startup.
    let value: serde_json::Value = cfg_text
        .as_deref()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let get = |k: &str| {
        value
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let cfg_device_id = get("device_id");
    let agent_url = get("agent_url");
    // config.json's token wins; otherwise the daemon's persisted token.
    let token = get("token").or(daemon_token);

    let device_id = cfg_device_id.unwrap_or_else(crate::device::device_id);
    let mut conns = Connections {
        device_id,
        current: None,
        profiles: BTreeMap::new(),
    };
    // Only mint a `default` profile when there is something to point at. A
    // A url-less record keeps `url` empty so recovery never guesses localhost.
    if agent_url.is_some() || token.is_some() {
        conns.profiles.insert(
            "default".to_string(),
            Profile {
                url: agent_url.unwrap_or_default(),
                token,
                label: None,
                fingerprint: None,
                generation: uuid::Uuid::new_v4().to_string(),
            },
        );
        conns.current = Some("default".to_string());
    }

    let toml_text = toml::to_string_pretty(&conns)
        .map_err(|e| CoreError::Message(format!("serialize connections.toml: {e}")))?;

    if let Some(parent) = conns_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create ~/.fleety: {e}")))?;
    }
    // O_EXCL latch — the first creator wins; a concurrent starter that lost the
    // race gets AlreadyExists and must not re-migrate or rename config.json.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&conns_path)
    {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(toml_text.as_bytes())
                .map_err(|e| CoreError::Message(format!("write connections.toml: {e}")))?;
            drop(f);
            set_owner_only(&conns_path);
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(Migration::AlreadyPresent);
        }
        Err(e) => return Err(CoreError::Message(format!("create connections.toml: {e}"))),
    }

    // Back up (never delete) the old files so the migration is reversible.
    let _ = std::fs::rename(&config_json, dir.join("config.json.migrated"));
    let _ = std::fs::rename(&daemon_token_path, dir.join("fleetyd.token.migrated"));
    Ok(Migration::Migrated)
}

/// One-time migration in `~/.fleety` (see [`migrate_from_config_json_at`]).
pub fn migrate_from_config_json() -> Result<Migration> {
    migrate_from_config_json_at(&fleety_dir())
}

/// Which server a single command / connection targets, overriding the persisted
/// current profile for this one resolution.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Target {
    /// No override — use the current profile, then trusted local fallback.
    #[default]
    Current,
    /// `--profile <name>`: use that named profile (its url + token).
    Named(String),
    /// Legacy `-s/--server <ws>` or `--url <ws>`: connect directly without
    /// persistence or saved-profile credential provenance.
    Url(String),
}

/// A server discovered on the LAN: its url and optional, untrusted fingerprint
/// hint. TXT metadata is never sufficient proof for sending a stored token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub url: String,
    pub fingerprint: Option<String>,
}

/// A candidate supplied to operational resolution. Same-host loopback is a
/// trusted local target; LAN mDNS remains an untrusted display/selection hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionCandidate {
    TrustedLocal(TrustedLocalUrl),
    Mdns(Discovered),
}

/// A URL proven to name a numeric loopback address. Its field is private so a
/// caller cannot label an arbitrary LAN URL as trusted local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedLocalUrl(String);

impl TrustedLocalUrl {
    pub fn parse(url: String) -> Option<Self> {
        validate_ws_url(&url).ok()?;
        let parsed = reqwest::Url::parse(&url).ok()?;
        let host = parsed.host_str()?.parse::<std::net::IpAddr>().ok()?;
        host.is_loopback().then_some(Self(url))
    }
}

/// Prefer a verified loopback candidate and avoid running the mDNS probe when
/// it exists. LAN discovery remains explicitly tagged as untrusted.
pub fn prefer_trusted_local_candidate(
    local: impl FnOnce() -> Option<String>,
    mdns: impl FnOnce() -> Option<Discovered>,
) -> Option<ResolutionCandidate> {
    if let Some(local) = local().and_then(TrustedLocalUrl::parse) {
        return Some(ResolutionCandidate::TrustedLocal(local));
    }
    mdns().map(ResolutionCandidate::Mdns)
}

/// Return `url` only when it is a verified numeric loopback WebSocket endpoint
/// with a listening TCP server.
pub fn trusted_local_server_up(url: &str, timeout: Duration) -> Option<String> {
    TrustedLocalUrl::parse(url.to_string())?;
    let parsed = reqwest::Url::parse(url).ok()?;
    let ip = parsed.host_str()?.parse::<std::net::IpAddr>().ok()?;
    let port = parsed.port_or_known_default()?;
    std::net::TcpStream::connect_timeout(&std::net::SocketAddr::new(ip, port), timeout).ok()?;
    Some(url.to_string())
}

/// Where a resolved connection target came from, so callers can surface the
/// right env, profile, trusted-local, or default hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A single-shot named profile override.
    OverrideProfile(String),
    /// A single-shot raw URL override.
    OverrideUrl,
    /// The `FLEETY_AGENT_URL` env var (temporary; never written).
    Env,
    /// The current profile (carries its name).
    Profile(String),
    /// A same-host loopback server detected by the CLI.
    Local,
    /// The built-in localhost default.
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileOwnerSnapshot {
    name: String,
    profile: Profile,
    require_current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenProvenance {
    None,
    CallerExplicit,
    SavedProfile,
}

/// A resolved connection target: the url, the token to authenticate with (if
/// any), and where it came from. The `(url, token)` pair is the connect input.
/// Display provenance, credential provenance, and durable mutation authority
/// are deliberately inseparable outside this module. Callers can inspect them,
/// but only [`resolve`] can issue a saved-profile owner capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    url: String,
    token: Option<String>,
    source: Source,
    owner: Option<ProfileOwnerSnapshot>,
    diagnostic_owner: Option<ProfileOwnerSnapshot>,
    token_provenance: TokenProvenance,
    fresh_default_owner: bool,
}

impl Resolved {
    /// Construct a target with display provenance but no durable owner
    /// authority. This is appropriate for tests, transient endpoints, and
    /// pre-persistence UI context; profile mutation helpers will reject it.
    pub fn unowned(url: String, token: Option<String>, source: Source) -> Self {
        let token = token.filter(|value| !value.is_empty());
        let token_provenance = if token.is_some() {
            TokenProvenance::CallerExplicit
        } else {
            TokenProvenance::None
        };
        Self {
            url,
            token,
            source,
            owner: None,
            diagnostic_owner: None,
            token_provenance,
            fresh_default_owner: false,
        }
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn url_owned(&self) -> String {
        self.url.clone()
    }

    pub fn token_owned(&self) -> Option<String> {
        self.token.clone()
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Whether the bytes sent in `Hello` came from the frozen saved profile.
    /// String equality is insufficient because a caller-explicit override may
    /// intentionally have the same value.
    pub fn sent_saved_profile_token(&self) -> bool {
        self.token_provenance == TokenProvenance::SavedProfile && self.token.is_some()
    }

    pub fn sent_caller_explicit_token(&self) -> bool {
        self.token_provenance == TokenProvenance::CallerExplicit && self.token.is_some()
    }

    pub fn has_profile_owner(&self) -> bool {
        self.owner.is_some()
    }

    pub fn profile_owner_name(&self) -> Option<&str> {
        self.owner.as_ref().map(|owner| owner.name.as_str())
    }

    pub fn profile_owner_fingerprint(&self) -> Option<&str> {
        self.owner
            .as_ref()
            .or(self.diagnostic_owner.as_ref())
            .and_then(|owner| owner.profile.fingerprint.as_deref())
    }

    pub fn has_profile_identity_expectation(&self) -> bool {
        self.owner.is_some() || self.diagnostic_owner.is_some()
    }

    pub fn can_create_fresh_default_owner(&self) -> bool {
        self.fresh_default_owner
    }

    /// Keep the resolved endpoint and credential for diagnostics while
    /// removing every capability that could authorize profile mutation.
    pub fn into_read_only(mut self) -> Self {
        if let Some(owner) = self.owner.take() {
            self.diagnostic_owner = Some(owner);
        }
        self.fresh_default_owner = false;
        self
    }
}

/// Resolve which server (and token) to connect to, by the single precedence
/// shared between the CLI and the daemon:
///
/// 1. `over` — a single-shot named `--profile` or raw `--server`/`--url` override.
/// 2. `env_url` — `FLEETY_AGENT_URL` (temporary; never written back). Like a
///    raw URL override, it never inherits saved-profile credentials.
/// 3. the current profile's url + token (**sticky**: once set, mDNS is skipped).
/// 4. mDNS discovery for display/selection only. A discovered advertiser never
///    becomes an operational target; the user must select it through
///    `fleety init` so enrollment owns the endpoint.
/// 5. the localhost default ([`DEFAULT_URL`]).
///
/// `env_token` (`FLEETY_TOKEN`) is an explicit token override that wins in every
/// explicit/sticky branch. `discovery` is injected so resolution is pure and
/// unit-testable; it is invoked at most once after explicit and sticky targets
/// are exhausted. Errors when `over` names a missing/url-less profile, when a
/// credentialed current profile has no endpoint to bind its token to, or when
/// mDNS finds an advertiser that still needs explicit selection. This function
/// performs no I/O of its own (it never writes), so an `env`/`--url` override
/// cannot mutate the persisted profiles.
pub fn resolve(
    conns: &Connections,
    over: &Target,
    env_url: Option<String>,
    env_token: Option<String>,
    discovery: impl FnOnce() -> Option<ResolutionCandidate>,
) -> Result<Resolved> {
    let env_token = env_token.filter(|s| !s.is_empty());
    match over {
        Target::Named(name) => {
            let p = conns.profiles.get(name).ok_or_else(|| {
                CoreError::Message(format!(
                    "no server profile named '{name}' — see `fleety server list`"
                ))
            })?;
            if p.url.is_empty() {
                return Err(CoreError::Message(format!(
                    "server profile '{name}' has no url; set one with \
                     `fleety server set-url {name} <ws-url>`"
                )));
            }
            validate_ws_url(&p.url)?;
            let (token, token_provenance) = match env_token.clone() {
                Some(token) => (Some(token), TokenProvenance::CallerExplicit),
                None => (
                    p.token.clone(),
                    if p.token.is_some() {
                        TokenProvenance::SavedProfile
                    } else {
                        TokenProvenance::None
                    },
                ),
            };
            return Ok(Resolved {
                url: p.url.clone(),
                token,
                source: Source::OverrideProfile(name.clone()),
                owner: Some(ProfileOwnerSnapshot {
                    name: name.clone(),
                    profile: p.clone(),
                    require_current: false,
                }),
                diagnostic_owner: None,
                token_provenance,
                fresh_default_owner: false,
            });
        }
        Target::Url(u) => {
            validate_ws_url(u)?;
            return Ok(Resolved::unowned(u.clone(), env_token, Source::OverrideUrl));
        }
        Target::Current => {}
    }

    if let Some(u) = env_url.filter(|s| !s.is_empty()) {
        validate_ws_url(&u)?;
        return Ok(Resolved::unowned(u, env_token, Source::Env));
    }

    // Sticky: once the current profile has a url, return it and never query mDNS
    // — an enrolled device does not drift to a LAN advertiser.
    if let Some(name) = conns.current.as_ref() {
        if let Some(p) = conns.profiles.get(name) {
            if !p.url.is_empty() {
                validate_ws_url(&p.url)?;
                let (token, token_provenance) = match env_token.clone() {
                    Some(token) => (Some(token), TokenProvenance::CallerExplicit),
                    None => (
                        p.token.clone(),
                        if p.token.is_some() {
                            TokenProvenance::SavedProfile
                        } else {
                            TokenProvenance::None
                        },
                    ),
                };
                return Ok(Resolved {
                    url: p.url.clone(),
                    token,
                    source: Source::Profile(name.clone()),
                    owner: Some(ProfileOwnerSnapshot {
                        name: name.clone(),
                        profile: p.clone(),
                        require_current: true,
                    }),
                    diagnostic_owner: None,
                    token_provenance,
                    fresh_default_owner: false,
                });
            }
            if p.token.as_deref().is_some_and(|token| !token.is_empty()) {
                return Err(CoreError::Message(explicit_repair_guidance()));
            }
        }
    }

    match discovery() {
        Some(ResolutionCandidate::TrustedLocal(url)) => {
            let token_provenance = if env_token.is_some() {
                TokenProvenance::CallerExplicit
            } else {
                TokenProvenance::None
            };
            return Ok(Resolved {
                url: url.0,
                // Loopback normally needs no credential, but an operator may
                // disable loopback trust and provide an explicit token.
                token: env_token,
                source: Source::Local,
                owner: None,
                diagnostic_owner: None,
                token_provenance,
                fresh_default_owner: conns.current.is_none() && conns.profiles.is_empty(),
            });
        }
        Some(ResolutionCandidate::Mdns(_)) => {
            // Unsigned TXT metadata can be copied, so even an advertiser with a
            // familiar name/fingerprint cannot become a connection target or
            // receive a caller-explicit token. Guided init owns selection,
            // pairing, and endpoint persistence.
            return Err(CoreError::Message(
                "found a Fleety server on the LAN, but automatic discovery is display-only; \
                 run `fleety init`, select the server, and pair it before connecting"
                    .to_string(),
            ));
        }
        None => {}
    }

    let token_provenance = if env_token.is_some() {
        TokenProvenance::CallerExplicit
    } else {
        TokenProvenance::None
    };
    Ok(Resolved {
        url: DEFAULT_URL.to_string(),
        token: env_token,
        source: Source::Default,
        owner: None,
        diagnostic_owner: None,
        token_provenance,
        fresh_default_owner: conns.current.is_none() && conns.profiles.is_empty(),
    })
}

/// Restrict a file to owner-only (`0600`) on Unix; a no-op elsewhere (Windows
/// relies on the per-user profile directory's ACL, like `fleetyd.token`).
#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

// ---- collecting discovery + authenticated fingerprint pinning ----

/// One server found during a collecting LAN scan (guided init / selection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub name: String,
    pub url: String,
    /// The advertised persistent identity fingerprint (absent on old servers).
    pub fingerprint: Option<String>,
}

/// Display name from an advertised mDNS instance: the `fleety-` prefix the
/// server prepends is stripped; an empty leftover falls back to the URL. Pure.
pub fn display_name_from_instance(instance: &str, url: &str) -> String {
    let stripped = instance.strip_prefix("fleety-").unwrap_or(instance).trim();
    if stripped.is_empty() {
        url.to_string()
    } else {
        stripped.to_string()
    }
}

/// Fold one resolved announcement into the collection, de-duplicating by URL
/// (the same server re-announces during a browse window). Pure.
pub fn push_discovered(found: &mut Vec<DiscoveredServer>, entry: DiscoveredServer) {
    if !found.iter().any(|d| d.url == entry.url) {
        found.push(entry);
    }
}

/// Browse the LAN for the full collection window and return EVERY resolved
/// `_fleety._tcp.local.` server (name + url + advertised fingerprint,
/// de-duplicated by URL, discovery order). Empty when mDNS is disabled or the
/// browse cannot start — callers fall back, never error.
pub fn discover_all_via_mdns(window: std::time::Duration) -> Vec<DiscoveredServer> {
    let mut found = Vec::new();
    if std::env::var("FLEETY_MDNS_DISABLED").is_ok() {
        return found;
    }
    // Stand-in for a live browse, so the discovery path can be exercised where
    // multicast does not reach between processes — CI runners, most containers.
    // It only fabricates the *candidate*; everything downstream (whether the
    // candidate may be used, whether credentials may be sent to it) runs
    // unchanged, which is the part worth testing.
    if let Ok(url) = std::env::var("FLEETY_MDNS_FAKE_URL") {
        let url = url.trim();
        if !url.is_empty() {
            found.push(DiscoveredServer {
                name: "fake".to_string(),
                url: url.to_string(),
                fingerprint: None,
            });
        }
        return found;
    }
    let Ok(daemon) = mdns_sd::ServiceDaemon::new() else {
        return found;
    };
    let Ok(receiver) = daemon.browse("_fleety._tcp.local.") else {
        return found;
    };
    let deadline = std::time::Instant::now() + window;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let recv_timeout = remaining.min(std::time::Duration::from_millis(500));
        match receiver.recv_timeout(recv_timeout) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let addrs = info.get_addresses_v4();
                if let Some(ip) = addrs.iter().next() {
                    let url = format!("ws://{}:{}", ip, info.get_port());
                    let instance = info
                        .get_fullname()
                        .strip_suffix("._fleety._tcp.local.")
                        .unwrap_or_else(|| info.get_fullname());
                    let fingerprint = info
                        .get_property_val_str("fp")
                        .filter(|v| !v.is_empty())
                        .map(String::from);
                    let name = display_name_from_instance(instance, &url);
                    push_discovered(
                        &mut found,
                        DiscoveredServer {
                            name,
                            url,
                            fingerprint,
                        },
                    );
                }
            }
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    let _ = daemon.shutdown();
    found
}

/// Select the safe mDNS target from a completed discovery window. A server
/// whose advertised fingerprint matches the current profile's pin wins. No
/// other profile participates in automatic selection: selecting A must never
/// borrow B's identity or token. With no current pin (or no match), discovery
/// order remains the unauthenticated fallback.
pub fn select_discovered(conns: &Connections, found: &[DiscoveredServer]) -> Option<Discovered> {
    let current_pin = conns
        .current_profile()
        .and_then(|profile| profile.fingerprint.as_deref());
    let preferred = current_pin
        .and_then(|pin| {
            found
                .iter()
                .find(|server| server.fingerprint.as_deref() == Some(pin))
        })
        .or_else(|| found.first())?;
    Some(Discovered {
        url: preferred.url.clone(),
        fingerprint: preferred.fingerprint.clone(),
    })
}

/// Collect mDNS advertisements for the whole window and select one using the
/// stored profile fingerprints. Keeping collection and selection together
/// prevents callers from accidentally discarding identity metadata or trusting
/// whichever responder happened to arrive first.
pub fn discover_for_connections(
    conns: &Connections,
    window: std::time::Duration,
) -> Option<Discovered> {
    let found = discover_all_via_mdns(window);
    select_discovered(conns, &found)
}

/// The trust-on-authenticated-connect rule for a server fingerprint seen on an
/// authenticated connection (or minted at pairing). Pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinDecision {
    /// No pin yet → adopt the seen fingerprint.
    Pin,
    /// Already pinned to this fingerprint → nothing to do.
    AlreadyPinned,
    /// Pinned to a DIFFERENT fingerprint → never overwrite; warn and suggest
    /// re-pairing (a changed server identity is an anomaly, not a rotation).
    IdentityChanged,
}

/// Decide what to do with a fingerprint seen on a trusted (authenticated or
/// pairing) connection, against the profile's existing pin. Pure.
pub fn tofu_pin_decision(existing: Option<&str>, seen: &str) -> PinDecision {
    match existing {
        None => PinDecision::Pin,
        Some(p) if p == seen => PinDecision::AlreadyPinned,
        Some(_) => PinDecision::IdentityChanged,
    }
}

fn resolved_profile_owner<'a>(
    conns: &'a mut Connections,
    target: &Resolved,
    action: &str,
) -> Result<&'a mut Profile> {
    let owner = target.owner.as_ref().ok_or_else(|| {
        CoreError::Message(format!(
            "the transient connection has no saved profile owner; {action} was not applied"
        ))
    })?;
    let source_name = match &target.source {
        Source::Profile(name) | Source::OverrideProfile(name) => name,
        _ => {
            return Err(CoreError::Message(format!(
                "the resolved connection source is transient; {action} was not applied"
            )))
        }
    };
    if source_name != &owner.name || target.url != owner.profile.url {
        return Err(CoreError::Message(format!(
            "server profile provenance changed during connection; {action} was not applied"
        )));
    }
    if owner.require_current && conns.current.as_deref() != Some(owner.name.as_str()) {
        return Err(CoreError::Message(format!(
            "server profile '{}' is no longer current; {action} was not applied",
            owner.name
        )));
    }
    let profile = conns.profiles.get_mut(&owner.name).ok_or_else(|| {
        CoreError::Message(format!(
            "server profile '{}' disappeared during connection; {action} was not applied",
            owner.name
        ))
    })?;
    if profile != &owner.profile {
        return Err(CoreError::Message(format!(
            "server profile '{}' changed during connection; {action} was not applied",
            owner.name
        )));
    }
    Ok(profile)
}

/// Validate the exact saved profile generation carried by a resolved target
/// without granting mutation authority to the caller.
pub fn validate_resolved_profile_owner<'a>(
    conns: &'a Connections,
    target: &Resolved,
    action: &str,
) -> Result<&'a Profile> {
    let owner = target.owner.as_ref().ok_or_else(|| {
        CoreError::Message(format!(
            "the transient connection has no saved profile owner; {action} was not applied"
        ))
    })?;
    let source_name = match &target.source {
        Source::Profile(name) | Source::OverrideProfile(name) => name,
        _ => {
            return Err(CoreError::Message(format!(
                "the resolved connection source is transient; {action} was not applied"
            )))
        }
    };
    if source_name != &owner.name || target.url != owner.profile.url {
        return Err(CoreError::Message(format!(
            "server profile provenance changed during connection; {action} was not applied"
        )));
    }
    if owner.require_current && conns.current.as_deref() != Some(owner.name.as_str()) {
        return Err(CoreError::Message(format!(
            "server profile '{}' is no longer current; {action} was not applied",
            owner.name
        )));
    }
    let profile = conns.profiles.get(&owner.name).ok_or_else(|| {
        CoreError::Message(format!(
            "server profile '{}' disappeared during connection; {action} was not applied",
            owner.name
        ))
    })?;
    if profile != &owner.profile {
        return Err(CoreError::Message(format!(
            "server profile '{}' changed during connection; {action} was not applied",
            owner.name
        )));
    }
    Ok(profile)
}

/// Apply trust-on-first-use only to the exact saved profile generation captured
/// by [`resolve`]. A delete/recreate or concurrent profile mutation is rejected
/// even when the name and URL are unchanged.
pub fn pin_resolved_profile_fingerprint(target: &Resolved, seen: &str) -> Result<PinDecision> {
    pin_resolved_profile_fingerprint_and_refresh_at(&connections_path(), target, seen)
        .map(|(decision, _)| decision)
}

pub fn pin_resolved_profile_fingerprint_at(
    path: &Path,
    target: &Resolved,
    seen: &str,
) -> Result<PinDecision> {
    pin_resolved_profile_fingerprint_and_refresh_at(path, target, seen)
        .map(|(decision, _)| decision)
}

/// Pin the exact saved generation and return a refreshed capability for the
/// committed generation while preserving the transport token provenance.
/// Long-lived callers must use this after TOFU so their next operation does not
/// present the now-stale pre-pin owner snapshot.
pub fn pin_resolved_profile_fingerprint_and_refresh(
    target: &Resolved,
    seen: &str,
) -> Result<(PinDecision, Resolved)> {
    pin_resolved_profile_fingerprint_and_refresh_at(&connections_path(), target, seen)
}

pub fn pin_resolved_profile_fingerprint_and_refresh_at(
    path: &Path,
    target: &Resolved,
    seen: &str,
) -> Result<(PinDecision, Resolved)> {
    if seen.trim().is_empty() {
        return Err(CoreError::Message(
            "the Server did not provide a usable identity fingerprint; no profile was changed"
                .to_string(),
        ));
    }
    mutate_at(path, |conns| {
        let profile = resolved_profile_owner(conns, target, "its fingerprint")?;
        let decision = tofu_pin_decision(profile.fingerprint.as_deref(), seen);
        if decision == PinDecision::Pin {
            profile.fingerprint = Some(seen.to_string());
        }
        let profile = profile.clone();
        let owner = target.owner.as_ref().ok_or_else(|| {
            CoreError::Message(
                "the transient connection has no saved profile owner; its fingerprint was not applied"
                    .to_string(),
            )
        })?;
        Ok((
            decision,
            Resolved {
                url: target.url.clone(),
                token: target.token.clone(),
                source: target.source.clone(),
                owner: Some(ProfileOwnerSnapshot {
                    name: owner.name.clone(),
                    profile,
                    require_current: owner.require_current,
                }),
                diagnostic_owner: None,
                token_provenance: target.token_provenance,
                fresh_default_owner: false,
            },
        ))
    })
}

/// Commit a non-empty Server identity and optional newly minted token onto the
/// exact owner generation captured by [`resolve`]. The returned target freezes
/// the newly committed disk generation and deliberately uses the stored token,
/// never a caller-explicit transport override.
pub fn store_resolved_profile_credentials(
    target: &Resolved,
    token: Option<&str>,
    fingerprint: &str,
) -> Result<(PinDecision, Resolved)> {
    store_resolved_profile_credentials_at(&connections_path(), target, token, fingerprint)
}

pub fn store_resolved_profile_credentials_at(
    path: &Path,
    target: &Resolved,
    token: Option<&str>,
    fingerprint: &str,
) -> Result<(PinDecision, Resolved)> {
    match store_resolved_profile_credentials_recoverable_at(path, target, token, fingerprint)? {
        CredentialCommit::Durable {
            decision,
            committed,
            ..
        } => Ok((decision, committed)),
        CredentialCommit::PublishedNotDurable { error, .. } => Err(error),
    }
}

#[derive(Debug)]
pub enum CredentialCommit {
    Durable {
        decision: PinDecision,
        committed: Resolved,
        profile_is_current: bool,
    },
    PublishedNotDurable {
        decision: PinDecision,
        committed: Resolved,
        profile_is_current: bool,
        error: CoreError,
    },
}

pub fn store_resolved_profile_credentials_recoverable(
    target: &Resolved,
    token: Option<&str>,
    fingerprint: &str,
) -> Result<CredentialCommit> {
    store_resolved_profile_credentials_recoverable_at(
        &connections_path(),
        target,
        token,
        fingerprint,
    )
}

pub fn store_resolved_profile_credentials_recoverable_at(
    path: &Path,
    target: &Resolved,
    token: Option<&str>,
    fingerprint: &str,
) -> Result<CredentialCommit> {
    store_resolved_profile_credentials_recoverable_at_with_sync(
        path,
        target,
        token,
        fingerprint,
        sync_published_connections,
    )
}

fn store_resolved_profile_credentials_recoverable_at_with_sync<P>(
    path: &Path,
    target: &Resolved,
    token: Option<&str>,
    fingerprint: &str,
    sync_published: P,
) -> Result<CredentialCommit>
where
    P: FnOnce(&Path, Option<&Path>) -> std::io::Result<()>,
{
    if fingerprint.trim().is_empty() || token.is_some_and(|value| value.trim().is_empty()) {
        return Err(CoreError::Message(
            "the Server returned incomplete credentials; no credential was saved".to_string(),
        ));
    }
    let ((decision, committed, profile_is_current), status) = mutate_at_with_status_with_sync(
        path,
        |conns| {
            let owner = target.owner.as_ref().ok_or_else(|| {
                CoreError::Message(
                    "the transient connection has no saved profile owner; credentials were not saved"
                        .to_string(),
                )
            })?;
            let owner_name = owner.name.clone();
            let require_current = owner.require_current;
            let profile_is_current = conns.current.as_deref() == Some(owner_name.as_str());
            let profile = resolved_profile_owner(conns, target, "authenticated credentials")?;
            let decision = tofu_pin_decision(profile.fingerprint.as_deref(), fingerprint);
            if decision == PinDecision::IdentityChanged {
                return Err(CoreError::Message(
                    "the saved profile has a different identity fingerprint; no credential was saved"
                        .to_string(),
                ));
            }
            if let Some(token) = token {
                profile.token = Some(token.to_string());
            }
            if decision == PinDecision::Pin {
                profile.fingerprint = Some(fingerprint.to_string());
            }
            let profile = profile.clone();
            let token = profile.token.clone();
            let token_provenance = if token.is_some() {
                TokenProvenance::SavedProfile
            } else {
                TokenProvenance::None
            };
            Ok((
                decision,
                Resolved {
                    url: profile.url.clone(),
                    token,
                    source: target.source.clone(),
                    owner: Some(ProfileOwnerSnapshot {
                        name: owner_name,
                        profile,
                        require_current,
                    }),
                    diagnostic_owner: None,
                    token_provenance,
                    fresh_default_owner: false,
                },
                profile_is_current,
            ))
        },
        sync_published,
    )?;
    Ok(match status {
        SaveStatus::Durable => CredentialCommit::Durable {
            decision,
            committed,
            profile_is_current,
        },
        SaveStatus::PublishedNotDurable(error) => CredentialCommit::PublishedNotDurable {
            decision,
            committed,
            profile_is_current,
            error,
        },
    })
}

/// Persist pairing material only onto the exact profile generation captured by
/// [`resolve`]. The minted token and Server identity must both be non-empty.
pub fn store_resolved_profile_pairing(
    target: &Resolved,
    token: &str,
    fingerprint: &str,
) -> Result<PinDecision> {
    store_resolved_profile_pairing_at(&connections_path(), target, token, fingerprint)
}

pub fn store_resolved_profile_pairing_at(
    path: &Path,
    target: &Resolved,
    token: &str,
    fingerprint: &str,
) -> Result<PinDecision> {
    match store_resolved_profile_pairing_recoverable_at(path, target, token, fingerprint)? {
        CredentialCommit::Durable { decision, .. } => Ok(decision),
        CredentialCommit::PublishedNotDurable { error, .. } => Err(error),
    }
}

pub fn store_resolved_profile_pairing_recoverable(
    target: &Resolved,
    token: &str,
    fingerprint: &str,
) -> Result<CredentialCommit> {
    store_resolved_profile_pairing_recoverable_at(&connections_path(), target, token, fingerprint)
}

pub fn store_resolved_profile_pairing_recoverable_at(
    path: &Path,
    target: &Resolved,
    token: &str,
    fingerprint: &str,
) -> Result<CredentialCommit> {
    store_resolved_profile_pairing_recoverable_at_with_sync(
        path,
        target,
        token,
        fingerprint,
        sync_published_connections,
    )
}

fn store_resolved_profile_pairing_recoverable_at_with_sync<P>(
    path: &Path,
    target: &Resolved,
    token: &str,
    fingerprint: &str,
    sync_published: P,
) -> Result<CredentialCommit>
where
    P: FnOnce(&Path, Option<&Path>) -> std::io::Result<()>,
{
    if token.trim().is_empty() || fingerprint.trim().is_empty() {
        return Err(CoreError::Message(
            "the Server returned incomplete pairing credentials; no credential was saved"
                .to_string(),
        ));
    }
    let ((decision, committed, profile_is_current), status) = mutate_at_with_status_with_sync(
        path,
        |conns| {
            let owner = target.owner.as_ref().ok_or_else(|| {
                CoreError::Message(
                    "the transient connection has no saved profile owner; pairing credentials were not saved"
                        .to_string(),
                )
            })?;
            let owner_name = owner.name.clone();
            let require_current = owner.require_current;
            let source = target.source.clone();
            let profile_is_current = conns.current.as_deref() == Some(owner_name.as_str());
            let profile = resolved_profile_owner(conns, target, "pairing credentials")?;
            let decision = tofu_pin_decision(profile.fingerprint.as_deref(), fingerprint);
            profile.token = Some(token.to_string());
            profile.fingerprint = Some(fingerprint.to_string());
            let profile = profile.clone();
            let committed = Resolved {
                url: profile.url.clone(),
                token: profile.token.clone(),
                source,
                owner: Some(ProfileOwnerSnapshot {
                    name: owner_name,
                    profile,
                    require_current,
                }),
                diagnostic_owner: None,
                token_provenance: TokenProvenance::SavedProfile,
                fresh_default_owner: false,
            };
            Ok((decision, committed, profile_is_current))
        },
        sync_published,
    )?;
    Ok(match status {
        SaveStatus::Durable => CredentialCommit::Durable {
            decision,
            committed,
            profile_is_current,
        },
        SaveStatus::PublishedNotDurable(error) => CredentialCommit::PublishedNotDurable {
            decision,
            committed,
            profile_is_current,
            error,
        },
    })
}

/// Clear a rejected token only when the resolved `Hello` actually used the
/// saved-profile credential. Caller-explicit bytes remain explicit even when
/// they happen to equal the frozen disk token.
pub fn clear_resolved_profile_token(target: &Resolved) -> Result<bool> {
    if !target.sent_saved_profile_token() {
        return Ok(false);
    }
    mutate(|conns| {
        let profile = resolved_profile_owner(conns, target, "its rejected token")?;
        if profile.token.as_deref() != target.token() {
            return Ok(false);
        }
        profile.token = None;
        Ok(true)
    })
}

/// Recovery guidance shared by one-shot CLI connections and fleetyd retries.
/// Automatic discovery cannot prove that a new endpoint owns an old bearer.
pub fn explicit_repair_guidance() -> String {
    "the saved Server address needs explicit recovery: select the intended endpoint with \
     `fleety init <ws-url> --name <profile> --pairing-code <code>`; automatic discovery \
     cannot prove Server identity, so Fleety will not send the stored token or change the URL"
        .to_string()
}

/// Changing a saved endpoint breaks its credential binding. Return whether
/// credentials were cleared so callers can tell the user to re-pair.
pub fn reselect_profile_endpoint(profile: &mut Profile, new_url: String) -> bool {
    if profile.url == new_url {
        return false;
    }
    let had_token = profile.token.take().is_some();
    let had_fingerprint = profile.fingerprint.take().is_some();
    profile.url = new_url;
    had_token || had_fingerprint
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tofu_pin_rules() {
        assert_eq!(tofu_pin_decision(None, "fp-1"), PinDecision::Pin);
        assert_eq!(
            tofu_pin_decision(Some("fp-1"), "fp-1"),
            PinDecision::AlreadyPinned
        );
        assert_eq!(
            tofu_pin_decision(Some("fp-1"), "fp-2"),
            PinDecision::IdentityChanged
        );
    }

    #[test]
    fn discovery_names_and_dedupe() {
        assert_eq!(display_name_from_instance("fleety-mini", "ws://u"), "mini");
        assert_eq!(display_name_from_instance("", "ws://u"), "ws://u");
        assert_eq!(display_name_from_instance("fleety-", "ws://u"), "ws://u");
        let mut found = Vec::new();
        let entry = |url: &str| DiscoveredServer {
            name: "n".into(),
            url: url.into(),
            fingerprint: None,
        };
        push_discovered(&mut found, entry("ws://a"));
        push_discovered(&mut found, entry("ws://a"));
        push_discovered(&mut found, entry("ws://b"));
        assert_eq!(found.len(), 2);
    }

    fn tmp_path() -> PathBuf {
        std::env::temp_dir().join(format!("fleety-conn-{}.toml", uuid::Uuid::new_v4()))
    }

    #[test]
    fn mutation_lease_drop_never_deletes_a_successor_lock() {
        let path = tmp_path().with_extension("toml.lock");
        std::fs::write(&path, "successor-owner").expect("publish successor");

        drop(MutationLease {
            path: path.clone(),
            owner: "previous-owner".to_string(),
        });

        assert_eq!(
            std::fs::read_to_string(&path).expect("successor lock remains"),
            "successor-owner"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn load_missing_is_empty_not_error() {
        let p = tmp_path();
        let conns = load_at(&p).expect("missing file → empty, not error");
        assert_eq!(conns, Connections::default());
        assert!(conns.current.is_none());
        assert!(conns.profiles.is_empty());
    }

    #[test]
    fn save_load_roundtrip_preserves_url_token_label() {
        let p = tmp_path();
        let mut conns = Connections {
            device_id: "dev-1".to_string(),
            current: Some("home".to_string()),
            profiles: BTreeMap::new(),
        };
        conns.profiles.insert(
            "home".to_string(),
            Profile {
                url: "ws://192.168.1.20:8787".to_string(),
                token: Some("tok-home".to_string()),
                label: Some("Home".to_string()),
                fingerprint: Some("AA:BB".to_string()),
                generation: "profile-home".to_string(),
            },
        );
        save_at(&p, &conns).expect("save");
        let back = load_at(&p).expect("load");
        assert_eq!(back, conns);
        let home = back.current_profile().expect("current resolves");
        assert_eq!(home.url, "ws://192.168.1.20:8787");
        assert_eq!(home.token.as_deref(), Some("tok-home"));
        assert_eq!(home.label.as_deref(), Some("Home"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn staged_sync_failure_never_replaces_existing_credentials() {
        let p = tmp_path();
        std::fs::write(
            &p,
            "device_id = \"dev-1\"\ncurrent = \"home\"\n\n\
             [profiles.home]\nurl = \"ws://old:8787\"\ntoken = \"old-token\"\n",
        )
        .expect("seed old credentials");
        let mut replacement = load_at(&p).expect("load old credentials");
        replacement.profiles.get_mut("home").expect("home").token = Some("new-token".to_string());
        let mut published = false;

        save_at_with_sync(
            &p,
            &replacement,
            |_| {
                Err(std::io::Error::other(
                    "injected staged credential sync failure",
                ))
            },
            |_, _| {
                published = true;
                Ok(())
            },
        )
        .expect_err("staged sync failure");

        assert!(!published, "an unsynced temp file is never published");
        let unchanged = load_at(&p).expect("load unchanged credentials");
        assert_eq!(
            unchanged.profiles["home"].token.as_deref(),
            Some("old-token")
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn publication_sync_failure_reports_complete_but_not_durable_credentials() {
        let p = tmp_path();
        let mut replacement = Connections {
            device_id: "dev-1".to_string(),
            current: Some("home".to_string()),
            ..Default::default()
        };
        replacement.profiles.insert(
            "home".to_string(),
            Profile {
                url: "ws://new:8787".to_string(),
                token: Some("new-token".to_string()),
                fingerprint: Some("new-fingerprint".to_string()),
                ..Default::default()
            },
        );

        let status = save_at_with_sync_status(&p, &replacement, std::fs::File::sync_all, |_, _| {
            Err(std::io::Error::other(
                "injected published credential sync failure",
            ))
        })
        .expect("the complete replacement is observable");
        assert!(matches!(status, SaveStatus::PublishedNotDurable(_)));

        let complete = load_at(&p).expect("published file remains structurally complete");
        assert_eq!(complete, replacement);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_is_an_error_not_empty() {
        let p = tmp_path();
        std::fs::write(&p, "this is = = not toml").expect("write junk");
        let err = load_at(&p).expect_err("corrupt file must error, never silently empty");
        assert!(
            err.to_string().contains("connections.toml"),
            "error names the file: {err}"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn effective_device_id_prefers_stored_then_machine() {
        // Stored id wins.
        let conns = Connections {
            device_id: "stored-id".to_string(),
            ..Default::default()
        };
        assert_eq!(conns.effective_device_id(), "stored-id");
        // Empty stored id → falls back to the machine id (non-empty).
        let fresh = Connections::default();
        assert!(!fresh.effective_device_id().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp_path();
        save_at(&p, &Connections::default()).expect("save");
        let mode = std::fs::metadata(&p).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "connections.toml must be owner-only");
        let _ = std::fs::remove_file(&p);
    }

    fn tmp_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("fleety-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("mk temp dir");
        d
    }

    #[test]
    fn migrate_creates_default_profile_backs_up_and_locks_device_id() {
        let dir = tmp_dir();
        std::fs::write(
            dir.join("config.json"),
            r#"{"device_id":"cfg-dev","agent_url":"ws://srv:8787","token":"tok-x","extra":true}"#,
        )
        .expect("seed config.json");

        assert_eq!(
            migrate_from_config_json_at(&dir).expect("migrate"),
            Migration::Migrated
        );
        let conns = load_at(&dir.join("connections.toml")).expect("load migrated");
        // device_id locked from config.json (not the machine id).
        assert_eq!(conns.device_id, "cfg-dev");
        assert_eq!(conns.current.as_deref(), Some("default"));
        let default = conns.current_profile().expect("default profile");
        assert_eq!(default.url, "ws://srv:8787");
        assert_eq!(default.token.as_deref(), Some("tok-x"));
        // config.json backed up (renamed), not deleted.
        assert!(
            !dir.join("config.json").exists(),
            "config.json renamed away"
        );
        assert!(
            dir.join("config.json.migrated").exists(),
            "backup kept for rollback"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_is_idempotent() {
        let dir = tmp_dir();
        std::fs::write(dir.join("config.json"), r#"{"agent_url":"ws://srv:8787"}"#).expect("seed");
        assert_eq!(
            migrate_from_config_json_at(&dir).expect("first"),
            Migration::Migrated
        );
        let before = std::fs::read_to_string(dir.join("connections.toml")).expect("read");
        // A second run does not re-migrate or rewrite the file.
        assert_eq!(
            migrate_from_config_json_at(&dir).expect("second"),
            Migration::AlreadyPresent
        );
        let after = std::fs::read_to_string(dir.join("connections.toml")).expect("read");
        assert_eq!(before, after, "idempotent: file unchanged on re-run");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_urlless_record_leaves_url_empty() {
        let dir = tmp_dir();
        // Token but no agent_url stays url-less for explicit recovery.
        std::fs::write(dir.join("config.json"), r#"{"token":"only-token"}"#).expect("seed");
        assert_eq!(
            migrate_from_config_json_at(&dir).expect("migrate"),
            Migration::Migrated
        );
        let conns = load_at(&dir.join("connections.toml")).expect("load");
        let default = conns.current_profile().expect("default profile");
        assert!(default.url.is_empty(), "url-less record keeps url empty");
        assert_eq!(default.token.as_deref(), Some("only-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_nothing_when_no_legacy_files() {
        let dir = tmp_dir();
        assert_eq!(
            migrate_from_config_json_at(&dir).expect("migrate"),
            Migration::NothingToMigrate
        );
        assert!(
            !dir.join("connections.toml").exists(),
            "a fresh device gets no connections.toml until it enrolls"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_folds_daemon_token_when_only_fleetyd_token_present() {
        let dir = tmp_dir();
        // A daemon-only-paired device: fleetyd.token, no config.json.
        std::fs::write(dir.join("fleetyd.token"), "  daemon-tok\n").expect("seed token");
        assert_eq!(
            migrate_from_config_json_at(&dir).expect("migrate"),
            Migration::Migrated
        );
        let conns = load_at(&dir.join("connections.toml")).expect("load");
        let default = conns.current_profile().expect("default profile");
        // The token is preserved for explicit recovery, but automatic mDNS must
        // not use it without a trusted endpoint binding.
        assert!(default.url.is_empty());
        assert_eq!(default.token.as_deref(), Some("daemon-tok"));
        assert!(
            dir.join("fleetyd.token.migrated").exists(),
            "daemon token backed up"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_latch_leaves_a_prior_connections_file_and_config_json_untouched() {
        let dir = tmp_dir();
        // Simulate another starter having already produced connections.toml.
        std::fs::write(dir.join("connections.toml"), "device_id = \"other\"\n").expect("seed");
        std::fs::write(dir.join("config.json"), r#"{"agent_url":"ws://srv"}"#).expect("seed cfg");
        assert_eq!(
            migrate_from_config_json_at(&dir).expect("migrate"),
            Migration::AlreadyPresent
        );
        // The loser must not clobber the winner's file nor rename config.json.
        let conns = load_at(&dir.join("connections.toml")).expect("load");
        assert_eq!(conns.device_id, "other");
        assert!(
            dir.join("config.json").exists(),
            "loser must not rename config.json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_under_concurrency_has_exactly_one_migrator() {
        let dir = tmp_dir();
        std::fs::write(
            dir.join("config.json"),
            r#"{"device_id":"race-dev","agent_url":"ws://srv:8787"}"#,
        )
        .expect("seed");
        // Many threads race to migrate the same dir; the O_EXCL latch must let
        // exactly one win, and the file must end with a single device_id.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || migrate_from_config_json_at(&d))
            })
            .collect();
        let migrated = handles
            .into_iter()
            .map(|h| h.join().expect("thread"))
            .filter(|r| matches!(r, Ok(Migration::Migrated)))
            .count();
        assert_eq!(migrated, 1, "exactly one thread migrates under the latch");
        let conns = load_at(&dir.join("connections.toml")).expect("load");
        assert_eq!(conns.device_id, "race-dev");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- resolve() precedence + mDNS sticky/fingerprint guard ----

    fn profile(url: &str) -> Profile {
        Profile {
            url: url.to_string(),
            ..Default::default()
        }
    }

    /// A discovery stub that must never run — it panics if the resolver queries it.
    fn no_discovery() -> Option<ResolutionCandidate> {
        panic!("discovery must not be queried on this path");
    }

    fn conns_with(current: Option<&str>, profiles: &[(&str, Profile)]) -> Connections {
        Connections {
            device_id: "dev".to_string(),
            current: current.map(String::from),
            profiles: profiles
                .iter()
                .map(|(n, p)| (n.to_string(), p.clone()))
                .collect(),
        }
    }

    #[test]
    fn resolve_named_override_uses_that_profile_and_errors_on_unknown() {
        let mut work = profile("ws://work:8787");
        work.token = Some("work-tok".to_string());
        let conns = conns_with(
            Some("home"),
            &[("home", profile("ws://home:8787")), ("work", work)],
        );
        let r = resolve(
            &conns,
            &Target::Named("work".into()),
            None,
            None,
            no_discovery,
        )
        .expect("named");
        assert_eq!(r.url, "ws://work:8787");
        assert_eq!(r.token.as_deref(), Some("work-tok"));
        assert_eq!(r.source, Source::OverrideProfile("work".to_string()));
        // An unknown name is an error, not a silent fallback.
        assert!(resolve(
            &conns,
            &Target::Named("ghost".into()),
            None,
            None,
            no_discovery
        )
        .is_err());
    }

    #[test]
    fn repeated_read_only_conversion_preserves_identity_expectation() {
        let mut saved = profile("ws://work:8787");
        saved.fingerprint = Some("server-fingerprint".to_string());
        let conns = conns_with(Some("work"), &[("work", saved)]);
        let target = resolve(&conns, &Target::Current, None, None, no_discovery)
            .expect("resolve saved target")
            .into_read_only()
            .into_read_only();

        assert!(!target.has_profile_owner());
        assert!(target.has_profile_identity_expectation());
        assert_eq!(
            target.profile_owner_fingerprint(),
            Some("server-fingerprint")
        );
    }

    #[test]
    fn raw_url_override_never_borrows_same_url_profile_tokens() {
        for current in ["a", "b"] {
            let mut a = profile("ws://same:8787");
            a.token = Some("token-a".to_string());
            let mut b = profile("ws://same:8787");
            b.token = Some("token-b".to_string());
            let conns = conns_with(Some(current), &[("b", b), ("a", a)]);

            let raw = resolve(
                &conns,
                &Target::Url("ws://same:8787".into()),
                None,
                None,
                no_discovery,
            )
            .expect("raw URL");
            assert_eq!(raw.url, "ws://same:8787");
            assert_eq!(
                raw.token, None,
                "raw URL must not borrow a saved token when current={current}"
            );
            assert_eq!(raw.source, Source::OverrideUrl);

            let explicit = resolve(
                &conns,
                &Target::Url("ws://same:8787".into()),
                None,
                Some("caller-token".to_string()),
                no_discovery,
            )
            .expect("raw URL with explicit token");
            assert_eq!(explicit.token.as_deref(), Some("caller-token"));
        }
    }

    #[test]
    fn resolved_profile_owner_rejects_same_url_owner_drift() {
        let mut original = profile("ws://same:8787");
        original.token = Some("old-token".to_string());
        let mut conns = conns_with(Some("A"), &[("A", original)]);
        let target = resolve(&conns, &Target::Current, None, None, no_discovery)
            .expect("capture owner generation");

        conns.profiles.get_mut("A").expect("A").token = Some("replacement-token".to_string());
        let error = resolved_profile_owner(&mut conns, &target, "pairing credentials")
            .expect_err("same name and URL do not authorize a replacement profile generation");
        assert!(error.report().message.contains("changed during connection"));
    }

    #[test]
    fn transient_resolved_target_cannot_acquire_profile_mutation_authority() {
        let mut saved = profile("ws://same:8787");
        saved.token = Some("saved-token".to_string());
        let mut conns = conns_with(Some("A"), &[("A", saved)]);
        let target = resolve(
            &conns,
            &Target::Url("ws://same:8787".to_string()),
            None,
            Some("caller-token".to_string()),
            no_discovery,
        )
        .expect("raw target");

        let error = resolved_profile_owner(&mut conns, &target, "its fingerprint")
            .expect_err("raw target has no owner capability");
        assert!(error.report().message.contains("no saved profile owner"));
    }

    #[test]
    fn resolve_env_url_does_not_inherit_a_different_profiles_token() {
        // Current profile home .20 exists, but FLEETY_AGENT_URL overrides it.
        let mut home = profile("ws://192.168.1.20:8787");
        home.token = Some("home-tok".to_string());
        let conns = conns_with(Some("home"), &[("home", home)]);
        let r = resolve(
            &conns,
            &Target::Current,
            Some("ws://env-override:8787".to_string()),
            None,
            no_discovery,
        )
        .expect("env");
        assert_eq!(r.url, "ws://env-override:8787");
        assert_eq!(r.source, Source::Env);
        assert!(r.token.is_none());
    }

    #[test]
    fn resolve_env_url_never_inherits_even_the_current_same_endpoint_token() {
        let mut home = profile("ws://same:8787");
        home.token = Some("home-tok".to_string());
        let conns = conns_with(Some("home"), &[("home", home)]);
        let raw = resolve(
            &conns,
            &Target::Current,
            Some("ws://same:8787".to_string()),
            None,
            no_discovery,
        )
        .expect("env");
        assert_eq!(raw.token, None);

        let explicit = resolve(
            &conns,
            &Target::Current,
            Some("ws://same:8787".to_string()),
            Some("caller-token".to_string()),
            no_discovery,
        )
        .expect("env with explicit token");
        assert_eq!(explicit.token.as_deref(), Some("caller-token"));
    }

    #[test]
    fn shared_resolver_rejects_credentials_and_fragments_for_raw_and_env_urls() {
        let mut conns = Connections::default();
        for invalid in [
            "ws://user:secret@example.test:8787/ws",
            "wss://example.test/ws#fragment",
        ] {
            conns.profiles.insert(
                "named".to_string(),
                Profile {
                    url: invalid.to_string(),
                    token: Some("saved-token".to_string()),
                    ..Profile::default()
                },
            );
            let named = resolve(
                &conns,
                &Target::Named("named".to_string()),
                None,
                None,
                no_discovery,
            )
            .expect_err("named URL must use the shared endpoint validator");
            assert!(named.report().message.contains("server url"));

            let raw = resolve(
                &conns,
                &Target::Url(invalid.to_string()),
                None,
                None,
                no_discovery,
            )
            .expect_err("raw URL must use the shared endpoint validator");
            assert!(raw.report().message.contains("server url"));

            let env = resolve(
                &conns,
                &Target::Current,
                Some(invalid.to_string()),
                None,
                no_discovery,
            )
            .expect_err("env URL must use the shared endpoint validator");
            assert!(env.report().message.contains("server url"));
        }
    }

    #[test]
    fn profile_generation_rejects_delete_recreate_aba() {
        let dir = tmp_dir();
        let path = dir.join("connections.toml");
        let mut conns = conns_with(Some("office"), &[("office", profile("ws://office:8787"))]);
        conns.profiles.get_mut("office").unwrap().generation = "generation-a".to_string();
        save_at(&path, &conns).expect("seed owner");
        let target =
            resolve(&conns, &Target::Current, None, None, no_discovery).expect("freeze owner");

        mutate_at(&path, |live| {
            let old = live
                .profiles
                .remove("office")
                .expect("delete old lifecycle");
            live.profiles.insert(
                "office".to_string(),
                Profile {
                    generation: "generation-b".to_string(),
                    ..old
                },
            );
            Ok(())
        })
        .expect("recreate same visible profile");

        let error =
            store_resolved_profile_pairing_at(&path, &target, "new-token", "new-fingerprint")
                .expect_err("old handshake must not commit into recreated owner");
        assert!(error.report().message.contains("changed"), "{error}");
        let after = load_at(&path).expect("load recreated owner");
        assert_eq!(after.profiles["office"].generation, "generation-b");
        assert_ne!(after.profiles["office"].token.as_deref(), Some("new-token"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn durable_resolution_upgrades_only_the_selected_profile_generation() {
        let dir = tmp_dir();
        let path = dir.join("connections.toml");
        std::fs::write(
            &path,
            "device_id = \"dev-1\"\ncurrent = \"home\"\n\n\
             [profiles.home]\nurl = \"ws://home:8787\"\ntoken = \"token\"\n\n\
             [profiles.other]\nurl = \"ws://other:8787\"\ntoken = \"other-token\"\n",
        )
        .expect("seed pre-generation connections");

        assert!(
            ensure_resolvable_profile_generation_at(&path, &Target::Current, false)
                .expect("upgrade selected generation")
        );
        let upgraded = load_at(&path).expect("load upgraded profile");
        let first = upgraded.profiles["home"].generation.clone();
        assert!(!first.is_empty());
        assert!(
            upgraded.profiles["other"].generation.is_empty(),
            "an unrelated legacy profile must remain untouched"
        );

        assert!(
            !ensure_resolvable_profile_generation_at(&path, &Target::Current, false)
                .expect("repeat upgrade")
        );
        assert_eq!(
            load_at(&path).expect("load stable profile").profiles["home"].generation,
            first
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_precise_mutations_preserve_unrelated_connection_fields() {
        let dir = tmp_dir();
        let path = dir.join("connections.toml");
        save_at(&path, &Connections::default()).expect("seed connections");
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));

        let workers = [
            ("a".to_string(), "ws://a:8787".to_string()),
            ("b".to_string(), "ws://b:8787".to_string()),
        ]
        .into_iter()
        .map(|(name, url)| {
            let path = path.clone();
            let start = std::sync::Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                mutate_at(&path, |connections| {
                    connections.profiles.insert(
                        name,
                        Profile {
                            url,
                            ..Default::default()
                        },
                    );
                    Ok(())
                })
                .expect("mutate connections");
            })
        })
        .collect::<Vec<_>>();
        start.wait();
        for worker in workers {
            worker.join().expect("mutation worker");
        }

        let after = load_at(&path).expect("load merged connections");
        assert_eq!(after.profiles["a"].url, "ws://a:8787");
        assert_eq!(after.profiles["b"].url, "ws://b:8787");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_sticky_current_profile_never_queries_mdns() {
        // Spec Example: current="home", home.url=ws://192.168.1.20:8787; an mDNS
        // advertiser (ws://192.168.1.99:8787) is present but must be ignored.
        let conns = conns_with(Some("home"), &[("home", profile("ws://192.168.1.20:8787"))]);
        // no_discovery panics if called — proving sticky profiles skip discovery.
        let r = resolve(&conns, &Target::Current, None, None, no_discovery).expect("sticky");
        assert_eq!(r.url, "ws://192.168.1.20:8787");
        assert_eq!(r.source, Source::Profile("home".to_string()));
    }

    #[test]
    fn resolve_trusted_local_candidate_remains_connectable() {
        let conns = Connections::default();
        let r = resolve(&conns, &Target::Current, None, None, || {
            prefer_trusted_local_candidate(|| Some("ws://127.0.0.1:9000".to_string()), || None)
        })
        .expect("same-host loopback remains a trusted operational target");
        assert_eq!(r.url, "ws://127.0.0.1:9000");
        assert_eq!(r.source, Source::Local);
        assert!(r.token.is_none());
    }

    #[test]
    fn trusted_local_candidate_rejects_lan_urls_and_skips_mdns() {
        assert!(TrustedLocalUrl::parse("ws://192.168.1.20:8787".to_string()).is_none());
        assert!(TrustedLocalUrl::parse("ws://example.test:8787".to_string()).is_none());
        let candidate = prefer_trusted_local_candidate(
            || Some("ws://127.0.0.1:8787".to_string()),
            || panic!("mDNS must not run when trusted loopback is available"),
        );
        assert!(matches!(
            candidate,
            Some(ResolutionCandidate::TrustedLocal(_))
        ));
    }

    #[test]
    fn resolve_trusted_local_keeps_caller_token_for_loopback_auth() {
        let conns = Connections::default();
        let r = resolve(
            &conns,
            &Target::Current,
            None,
            Some("explicit-loopback-token".to_string()),
            || prefer_trusted_local_candidate(|| Some("ws://127.0.0.1:9000".to_string()), || None),
        )
        .expect("an explicit token may authenticate trusted loopback");
        assert_eq!(r.token.as_deref(), Some("explicit-loopback-token"));
    }

    #[test]
    fn resolve_mdns_candidate_requires_explicit_selection() {
        // Spec Example: no current profile; home pinned fingerprint AA:BB + token;
        // mDNS resolves ws://192.168.1.99:8787 presenting CC:DD. The advertiser
        // remains display-only and never becomes an operational target.
        let mut home = profile(""); // url-less: token-only, relies on mDNS
        home.token = Some("home-tok".to_string());
        home.fingerprint = Some("AA:BB".to_string());
        let conns = conns_with(None, &[("home", home)]);
        let error = resolve(&conns, &Target::Current, None, None, || {
            Some(ResolutionCandidate::Mdns(Discovered {
                url: "ws://192.168.1.99:8787".to_string(),
                fingerprint: Some("CC:DD".to_string()),
            }))
        })
        .expect_err("automatic discovery must not create a connection target");
        assert!(error.report().message.contains("fleety init"));
    }

    #[test]
    fn credentialed_discovery_hint_requires_explicit_repair() {
        let mut home = profile("");
        home.token = Some("home-tok".to_string());
        home.fingerprint = Some("fp-home".to_string());
        let conns = conns_with(Some("home"), &[("home", home)]);
        let found = vec![
            DiscoveredServer {
                name: "unrelated".to_string(),
                url: "ws://wrong:8787".to_string(),
                fingerprint: Some("fp-wrong".to_string()),
            },
            DiscoveredServer {
                name: "home".to_string(),
                url: "ws://right:8787".to_string(),
                fingerprint: Some("fp-home".to_string()),
            },
        ];

        let selected = select_discovered(&conns, &found).expect("matching advertiser");
        assert_eq!(selected.url, "ws://right:8787");
        assert_eq!(selected.fingerprint.as_deref(), Some("fp-home"));
        let error = resolve(&conns, &Target::Current, None, None, || {
            Some(ResolutionCandidate::Mdns(selected))
        })
        .expect_err("credentialed profile must not follow discovery");
        assert!(error.report().message.contains("--pairing-code <code>"));
    }

    #[test]
    fn current_unpinned_a_never_borrows_pinned_b_identity_or_token() {
        let mut a = profile("");
        a.token = Some("token-a".to_string());
        let mut b = profile("");
        b.token = Some("token-b".to_string());
        b.fingerprint = Some("fp-b".to_string());
        let conns = conns_with(Some("a"), &[("a", a), ("b", b)]);
        let found = vec![DiscoveredServer {
            name: "server-b".to_string(),
            url: "ws://b:8787".to_string(),
            fingerprint: Some("fp-b".to_string()),
        }];

        for _ in 0..2 {
            let selected = select_discovered(&conns, &found).expect("fallback advertiser");
            let error = resolve(&conns, &Target::Current, None, None, || {
                Some(ResolutionCandidate::Mdns(selected))
            })
            .expect_err("credentialed current A requires explicit recovery");
            assert!(error.report().message.contains("--pairing-code <code>"));
        }
        assert_eq!(conns.profiles["a"].fingerprint, None);
        assert_eq!(conns.profiles["a"].token.as_deref(), Some("token-a"));
    }

    #[test]
    fn no_current_profile_keeps_discovery_display_only() {
        let mut b = profile("");
        b.token = Some("token-b".to_string());
        b.fingerprint = Some("fp-b".to_string());
        let conns = conns_with(None, &[("b", b)]);
        let found = vec![DiscoveredServer {
            name: "server-b".to_string(),
            url: "ws://b:8787".to_string(),
            fingerprint: Some("fp-b".to_string()),
        }];

        let selected = select_discovered(&conns, &found).expect("advertiser");
        let error = resolve(&conns, &Target::Current, None, None, || {
            Some(ResolutionCandidate::Mdns(selected))
        })
        .expect_err("unselected advertiser must remain display-only");
        assert!(error.report().message.contains("fleety init"));
    }

    #[test]
    fn explicit_env_token_without_endpoint_never_targets_mdns() {
        let conns = Connections::default();
        let error = resolve(
            &conns,
            &Target::Current,
            None,
            Some("caller-secret".to_string()),
            || {
                Some(ResolutionCandidate::Mdns(Discovered {
                    url: "ws://rogue-advertiser:8787".to_string(),
                    fingerprint: Some("copied-fingerprint".to_string()),
                }))
            },
        )
        .expect_err("an explicit token still requires an explicit endpoint");
        assert!(error.report().message.contains("fleety init"));
    }

    #[test]
    fn resolve_mdns_copied_matching_fingerprint_never_attaches_pinned_token() {
        let mut home = profile("");
        home.token = Some("home-tok".to_string());
        home.fingerprint = Some("AA:BB".to_string());
        let conns = conns_with(Some("home"), &[("home", home)]);
        let error = resolve(&conns, &Target::Current, None, None, || {
            Some(ResolutionCandidate::Mdns(Discovered {
                url: "ws://192.168.1.20:8787".to_string(),
                fingerprint: Some("AA:BB".to_string()),
            }))
        })
        .expect_err("stored credential requires explicit repair");
        assert!(error
            .report()
            .message
            .contains("will not send the stored token"));
    }

    #[test]
    fn reselect_profile_endpoint_only_clears_credentials_when_url_changes() {
        for (token, fingerprint, expected_cleared) in [
            (None, None, false),
            (Some("token"), None, true),
            (None, Some("pin"), true),
            (Some("token"), Some("pin"), true),
        ] {
            let mut changed = Profile {
                url: "ws://old:8787".to_string(),
                token: token.map(str::to_string),
                fingerprint: fingerprint.map(str::to_string),
                ..Default::default()
            };
            assert_eq!(
                reselect_profile_endpoint(&mut changed, "ws://new:8787".to_string()),
                expected_cleared
            );
            assert_eq!(changed.url, "ws://new:8787");
            assert!(changed.token.is_none());
            assert!(changed.fingerprint.is_none());
        }

        let mut unchanged = Profile {
            url: "ws://same:8787".to_string(),
            token: Some("token".to_string()),
            fingerprint: Some("pin".to_string()),
            ..Default::default()
        };
        assert!(!reselect_profile_endpoint(
            &mut unchanged,
            "ws://same:8787".to_string()
        ));
        assert_eq!(unchanged.token.as_deref(), Some("token"));
        assert_eq!(unchanged.fingerprint.as_deref(), Some("pin"));
    }

    #[test]
    fn resolve_falls_back_to_localhost_when_nothing_configured() {
        let conns = Connections::default();
        let r = resolve(&conns, &Target::Current, None, None, || None).expect("default");
        assert_eq!(r.url, DEFAULT_URL);
        assert_eq!(r.source, Source::Default);
        assert!(r.token.is_none());
    }

    #[test]
    fn resolve_env_token_overrides_profile_token() {
        let mut home = profile("ws://home:8787");
        home.token = Some("profile-tok".to_string());
        let conns = conns_with(Some("home"), &[("home", home)]);
        let r = resolve(
            &conns,
            &Target::Current,
            None,
            Some("env-tok".to_string()),
            no_discovery,
        )
        .expect("env token");
        assert_eq!(r.token.as_deref(), Some("env-tok"));
    }

    #[test]
    fn credential_publication_sync_failure_returns_the_complete_committed_generation() {
        let p = tmp_path();
        let initial = conns_with(Some("home"), &[("home", profile("ws://home:8787"))]);
        save_at(&p, &initial).expect("seed profile");
        let target = resolve(&initial, &Target::Current, None, None, no_discovery)
            .expect("resolve durable owner");

        let commit = store_resolved_profile_credentials_recoverable_at_with_sync(
            &p,
            &target,
            Some("new-token"),
            "new-fingerprint",
            |_, _| {
                Err(std::io::Error::other(
                    "injected published credential sync failure",
                ))
            },
        )
        .expect("complete replacement is recoverable");

        let CredentialCommit::PublishedNotDurable {
            decision,
            committed,
            error,
            ..
        } = commit
        else {
            panic!("publication sync failure must preserve the committed generation");
        };
        assert_eq!(decision, PinDecision::Pin);
        assert_eq!(committed.token(), Some("new-token"));
        assert_eq!(
            committed.profile_owner_fingerprint(),
            Some("new-fingerprint")
        );
        assert!(error.report().message.contains("sync published"));

        let published = load_at(&p).expect("read published profile");
        assert_eq!(
            published.profiles["home"].token.as_deref(),
            Some("new-token")
        );
        assert_eq!(
            published.profiles["home"].fingerprint.as_deref(),
            Some("new-fingerprint")
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pairing_publication_sync_failure_returns_the_complete_committed_generation() {
        let p = tmp_path();
        let initial = conns_with(Some("home"), &[("home", profile("ws://home:8787"))]);
        save_at(&p, &initial).expect("seed profile");
        let target = resolve(&initial, &Target::Current, None, None, no_discovery)
            .expect("resolve durable owner");

        let commit = store_resolved_profile_pairing_recoverable_at_with_sync(
            &p,
            &target,
            "paired-token",
            "paired-fingerprint",
            |_, _| {
                Err(std::io::Error::other(
                    "injected published pairing sync failure",
                ))
            },
        )
        .expect("complete pairing replacement is recoverable");

        let CredentialCommit::PublishedNotDurable {
            committed, error, ..
        } = commit
        else {
            panic!("publication sync failure must preserve the paired generation");
        };
        assert_eq!(committed.token(), Some("paired-token"));
        assert_eq!(
            committed.profile_owner_fingerprint(),
            Some("paired-fingerprint")
        );
        assert!(error.report().message.contains("sync published"));

        let published = load_at(&p).expect("read published pairing");
        assert_eq!(
            published.profiles["home"].token.as_deref(),
            Some("paired-token")
        );
        assert_eq!(
            published.profiles["home"].fingerprint.as_deref(),
            Some("paired-fingerprint")
        );

        mutate_at(&p, |connections| {
            connections
                .profiles
                .get_mut("home")
                .ok_or_else(|| CoreError::Message("missing home profile".to_string()))?
                .label = Some("concurrent replacement".to_string());
            Ok(())
        })
        .expect("replace the published owner before retry");
        let retry = sync_resolved_profile_publication_at(&p, &committed)
            .expect_err("durability retry must reject owner drift");
        assert!(
            retry.report().message.contains("changed"),
            "retry must name the superseded generation: {retry}"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pairing_commit_reports_currentness_from_the_commit_lease() {
        let p = tmp_path();
        let initial = conns_with(
            Some("a"),
            &[("a", profile("ws://a:8787")), ("b", profile("ws://b:8787"))],
        );
        save_at(&p, &initial).expect("seed profiles");
        let target = resolve(
            &initial,
            &Target::Named("b".to_string()),
            None,
            None,
            no_discovery,
        )
        .expect("resolve non-current B");
        mutate_at(&p, |connections| {
            connections.current = Some("b".to_string());
            Ok(())
        })
        .expect("switch current during handshake");

        let commit =
            store_resolved_profile_pairing_recoverable_at(&p, &target, "new-token", "new-pin")
                .expect("pair unchanged B");

        assert!(
            matches!(
                commit,
                CredentialCommit::Durable {
                    profile_is_current: true,
                    ..
                }
            ),
            "notification decision must reflect current-at-commit"
        );
        let _ = std::fs::remove_file(&p);
    }
}
