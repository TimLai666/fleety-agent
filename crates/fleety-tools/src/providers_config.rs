//! `providers.toml` — the named model-provider pool.
//!
//! A structured, separate file (`~/.fleety/providers.toml`, overridable with
//! `FLEETY_PROVIDERS`) that defines any number of named providers, optional
//! groups over them, and a role→name map. This module owns the data model plus
//! a pure [`parse`] and a fail-soft [`load_from`]/[`load`]: a missing or broken
//! file is treated as absent (the caller falls back to the env-built tiers) and
//! never crashes. `fleety-server` turns this data into runtime providers and
//! group pools; `fleety-tools::config` (a later change) writes it back.
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
}
