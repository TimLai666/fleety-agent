//! `providers.toml` — the named model-provider pool.
//!
//! A structured, separate file (`~/.fleety/providers.toml`, overridable with
//! `FLEETY_PROVIDERS`) that defines any number of named providers, optional
//! groups over them, and a role→name map. This module owns the data model plus
//! a pure [`parse`] and a fail-soft [`load_from`]/[`load`]: a missing or broken
//! file is treated as absent (the caller falls back to the env-built tiers) and
//! never crashes. `fleety-server` turns this data into runtime providers and
//! group pools; [`write_providers`] / [`validate`] back the `config
//! provider|group|role` subcommands and the interactive editor.
//!
//! Keys live here (not in `config.toml`) so secrets stay isolated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_core::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// How a group dispatches a call across its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Advance the starting member each call to spread load across members.
    RoundRobin,
    /// Always start at the first member; only advance on failure (primary + backups).
    Failover,
}

/// One named provider: an endpoint/account plus its model and optional traits.
/// Mirrors the env-built provider fields (`{prefix}_BASE_URL` / model / key /
/// stream / modalities / effort) so the runtime builds it the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSpec {
    pub name: String,
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    /// Comma-separated input modalities (e.g. `"text,image"`); `None` derives
    /// from the model-family heuristic, exactly like the env path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<String>,
    /// Default reasoning effort (`low`/`medium`/`high`); `None` sends none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// A named group over member providers, with a dispatch strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupSpec {
    pub name: String,
    pub members: Vec<String>,
    pub strategy: Strategy,
}

/// The whole `providers.toml`: providers, groups, and a role→name map. TOML uses
/// the singular table-array keys `[[provider]]` / `[[group]]` and a `[roles]`
/// table; serde renames bridge those to the plural fields here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default, rename = "provider", skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderSpec>,
    #[serde(default, rename = "group", skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ProvidersConfig {
    /// Find a provider by name.
    pub fn provider(&self, name: &str) -> Option<&ProviderSpec> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Find a group by name.
    pub fn group(&self, name: &str) -> Option<&GroupSpec> {
        self.groups.iter().find(|g| g.name == name)
    }
}

/// Parse `providers.toml` text. Pure; a TOML/shape error is returned, not logged
/// or swallowed (callers decide whether to fail soft).
pub fn parse(text: &str) -> Result<ProvidersConfig> {
    toml::from_str(text).map_err(|e| CoreError::Message(format!("invalid providers.toml: {e}")))
}

/// The `providers.toml` path (`FLEETY_PROVIDERS` override, else
/// `~/.fleety/providers.toml`).
pub fn providers_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_PROVIDERS") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".fleety").join("providers.toml")
}

/// Load and parse a `providers.toml` at `path`, failing soft: a missing file or
/// any read/parse error yields `None` (a parse error is logged as a warning), so
/// a broken file never blocks startup — the caller falls back to env tiers.
pub fn load_from(path: &Path) -> Option<ProvidersConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    match parse(&text) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e,
                "ignoring providers.toml (falling back to environment)");
            None
        }
    }
}

/// Load the `providers.toml` from [`providers_path`], failing soft (see
/// [`load_from`]).
pub fn load() -> Option<ProvidersConfig> {
    load_from(&providers_path())
}

/// Load for *editing*: a missing file yields an empty config, but a present
/// file that fails to parse is an error (so editors never silently clobber a
/// broken file). Distinct from [`load_from`], which fails soft for the runtime.
pub fn load_or_default(path: &Path) -> Result<ProvidersConfig> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(_) => Ok(ProvidersConfig::default()),
    }
}

/// Validate cross-references in a config. Pure. Rejects a duplicate provider
/// name, a group member that isn't a defined provider, and a role whose target
/// is neither a defined provider nor a defined group. (A group's strategy is an
/// enum, so an invalid strategy can't be represented — it's rejected when a
/// command flag is parsed.) Each error names the offending item.
pub fn validate(cfg: &ProvidersConfig) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for p in &cfg.providers {
        if !seen.insert(p.name.as_str()) {
            return Err(CoreError::Message(format!(
                "duplicate provider name '{}'",
                p.name
            )));
        }
    }
    for g in &cfg.groups {
        for m in &g.members {
            if cfg.provider(m).is_none() {
                return Err(CoreError::Message(format!(
                    "group '{}' references undefined provider '{m}'",
                    g.name
                )));
            }
        }
    }
    for (role, target) in &cfg.roles {
        if cfg.provider(target).is_none() && cfg.group(target).is_none() {
            return Err(CoreError::Message(format!(
                "role '{role}' targets undefined provider/group '{target}'"
            )));
        }
    }
    Ok(())
}

/// Serialize `cfg` to TOML and write it to `path` atomically: write a temp file
/// in the same directory, then rename it over `path` (so a crash or concurrent
/// write never leaves a half-written file). The config is validated first; an
/// invalid config is not written.
pub fn write_providers(path: &Path, cfg: &ProvidersConfig) -> Result<()> {
    validate(cfg)?;
    let text = toml::to_string_pretty(cfg)
        .map_err(|e| CoreError::Message(format!("serialize providers.toml: {e}")))?;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(d) = dir {
        std::fs::create_dir_all(d)
            .map_err(|e| CoreError::Message(format!("cannot create providers dir: {e}")))?;
    }
    let tmp_name = format!(".providers-{}.tmp", uuid::Uuid::new_v4());
    let tmp = match dir {
        Some(d) => d.join(tmp_name),
        None => PathBuf::from(tmp_name),
    };
    std::fs::write(&tmp, text)
        .map_err(|e| CoreError::Message(format!("write providers.toml: {e}")))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::Message(format!("replace providers.toml: {e}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_providers_group_and_roles() {
        let text = r#"
            [[provider]]
            name = "codex-1"
            base_url = "https://api.openai.com/v1"
            model = "gpt-5"
            key = "sk-a"

            [[provider]]
            name = "codex-2"
            base_url = "https://api.openai.com/v1"
            model = "gpt-5"
            key = "sk-b"
            stream = true
            modalities = "text,image"
            effort = "medium"

            [[group]]
            name = "codex"
            members = ["codex-1", "codex-2"]
            strategy = "round_robin"

            [roles]
            main = "codex"
            cheap = "codex-1"
        "#;
        let cfg = parse(text).expect("parses");
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.groups.len(), 1);
        assert_eq!(cfg.groups[0].strategy, Strategy::RoundRobin);
        assert_eq!(cfg.groups[0].members, vec!["codex-1", "codex-2"]);
        assert_eq!(cfg.roles.get("main").map(String::as_str), Some("codex"));
        assert!(cfg.provider("codex-2").unwrap().stream);
        assert_eq!(
            cfg.provider("codex-2").unwrap().modalities.as_deref(),
            Some("text,image")
        );
    }

    #[test]
    fn parse_rejects_bad_toml() {
        assert!(parse("this is = = not toml").is_err());
        // Unknown strategy is a shape error → Err (fails soft at the load layer).
        let bad = "[[group]]\nname=\"g\"\nmembers=[]\nstrategy=\"random\"\n";
        assert!(parse(bad).is_err());
    }

    #[test]
    fn empty_sections_default_to_empty() {
        let cfg = parse("").expect("empty parses");
        assert!(cfg.providers.is_empty());
        assert!(cfg.groups.is_empty());
        assert!(cfg.roles.is_empty());
    }

    #[test]
    fn load_from_missing_file_is_none() {
        let p = std::env::temp_dir().join(format!("fleety-no-such-{}.toml", uuid::Uuid::new_v4()));
        assert!(load_from(&p).is_none());
    }

    #[test]
    fn load_from_broken_file_is_none() {
        let p = std::env::temp_dir().join(format!("fleety-bad-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&p, "= = broken").expect("write");
        assert!(load_from(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    fn sample() -> ProvidersConfig {
        let mut roles = BTreeMap::new();
        roles.insert("main".to_string(), "codex".to_string());
        roles.insert("cheap".to_string(), "codex-1".to_string());
        ProvidersConfig {
            providers: vec![
                ProviderSpec {
                    name: "codex-1".to_string(),
                    base_url: "https://api.openai.com/v1".to_string(),
                    model: "gpt-5".to_string(),
                    key: Some("sk-a".to_string()),
                    stream: true,
                    modalities: Some("text,image".to_string()),
                    effort: Some("medium".to_string()),
                },
                ProviderSpec {
                    name: "codex-2".to_string(),
                    base_url: "https://api.openai.com/v1".to_string(),
                    model: "gpt-5".to_string(),
                    key: None,
                    stream: false,
                    modalities: None,
                    effort: None,
                },
            ],
            groups: vec![GroupSpec {
                name: "codex".to_string(),
                members: vec!["codex-1".to_string(), "codex-2".to_string()],
                strategy: Strategy::RoundRobin,
            }],
            roles,
        }
    }

    #[test]
    fn write_then_parse_is_stable() {
        let cfg = sample();
        let p = std::env::temp_dir().join(format!("fleety-rt-{}.toml", uuid::Uuid::new_v4()));
        write_providers(&p, &cfg).expect("write");
        let back = load_from(&p).expect("re-read");
        assert_eq!(back, cfg);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn write_rejects_invalid_config() {
        // A role targeting an undefined name fails validation → no file written.
        let mut cfg = ProvidersConfig::default();
        cfg.roles.insert("main".to_string(), "ghost".to_string());
        let p = std::env::temp_dir().join(format!("fleety-inv-{}.toml", uuid::Uuid::new_v4()));
        assert!(write_providers(&p, &cfg).is_err());
        assert!(!p.exists(), "no file should be written on invalid config");
    }

    #[test]
    fn validate_rejects_inconsistencies_and_passes_clean() {
        // Duplicate provider name.
        let mut dup = sample();
        dup.providers[1].name = "codex-1".to_string();
        assert!(validate(&dup).unwrap_err().to_string().contains("codex-1"));

        // Group member that isn't a provider.
        let mut bad_member = sample();
        bad_member.groups[0].members.push("ghost".to_string());
        assert!(validate(&bad_member)
            .unwrap_err()
            .to_string()
            .contains("ghost"));

        // Role target that isn't a provider or group.
        let mut bad_role = sample();
        bad_role.roles.insert("x".to_string(), "nope".to_string());
        assert!(validate(&bad_role)
            .unwrap_err()
            .to_string()
            .contains("nope"));

        // A consistent config passes.
        assert!(validate(&sample()).is_ok());
    }
}
