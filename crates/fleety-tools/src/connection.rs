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
use std::path::{Path, PathBuf};

use agent_core::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// The connect target when nothing is configured and mDNS finds nothing.
pub const DEFAULT_URL: &str = "ws://127.0.0.1:8787";

/// One named server: its WebSocket URL plus an optional paired token, a human
/// label, and the server fingerprint pinned at enrollment (used to guard mDNS).
/// A profile may be url-less (token only, relies on mDNS) — [`resolve`] then
/// falls through to discovery rather than treating an empty url as a target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
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

/// Persist connections to `path` atomically (temp file + rename, so a crash or
/// concurrent write never leaves a half-written file) with `0600` permissions
/// on Unix (the file may hold tokens). The parent dir is created if missing.
pub fn save_at(path: &Path, conns: &Connections) -> Result<()> {
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
    std::fs::write(&tmp, &text)
        .map_err(|e| CoreError::Message(format!("write connections.toml: {e}")))?;
    set_owner_only(&tmp);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Message(format!("replace connections.toml: {e}")));
    }
    Ok(())
}

/// Persist connections to [`connections_path`] (see [`save_at`]).
pub fn save(conns: &Connections) -> Result<()> {
    save_at(&connections_path(), conns)
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
///   with an **empty** `url`, so [`resolve`] falls through to mDNS rather than
///   pinning `localhost`.
/// - **Backup, not delete:** the old `config.json` / `fleetyd.token` are renamed
///   to `*.migrated` (kept for rollback), never removed.
/// - **Concurrency-safe:** `connections.toml` is created with an `O_EXCL`
///   (`create_new`) latch, so the CLI and a same-host daemon starting at once
///   cannot each migrate and mint two device ids — the loser leaves the winner's
///   file untouched.
pub fn migrate_from_config_json_at(dir: &Path) -> Result<Migration> {
    let conns_path = dir.join("connections.toml");
    if conns_path.exists() {
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
    // url-less record (token only) keeps `url` empty so `resolve` uses mDNS.
    if agent_url.is_some() || token.is_some() {
        conns.profiles.insert(
            "default".to_string(),
            Profile {
                url: agent_url.unwrap_or_default(),
                token,
                label: None,
                fingerprint: None,
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
    /// No override — use the normal precedence (current profile → mDNS → local).
    #[default]
    Current,
    /// `-s/--server <name>`: use that named profile (its url + token).
    Named(String),
    /// `--url <ws>`: connect to this url directly (token only if a profile pins it).
    Url(String),
}

/// A server discovered on the LAN: its url and (optionally) the fingerprint it
/// presents, used to decide whether a pinned profile's token may be sent to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub url: String,
    pub fingerprint: Option<String>,
}

/// Where a resolved connection target came from — so callers can surface the
/// right hint (an `env` override banner, a "discovered on the LAN" note, or the
/// "no server configured, using localhost" fallback message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A single-shot `-s <name>` / `--url <ws>` override.
    Override,
    /// The `FLEETY_AGENT_URL` env var (temporary; never written).
    Env,
    /// The current profile (carries its name).
    Profile(String),
    /// Discovered on the LAN via mDNS.
    Mdns,
    /// The built-in localhost default.
    Default,
}

/// A resolved connection target: the url, the token to authenticate with (if
/// any), and where it came from. The `(url, token)` pair is the connect input;
/// `source` lets the caller print the appropriate hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub url: String,
    pub token: Option<String>,
    pub source: Source,
}

/// Resolve which server (and token) to connect to, by the single precedence
/// shared between the CLI and the daemon:
///
/// 1. `over` — a single-shot `-s <name>` / `--url <ws>` override.
/// 2. `env_url` — `FLEETY_AGENT_URL` (temporary; never written back).
/// 3. the current profile's url + token (**sticky**: once set, mDNS is skipped).
/// 4. mDNS discovery (only when there is no usable current profile; a pinned
///    profile's token is sent **only** to a discovered server whose fingerprint
///    matches — a rogue advertiser with a different/absent fingerprint gets no
///    token).
/// 5. the localhost default ([`DEFAULT_URL`]).
///
/// `env_token` (`FLEETY_TOKEN`) is an explicit token override that wins in every
/// branch. `mdns` is injected so resolution is pure and unit-testable — real
/// callers pass a LAN probe; it is invoked at most once, only for the mDNS
/// branch. Errors only when `over` names a profile that doesn't exist or is
/// url-less. This function performs no I/O of its own (it never writes), so an
/// `env`/`--url` override cannot mutate the persisted profiles.
pub fn resolve(
    conns: &Connections,
    over: &Target,
    env_url: Option<String>,
    env_token: Option<String>,
    mdns: impl FnOnce() -> Option<Discovered>,
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
            return Ok(Resolved {
                url: p.url.clone(),
                token: env_token.or_else(|| p.token.clone()),
                source: Source::Override,
            });
        }
        Target::Url(u) => {
            let tok = conns
                .profiles
                .values()
                .find(|p| p.url == *u)
                .and_then(|p| p.token.clone());
            return Ok(Resolved {
                url: u.clone(),
                token: env_token.or(tok),
                source: Source::Override,
            });
        }
        Target::Current => {}
    }

    if let Some(u) = env_url.filter(|s| !s.is_empty()) {
        // Temporary env override: keep the current profile's token available (so
        // a unit-file `FLEETY_AGENT_URL` deployment still authenticates), but the
        // url is never persisted.
        let tok = conns.current_profile().and_then(|p| p.token.clone());
        return Ok(Resolved {
            url: u,
            token: env_token.or(tok),
            source: Source::Env,
        });
    }

    // Sticky: once the current profile has a url, return it and never query mDNS
    // — an enrolled device does not drift to a LAN advertiser.
    if let Some(name) = conns.current.as_ref() {
        if let Some(p) = conns.profiles.get(name) {
            if !p.url.is_empty() {
                return Ok(Resolved {
                    url: p.url.clone(),
                    token: env_token.or_else(|| p.token.clone()),
                    source: Source::Profile(name.clone()),
                });
            }
        }
    }

    // mDNS fallback (no usable current profile url). Fingerprint guard: a pinned
    // profile's token is handed to the discovered server only when the
    // fingerprints match; a mismatched/absent fingerprint yields no token.
    if let Some(disc) = mdns() {
        let tok = disc.fingerprint.as_deref().and_then(|fp| {
            conns
                .profiles
                .values()
                .find(|p| p.fingerprint.as_deref() == Some(fp))
                .and_then(|p| p.token.clone())
        });
        return Ok(Resolved {
            url: disc.url,
            token: env_token.or(tok),
            source: Source::Mdns,
        });
    }

    Ok(Resolved {
        url: DEFAULT_URL.to_string(),
        token: env_token,
        source: Source::Default,
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

// ---- collecting discovery + fingerprint pinning + sticky healing ----

/// One server found during a collecting LAN scan (guided init, sticky healing).
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

/// Apply [`tofu_pin_decision`] to the CURRENT profile, persisting a new pin.
/// Returns the decision so callers can warn on `IdentityChanged`.
pub fn pin_current_fingerprint(seen: &str) -> Result<PinDecision> {
    let mut conns = load()?;
    let Some(name) = conns.current.clone() else {
        return Ok(PinDecision::AlreadyPinned); // no current profile → nothing to pin onto
    };
    let Some(profile) = conns.profiles.get_mut(&name) else {
        return Ok(PinDecision::AlreadyPinned);
    };
    let decision = tofu_pin_decision(profile.fingerprint.as_deref(), seen);
    if decision == PinDecision::Pin {
        profile.fingerprint = Some(seen.to_string());
        save(&conns)?;
    }
    Ok(decision)
}

/// The sticky-heal candidate: among `found`, the advertiser whose fingerprint
/// exactly equals `pinned` and whose URL differs from the failing one. Anything
/// fingerprint-less or different is excluded — the stored token is never handed
/// to an unverified advertiser. Pure.
pub fn heal_candidate(pinned: &str, old_url: &str, found: &[DiscoveredServer]) -> Option<String> {
    found
        .iter()
        .find(|d| d.fingerprint.as_deref() == Some(pinned) && d.url != old_url)
        .map(|d| d.url.clone())
}

/// One sticky-heal attempt after a failed connect to the current profile's
/// `old_url`: scan the LAN, adopt ONLY an advertiser matching the pinned
/// fingerprint, persist its URL onto the profile, and return it. `None` (no
/// pin, disabled mDNS, no match, or persistence failure) leaves everything
/// untouched — the caller surfaces the original connection failure.
pub fn heal_current_profile(old_url: &str) -> Option<String> {
    if std::env::var("FLEETY_MDNS_DISABLED").is_ok() {
        return None;
    }
    let mut conns = load().ok()?;
    let name = conns.current.clone()?;
    let pinned = conns.profiles.get(&name)?.fingerprint.clone()?;
    let found = discover_all_via_mdns(std::time::Duration::from_secs(3));
    let new_url = heal_candidate(&pinned, old_url, &found)?;
    if let Some(profile) = conns.profiles.get_mut(&name) {
        profile.url = new_url.clone();
    }
    save(&conns).ok()?;
    Some(new_url)
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
    fn heal_adopts_only_the_pinned_fingerprint_at_a_new_url() {
        let mk = |name: &str, url: &str, fp: Option<&str>| DiscoveredServer {
            name: name.into(),
            url: url.into(),
            fingerprint: fp.map(String::from),
        };
        let found = vec![
            mk("other", "ws://10.0.0.9:8787", Some("fp-other")),
            mk("bare", "ws://10.0.0.8:8787", None),
            mk("mine", "ws://10.0.0.7:8787", Some("fp-mine")),
        ];
        // Only the exact pinned fingerprint is adopted — never the others.
        assert_eq!(
            heal_candidate("fp-mine", "ws://10.0.0.2:8787", &found),
            Some("ws://10.0.0.7:8787".to_string())
        );
        // A match at the SAME (failing) url is not a heal.
        assert_eq!(
            heal_candidate("fp-mine", "ws://10.0.0.7:8787", &found),
            None
        );
        // No match → nothing adopted.
        assert_eq!(heal_candidate("fp-unknown", "ws://x", &found), None);
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
        // Token but no agent_url → url-less; resolve should later fall to mDNS.
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
        // url-less (relies on env/mDNS) but the token is preserved so the daemon
        // keeps authenticating without re-pairing.
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

    /// An mDNS stub that must never run — it panics if the resolver queries it.
    fn no_mdns() -> Option<Discovered> {
        panic!("mDNS must not be queried on this path");
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
        let r = resolve(&conns, &Target::Named("work".into()), None, None, no_mdns).expect("named");
        assert_eq!(r.url, "ws://work:8787");
        assert_eq!(r.token.as_deref(), Some("work-tok"));
        assert_eq!(r.source, Source::Override);
        // An unknown name is an error, not a silent fallback.
        assert!(resolve(&conns, &Target::Named("ghost".into()), None, None, no_mdns).is_err());
    }

    #[test]
    fn resolve_url_override_is_direct_with_pinned_token_only() {
        let conns = conns_with(Some("home"), &[("home", profile("ws://home:8787"))]);
        // A url with no matching profile → no token.
        let r = resolve(
            &conns,
            &Target::Url("ws://adhoc:9000".into()),
            None,
            None,
            no_mdns,
        )
        .expect("url");
        assert_eq!(r.url, "ws://adhoc:9000");
        assert!(r.token.is_none());
        assert_eq!(r.source, Source::Override);
    }

    #[test]
    fn resolve_env_url_wins_over_current_and_is_marked_env() {
        // Current profile home .20 exists, but FLEETY_AGENT_URL overrides it.
        let mut home = profile("ws://192.168.1.20:8787");
        home.token = Some("home-tok".to_string());
        let conns = conns_with(Some("home"), &[("home", home)]);
        let r = resolve(
            &conns,
            &Target::Current,
            Some("ws://env-override:8787".to_string()),
            None,
            no_mdns,
        )
        .expect("env");
        assert_eq!(r.url, "ws://env-override:8787");
        assert_eq!(r.source, Source::Env);
        // The current profile's token stays available for a unit-file deployment.
        assert_eq!(r.token.as_deref(), Some("home-tok"));
    }

    #[test]
    fn resolve_sticky_current_profile_never_queries_mdns() {
        // Spec Example: current="home", home.url=ws://192.168.1.20:8787; an mDNS
        // advertiser (ws://192.168.1.99:8787) is present but must be ignored.
        let conns = conns_with(Some("home"), &[("home", profile("ws://192.168.1.20:8787"))]);
        // no_mdns panics if called — proving the resolver never queries mDNS.
        let r = resolve(&conns, &Target::Current, None, None, no_mdns).expect("sticky");
        assert_eq!(r.url, "ws://192.168.1.20:8787");
        assert_eq!(r.source, Source::Profile("home".to_string()));
    }

    #[test]
    fn resolve_mdns_fingerprint_guard_withholds_mismatched_token() {
        // Spec Example: no current profile; home pinned fingerprint AA:BB + token;
        // mDNS resolves ws://192.168.1.99:8787 presenting CC:DD → no token.
        let mut home = profile(""); // url-less: token-only, relies on mDNS
        home.token = Some("home-tok".to_string());
        home.fingerprint = Some("AA:BB".to_string());
        let conns = conns_with(None, &[("home", home)]);
        let r = resolve(&conns, &Target::Current, None, None, || {
            Some(Discovered {
                url: "ws://192.168.1.99:8787".to_string(),
                fingerprint: Some("CC:DD".to_string()),
            })
        })
        .expect("mdns");
        assert_eq!(r.url, "ws://192.168.1.99:8787");
        assert_eq!(r.source, Source::Mdns);
        assert!(
            r.token.is_none(),
            "mismatched fingerprint must not receive the token"
        );
    }

    #[test]
    fn resolve_mdns_matching_fingerprint_attaches_pinned_token() {
        let mut home = profile("");
        home.token = Some("home-tok".to_string());
        home.fingerprint = Some("AA:BB".to_string());
        let conns = conns_with(None, &[("home", home)]);
        let r = resolve(&conns, &Target::Current, None, None, || {
            Some(Discovered {
                url: "ws://192.168.1.20:8787".to_string(),
                fingerprint: Some("AA:BB".to_string()),
            })
        })
        .expect("mdns");
        assert_eq!(r.token.as_deref(), Some("home-tok"));
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
            no_mdns,
        )
        .expect("env token");
        assert_eq!(r.token.as_deref(), Some("env-tok"));
    }
}
