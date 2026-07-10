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

use std::collections::HashMap;
use std::path::PathBuf;

use agent_core::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Server,
    Daemon,
    Cli,
    Shared,
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
            default: "127.0.0.1:8787",
            description: "WebSocket listen address.",
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
            default: "0",
            description: "Require a token to connect (1/0).",
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
            key: "FLEETY_VOICE_AUDIO",
            scope: Cli,
            default: "auto",
            description: "Voice transport: auto (send audio iff model accepts it) / on / off (local STT).",
            secret: false,
            validator: Some(v_voice_audio),
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
            default: "UTC",
            description: "Fallback timezone for display (IANA).",
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
            key: "FLEETY_AUTO_INSTALL_DEPS",
            scope: Shared,
            default: "1",
            description: "Auto-install missing dependencies at boot (1/0).",
            secret: false,
            validator: Some(v_bool),
        },
        // FLEETY_AGENT_URL is deliberately NOT a registry setting: the connection
        // target lives in ~/.fleety/connections.toml and is managed via
        // `fleety server` (see fleety_tools::connection). It survives only as a
        // transient env override in the shared resolver, never seeded from
        // config.toml — so there is no config.json / config.toml / env precedence
        // trap. `config set FLEETY_AGENT_URL` is therefore an unknown key.
        Setting {
            key: "FLEETY_DEVICE_ID",
            scope: Daemon,
            default: "(hostname)",
            description: "This device's id.",
            secret: false,
            validator: None,
        },
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
        Setting {
            key: "FLEETY_CODEX_CLIENT_ID",
            scope: Shared,
            // The Codex CLI public client id (same value the upstream Codex CLI and
            // other clients use). Overridable, but this default lets login work
            // out of the box.
            default: "app_EMoamEEZ73f0CkXaXp7hrann",
            description: "Codex ChatGPT OAuth public client id.",
            secret: false,
            validator: None,
        },
        Setting {
            key: "FLEETY_CODEX_AUTHORIZE_URL",
            scope: Shared,
            default: "https://auth.openai.com/oauth/authorize",
            description: "Codex OAuth authorization endpoint.",
            secret: false,
            validator: Some(v_url),
        },
        Setting {
            key: "FLEETY_CODEX_TOKEN_URL",
            scope: Shared,
            default: "https://auth.openai.com/oauth/token",
            description: "Codex OAuth token endpoint.",
            secret: false,
            validator: Some(v_url),
        },
        Setting {
            key: "FLEETY_CODEX_BACKEND_URL",
            scope: Shared,
            default: "https://chatgpt.com/backend-api/codex",
            description: "Codex OAuth backend base URL for model calls.",
            secret: false,
            validator: Some(v_url),
        },
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
    check(value).map_err(|why| CoreError::Message(format!("invalid value for {}: {why}", setting.key)))
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
    std::fs::write(path, text).map_err(|e| CoreError::Message(format!("write config: {e}")))?;
    Ok(())
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
        Some("provider" | "group" | "role") => match args.get(1).map(String::as_str) {
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
        Some("edit") => Command::Edit,
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
    registry()
        .iter()
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

/// Run a `config` subcommand against the config file. `edit` is the line-based
/// loop; the CLI overrides `edit` with a ratatui screen when stdout is a TTY.
pub fn run(args: &[String]) -> Result<()> {
    // Interactive flat-key edit stays local + line-based; everything else
    // renders to text (so the same code serves the remote handler).
    let is_providers = matches!(
        args.first().map(String::as_str),
        Some("provider" | "group" | "role")
    );
    if !is_providers && matches!(parse(args), Command::Edit) {
        return edit_line_based(&config_path());
    }
    let out = run_rendered(args)?;
    let out = out.trim_end_matches('\n');
    if !out.is_empty() {
        println!("{out}");
    }
    Ok(())
}

/// Run a `config` subcommand and return its rendered text instead of printing,
/// so the remote handler can send it over the wire. `provider`/`group`/`role`
/// manage the structured providers.toml; everything else is the flat-key
/// registry. `edit` is interactive and has no rendered form (an error here).
pub fn run_rendered(args: &[String]) -> Result<String> {
    if matches!(
        args.first().map(String::as_str),
        Some("provider" | "group" | "role")
    ) {
        return run_providers_at(&pc::providers_path(), args);
    }
    let path = config_path();
    Ok(match parse(args) {
        Command::List => {
            let map = load(&path);
            let mut out = String::from("Settings (env → config → default; secrets masked):\n\n");
            for (key, scope, value, source) in rows(&map) {
                out.push_str(&format!("  [{scope:6}] {key:<26} = {value}  ({source})\n"));
            }
            out.push_str(&format!(
                "\nEdit with: config set <KEY> <VALUE>   (file: {})",
                path.display()
            ));
            out
        }
        Command::Get(key) => {
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
            let setting = find(&key).ok_or_else(|| {
                CoreError::Message(format!(
                    "unknown setting '{key}'. Run `config list` to see valid keys."
                ))
            })?;
            // Reject out-of-domain values before touching the file, so a typo
            // never lands silently (server/daemon and remote `--target` all
            // funnel through here).
            validate(setting, &value)?;
            let mut map = load(&path);
            map.insert((setting.scope, key.clone()), value);
            save(&path, &map)?;
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
            let setting =
                find(&key).ok_or_else(|| CoreError::Message(format!("unknown setting '{key}'")))?;
            let mut map = load(&path);
            map.remove(&(setting.scope, key.clone()));
            save(&path, &map)?;
            format!("unset {key} (reverts to env/default)")
        }
        Command::Edit => {
            return Err(CoreError::Message(
                "`config edit` is interactive — run it locally on a TTY".to_string(),
            ))
        }
        Command::Help => {
            "usage: config [list | get <KEY> | set <KEY> <VALUE> | unset <KEY> | edit]".to_string()
        }
    })
}

// ---- providers.toml subcommands (provider / group / role) ----

use crate::providers_config::{self as pc, GroupSpec, ProviderSpec, Strategy};

/// A parsed `provider` / `group` / `role` subcommand over `providers.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCmd {
    ProviderAdd(ProviderSpec),
    /// Update named provider's fields (only `Some` fields change).
    ProviderSet {
        name: String,
        base_url: Option<String>,
        model: Option<String>,
        key: Option<String>,
        stream: Option<bool>,
        modalities: Option<String>,
        effort: Option<String>,
    },
    ProviderRemove(String),
    ProviderList,
    GroupSet {
        name: String,
        members: Vec<String>,
        strategy: Strategy,
    },
    GroupRemove(String),
    GroupList,
    RoleSet {
        role: String,
        target: String,
    },
    RoleUnset(String),
    RoleList,
}

fn strategy_from(s: &str) -> Result<Strategy> {
    match s {
        "round_robin" => Ok(Strategy::RoundRobin),
        "failover" => Ok(Strategy::Failover),
        _ => Err(CoreError::Message(format!(
            "invalid strategy '{s}' (expected round_robin or failover)"
        ))),
    }
}

/// Split `--flag value` / bare `--flag` tokens into a (key→value, bare-set)
/// pair. A `--flag` that needs a value but has none is an error; a non-flag
/// token here is unexpected.
fn split_flags(
    args: &[String],
) -> Result<(HashMap<String, String>, std::collections::HashSet<String>)> {
    let mut kv = HashMap::new();
    let mut bare = std::collections::HashSet::new();
    let mut i = 0;
    while i < args.len() {
        let Some(name) = args[i].strip_prefix("--") else {
            return Err(CoreError::Message(format!(
                "unexpected argument '{}'",
                args[i]
            )));
        };
        if name == "stream" {
            bare.insert(name.to_string());
            i += 1;
        } else {
            let v = args
                .get(i + 1)
                .ok_or_else(|| CoreError::Message(format!("flag --{name} needs a value")))?;
            kv.insert(name.to_string(), v.clone());
            i += 2;
        }
    }
    Ok((kv, bare))
}

/// Error if any flags were given but not consumed (catches typos/unknown flags).
fn no_unknown_flags(kv: &HashMap<String, String>) -> Result<()> {
    if let Some(k) = kv.keys().next() {
        return Err(CoreError::Message(format!("unknown flag --{k}")));
    }
    Ok(())
}

/// Parse a `provider` / `group` / `role` subcommand. Pure and unit-testable; an
/// unknown verb, missing required field, bad strategy, or unknown flag is an
/// error.
pub fn parse_providers(args: &[String]) -> Result<ProviderCmd> {
    let kind = args.first().map(String::as_str);
    let verb = args.get(1).map(String::as_str);
    let rest = if args.len() > 2 { &args[2..] } else { &[][..] };
    let need = |o: Option<&String>, what: &str| -> Result<String> {
        o.cloned()
            .ok_or_else(|| CoreError::Message(format!("missing {what}")))
    };
    match (kind, verb) {
        (Some("provider"), Some("list")) => Ok(ProviderCmd::ProviderList),
        (Some("provider"), Some("remove")) => {
            Ok(ProviderCmd::ProviderRemove(need(rest.first(), "provider name")?))
        }
        (Some("provider"), Some("add")) => {
            let name = need(rest.first(), "provider name")?;
            let (mut kv, bare) = split_flags(rest.get(1..).unwrap_or(&[]))?;
            let base_url = kv
                .remove("base-url")
                .ok_or_else(|| CoreError::Message("missing --base-url".to_string()))?;
            let model = kv
                .remove("model")
                .ok_or_else(|| CoreError::Message("missing --model".to_string()))?;
            let key = kv.remove("key");
            let modalities = kv.remove("modalities");
            let effort = kv.remove("effort");
            let auth = kv.remove("auth");
            no_unknown_flags(&kv)?;
            Ok(ProviderCmd::ProviderAdd(ProviderSpec {
                name,
                base_url,
                model,
                key,
                stream: bare.contains("stream"),
                modalities,
                effort,
                auth,
            }))
        }
        (Some("provider"), Some("set")) => {
            let name = need(rest.first(), "provider name")?;
            let (mut kv, bare) = split_flags(rest.get(1..).unwrap_or(&[]))?;
            let base_url = kv.remove("base-url");
            let model = kv.remove("model");
            let key = kv.remove("key");
            let modalities = kv.remove("modalities");
            let effort = kv.remove("effort");
            no_unknown_flags(&kv)?;
            let stream = if bare.contains("stream") {
                Some(true)
            } else {
                None
            };
            Ok(ProviderCmd::ProviderSet {
                name,
                base_url,
                model,
                key,
                stream,
                modalities,
                effort,
            })
        }
        (Some("group"), Some("list")) => Ok(ProviderCmd::GroupList),
        (Some("group"), Some("remove")) => {
            Ok(ProviderCmd::GroupRemove(need(rest.first(), "group name")?))
        }
        (Some("group"), Some("set")) => {
            let name = need(rest.first(), "group name")?;
            let (mut kv, _bare) = split_flags(rest.get(1..).unwrap_or(&[]))?;
            let members = kv
                .remove("members")
                .ok_or_else(|| CoreError::Message("missing --members".to_string()))?
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            let strategy = strategy_from(
                &kv.remove("strategy")
                    .ok_or_else(|| CoreError::Message("missing --strategy".to_string()))?,
            )?;
            no_unknown_flags(&kv)?;
            Ok(ProviderCmd::GroupSet {
                name,
                members,
                strategy,
            })
        }
        (Some("role"), Some("list")) => Ok(ProviderCmd::RoleList),
        (Some("role"), Some("unset")) => {
            Ok(ProviderCmd::RoleUnset(need(rest.first(), "role name")?))
        }
        (Some("role"), Some("set")) => {
            let role = need(rest.first(), "role name")?;
            let target = need(rest.get(1), "target provider/group name")?;
            Ok(ProviderCmd::RoleSet { role, target })
        }
        _ => Err(CoreError::Message(
            "usage: config provider <add|set|remove|list> | group <set|remove|list> | role <set|unset|list>"
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

/// Execute a `provider` / `group` / `role` subcommand against `providers.toml`:
/// load (empty when missing, error when present-but-broken), apply, validate,
/// and write atomically.
pub fn run_providers(args: &[String]) -> Result<()> {
    let out = run_providers_at(&pc::providers_path(), args)?;
    let out = out.trim_end_matches('\n');
    if !out.is_empty() {
        println!("{out}");
    }
    Ok(())
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
            for p in &cfg.providers {
                out.push_str(&format!(
                    "  {:<16} {} [{}] key={} stream={}\n",
                    p.name,
                    p.base_url,
                    p.model,
                    mask_key(&p.key),
                    p.stream
                ));
            }
            Ok(out)
        }
        ProviderCmd::ProviderAdd(spec) => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.provider(&spec.name).is_some() {
                return Err(CoreError::Message(format!(
                    "provider '{}' already exists (use `provider set` to change it)",
                    spec.name
                )));
            }
            let name = spec.name.clone();
            cfg.providers.push(spec);
            pc::write_providers(path, &cfg)?;
            Ok(format!("added provider '{name}'"))
        }
        ProviderCmd::ProviderSet {
            name,
            base_url,
            model,
            key,
            stream,
            modalities,
            effort,
        } => {
            let mut cfg = pc::load_or_default(path)?;
            let p = cfg
                .providers
                .iter_mut()
                .find(|p| p.name == name)
                .ok_or_else(|| CoreError::Message(format!("no such provider '{name}'")))?;
            if let Some(v) = base_url {
                p.base_url = v;
            }
            if let Some(v) = model {
                p.model = v;
            }
            if key.is_some() {
                p.key = key;
            }
            if let Some(v) = stream {
                p.stream = v;
            }
            if modalities.is_some() {
                p.modalities = modalities;
            }
            if effort.is_some() {
                p.effort = effort;
            }
            pc::write_providers(path, &cfg)?;
            Ok(format!("updated provider '{name}'"))
        }
        ProviderCmd::ProviderRemove(name) => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.provider(&name).is_none() {
                return Err(CoreError::Message(format!("no such provider '{name}'")));
            }
            if let Some(g) = cfg.groups.iter().find(|g| g.members.contains(&name)) {
                return Err(CoreError::Message(format!(
                    "cannot remove provider '{name}': group '{}' references it",
                    g.name
                )));
            }
            if let Some((r, _)) = cfg.roles.iter().find(|(_, t)| **t == name) {
                return Err(CoreError::Message(format!(
                    "cannot remove provider '{name}': role '{r}' references it"
                )));
            }
            cfg.providers.retain(|p| p.name != name);
            pc::write_providers(path, &cfg)?;
            Ok(format!("removed provider '{name}'"))
        }
        ProviderCmd::GroupList => {
            let cfg = pc::load_or_default(path)?;
            let mut out = String::new();
            if cfg.groups.is_empty() {
                out.push_str(&format!("(no groups defined in {})\n", path.display()));
            }
            for g in &cfg.groups {
                out.push_str(&format!(
                    "  {:<16} [{:?}] {}\n",
                    g.name,
                    g.strategy,
                    g.members.join(", ")
                ));
            }
            Ok(out)
        }
        ProviderCmd::GroupSet {
            name,
            members,
            strategy,
        } => {
            let mut cfg = pc::load_or_default(path)?;
            cfg.groups.retain(|g| g.name != name);
            cfg.groups.push(GroupSpec {
                name: name.clone(),
                members,
                strategy,
            });
            pc::write_providers(path, &cfg)?;
            Ok(format!("set group '{name}'"))
        }
        ProviderCmd::GroupRemove(name) => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.group(&name).is_none() {
                return Err(CoreError::Message(format!("no such group '{name}'")));
            }
            if let Some((r, _)) = cfg.roles.iter().find(|(_, t)| **t == name) {
                return Err(CoreError::Message(format!(
                    "cannot remove group '{name}': role '{r}' references it"
                )));
            }
            cfg.groups.retain(|g| g.name != name);
            pc::write_providers(path, &cfg)?;
            Ok(format!("removed group '{name}'"))
        }
        ProviderCmd::RoleSet { role, target } => {
            let mut cfg = pc::load_or_default(path)?;
            cfg.roles.insert(role.clone(), target.clone());
            pc::write_providers(path, &cfg)?;
            Ok(format!("bound role '{role}' → '{target}'"))
        }
        ProviderCmd::RoleUnset(role) => {
            let mut cfg = pc::load_or_default(path)?;
            if cfg.roles.remove(&role).is_none() {
                return Err(CoreError::Message(format!("no such role '{role}'")));
            }
            pc::write_providers(path, &cfg)?;
            Ok(format!("unset role '{role}'"))
        }
        ProviderCmd::RoleList => {
            let cfg = pc::load_or_default(path)?;
            let mut out = String::new();
            if cfg.roles.is_empty() {
                out.push_str(&format!("(no roles defined in {})\n", path.display()));
            }
            for (r, t) in &cfg.roles {
                out.push_str(&format!("  {r:<16} → {t}\n"));
            }
            Ok(out)
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
            println!("  {i:>2}) [{scope:6}] {key:<26} = {value}  ({source})");
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
            println!("{e}");
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
        assert_eq!(parse(&v(&["get"])), Command::Help); // missing operand
        assert_eq!(parse(&v(&["set", "X"])), Command::Help);
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
    fn codex_oauth_settings_registered_with_defaults() {
        let cid = find("FLEETY_CODEX_CLIENT_ID").expect("client id registered");
        assert!(cid.default.starts_with("app_")); // the Codex public client id
        assert!(!cid.secret); // not a secret (it's a public client id)
        let auth_url = find("FLEETY_CODEX_AUTHORIZE_URL").expect("authorize url");
        assert!(auth_url.default.starts_with("https://"));
        assert!(find("FLEETY_CODEX_TOKEN_URL").is_some());
        assert!(find("FLEETY_CODEX_BACKEND_URL").is_some());
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
    fn unknown_key_is_rejected() {
        assert!(find("FLEETY_NOPE").is_none());
        assert!(find("FLEETY_ADDR").is_some());
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
        map.insert((Scope::Daemon, "FLEETY_AGENT_URL".into()), "ws://seeded".into());
        seed_env_from_config(&map);
        assert!(std::env::var("FLEETY_AGENT_URL").is_err(), "must not seed a non-registry key");
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
        // provider add → a fully-populated spec.
        let cmd = parse_providers(&v(&[
            "provider",
            "add",
            "codex-1",
            "--base-url",
            "https://x/v1",
            "--model",
            "gpt-5",
            "--key",
            "sk",
            "--stream",
            "--modalities",
            "text,image",
            "--effort",
            "high",
        ]))
        .expect("add parses");
        match cmd {
            ProviderCmd::ProviderAdd(s) => {
                assert_eq!(s.name, "codex-1");
                assert_eq!(s.base_url, "https://x/v1");
                assert!(s.stream);
                assert_eq!(s.modalities.as_deref(), Some("text,image"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // group set with a strategy.
        assert_eq!(
            parse_providers(&v(&[
                "group",
                "set",
                "codex",
                "--members",
                "codex-1,codex-2",
                "--strategy",
                "round_robin",
            ]))
            .unwrap(),
            ProviderCmd::GroupSet {
                name: "codex".into(),
                members: vec!["codex-1".into(), "codex-2".into()],
                strategy: Strategy::RoundRobin,
            }
        );
        // role set is two positionals.
        assert_eq!(
            parse_providers(&v(&["role", "set", "main", "codex"])).unwrap(),
            ProviderCmd::RoleSet {
                role: "main".into(),
                target: "codex".into()
            }
        );
        // Errors: unknown verb, bad strategy, unknown flag, missing required.
        assert!(parse_providers(&v(&["provider", "frobnicate"])).is_err());
        assert!(parse_providers(&v(&[
            "group",
            "set",
            "g",
            "--members",
            "a",
            "--strategy",
            "random"
        ]))
        .is_err());
        assert!(parse_providers(&v(&[
            "provider",
            "add",
            "p",
            "--base-url",
            "u",
            "--model",
            "m",
            "--bogus",
            "x"
        ]))
        .is_err());
        assert!(parse_providers(&v(&["provider", "add", "p", "--model", "m"])).is_err());
    }

    #[test]
    fn config_effect_by_verb() {
        let eff = |p: &[&str]| config_effect(&v(p));
        // providers.toml mutations → next connection.
        assert_eq!(
            eff(&["provider", "add", "p", "--base-url", "u", "--model", "m"]),
            Some(ConfigEffect::NextConnection)
        );
        assert_eq!(
            eff(&[
                "group",
                "set",
                "g",
                "--members",
                "p",
                "--strategy",
                "failover"
            ]),
            Some(ConfigEffect::NextConnection)
        );
        assert_eq!(
            eff(&["role", "set", "main", "g"]),
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
    }

    #[test]
    fn run_providers_add_list_and_guard_removal() {
        let path = std::env::temp_dir().join(format!("fleety-cfg-{}.toml", uuid::Uuid::new_v4()));
        // Add two providers, then a group over them.
        run_providers_at(
            &path,
            &v(&["provider", "add", "p1", "--base-url", "u1", "--model", "m"]),
        )
        .unwrap();
        run_providers_at(
            &path,
            &v(&["provider", "add", "p2", "--base-url", "u2", "--model", "m"]),
        )
        .unwrap();
        run_providers_at(
            &path,
            &v(&[
                "group",
                "set",
                "g",
                "--members",
                "p1,p2",
                "--strategy",
                "failover",
            ]),
        )
        .unwrap();
        // Adding a duplicate name fails and leaves the file unchanged.
        assert!(run_providers_at(
            &path,
            &v(&["provider", "add", "p1", "--base-url", "u", "--model", "m"])
        )
        .is_err());
        // A referenced provider can't be removed (the group names it).
        let err = run_providers_at(&path, &v(&["provider", "remove", "p1"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("group 'g'"), "got: {err}");
        // The file still parses with both providers + the group.
        let cfg = pc::load_from(&path).expect("re-read");
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.groups.len(), 1);
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
            ("FLEETY_BACKUP_INTERVAL_SECS", "3600", "hourly"),
            ("FLEETY_PRESENCE_INTERVAL_SECS", "300", "5m"),
            ("FLEETY_VOICE_AUDIO_MAX_KB", "2048", "big"),
            ("FLEETY_MODEL_BASE_URL", "https://api.x/v1", "notaurl"),
            ("FLEETY_CODEX_AUTHORIZE_URL", "https://auth/x", "ftp://x"),
            ("FLEETY_CODEX_TOKEN_URL", "http://auth/x", "auth/x"),
            ("FLEETY_CODEX_BACKEND_URL", "https://b/x", "b"),
        ];
        for &(key, good, bad) in cases {
            let s = find(key).unwrap_or_else(|| panic!("{key} registered"));
            assert!(s.validator.is_some(), "{key} should carry a validator");
            assert!(validate(s, good).is_ok(), "{key}: '{good}' should pass");
            let err = validate(s, bad).unwrap_err().to_string();
            assert!(err.contains(key), "{key}: error should name the key, got: {err}");
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
        assert!(!path.exists(), "an invalid set must not create the config file");

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
