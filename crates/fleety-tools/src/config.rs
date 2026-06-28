//! Typed, file-backed configuration shared by the server, daemon, and CLI.
//!
//! A curated registry of known settings (each a `FLEETY_*` name) persists to
//! `~/.fleety/config.toml`, sectioned by scope. Read precedence is **env → config
//! → default**: an explicit environment variable always wins, so existing
//! env-based deployments are unaffected; config.toml only fills what env leaves
//! unset. The CLI edits this; the server/daemon seed their env from it at boot.

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

/// A known setting: its canonical key (== its `FLEETY_*` env name), scope,
/// default, one-line description, and whether it holds a secret (masked in
/// display).
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub key: &'static str,
    pub scope: Scope,
    pub default: &'static str,
    pub description: &'static str,
    pub secret: bool,
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
        },
        Setting {
            key: "FLEETY_WORKSPACE",
            scope: Server,
            default: "(cwd)",
            description: "Fallback workspace root for tools.",
            secret: false,
        },
        Setting {
            key: "FLEETY_POLICY",
            scope: Server,
            default: "full_access",
            description: "full_access or require_approval.",
            secret: false,
        },
        Setting {
            key: "FLEETY_REQUIRE_AUTH",
            scope: Server,
            default: "0",
            description: "Require a token to connect (1/0).",
            secret: false,
        },
        Setting {
            key: "FLEETY_TOKEN",
            scope: Server,
            default: "",
            description: "Bootstrap admin token for first pairing.",
            secret: true,
        },
        Setting {
            key: "FLEETY_MODEL_BASE_URL",
            scope: Server,
            default: "",
            description: "OpenAI-compatible model base URL.",
            secret: false,
        },
        Setting {
            key: "FLEETY_MODEL",
            scope: Server,
            default: "",
            description: "Main model name.",
            secret: false,
        },
        Setting {
            key: "FLEETY_MODEL_KEY",
            scope: Server,
            default: "",
            description: "Main model API key.",
            secret: true,
        },
        Setting {
            key: "FLEETY_CHEAP_MODEL",
            scope: Server,
            default: "",
            description: "Economy/cheap model name (subagents, housekeeping).",
            secret: false,
        },
        Setting {
            key: "FLEETY_TZ",
            scope: Shared,
            default: "UTC",
            description: "Fallback timezone for display (IANA).",
            secret: false,
        },
        Setting {
            key: "FLEETY_FS_SCOPE",
            scope: Shared,
            default: "full",
            description: "full or workspace (path confinement).",
            secret: false,
        },
        Setting {
            key: "FLEETY_AUTO_INSTALL_DEPS",
            scope: Shared,
            default: "1",
            description: "Auto-install missing dependencies at boot (1/0).",
            secret: false,
        },
        Setting {
            key: "FLEETY_AGENT_URL",
            scope: Daemon,
            default: "(mDNS → ws://127.0.0.1:8787)",
            description: "Server WebSocket URL the daemon/CLI connects to.",
            secret: false,
        },
        Setting {
            key: "FLEETY_DEVICE_ID",
            scope: Daemon,
            default: "(hostname)",
            description: "This device's id.",
            secret: false,
        },
    ]
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
    for setting in registry() {
        let already = std::env::var(setting.key)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if already {
            continue;
        }
        if let Some(v) = map.get(&(setting.scope, setting.key.to_string())) {
            std::env::set_var(setting.key, v);
        }
    }
}

/// Mask a value for display when its setting is secret.
pub fn display_value(setting: &Setting, value: &str) -> String {
    if setting.secret && !value.is_empty() {
        "********".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_key_is_rejected() {
        assert!(find("FLEETY_NOPE").is_none());
        assert!(find("FLEETY_ADDR").is_some());
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
        map.insert((Scope::Cli, "FLEETY_AGENT_URL".into()), "ws://x:1".into());
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
                .get(&(Scope::Cli, "FLEETY_AGENT_URL".into()))
                .map(String::as_str),
            Some("ws://x:1")
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
}
