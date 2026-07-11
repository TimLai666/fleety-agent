//! `providers.toml` — the two-tier provider / model-role configuration.
//!
//! A structured, separate file (`~/.fleety/providers.toml`, overridable with
//! `FLEETY_PROVIDERS`) with two tiers (design §3.3):
//!
//! - **Providers** are endpoints/accounts, tagged by `type` (an extensible
//!   registry — see [`provider_types`]). `type = "api"` carries a `base_url` and
//!   optional `key`; `type = "oauth:codex"` sources a per-provider OAuth token
//!   from `fleety auth login` and carries no `base_url`/`key`.
//! - **Model roles** are fixed `main` and `cheap`; each is a pool with a
//!   [`Strategy`] and a list of [`Member`]s, where a member is the full build
//!   unit — it names a provider plus the `model` and the call-time traits
//!   (`stream`/`modalities`/`effort`) that follow the model, not the account. One
//!   provider can therefore serve different models to different roles.
//!
//! [`parse`] and the fail-soft [`load_from`]/[`load`] never crash (a missing or
//! broken file is treated as absent, the caller falls back to the env tiers).
//! [`write_providers`] / [`validate`] back the `config provider|model`
//! subcommands and the interactive editor — the write path is **not** fail-soft
//! (referential integrity is enforced before a file is written).
//! [`migrate_providers`] one-time upgrades a legacy provider-binds-model file.
//!
//! Keys live here (not in `config.toml`) so secrets stay isolated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_core::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// How a model role's member pool dispatches a call across its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Exactly one member (a plain single-provider role).
    Single,
    /// Advance the starting member each call to spread load across members.
    RoundRobin,
    /// Always start at the first member; only advance on failure (primary + backups).
    Failover,
}

/// A known provider `type`: its authentication/endpoint rules. The list in
/// [`provider_types`] is the extensible registry — adding a new auth type is one
/// entry here, never a new core `match` arm.
pub struct ProviderType {
    pub name: &'static str,
    /// The `api` shape needs a `base_url`; oauth types must not carry one.
    pub requires_base_url: bool,
    /// The `api` shape may carry a static `key`; oauth types must not.
    pub allows_key: bool,
    /// Whether the bearer comes from an OAuth login rather than a static key.
    pub is_oauth: bool,
}

/// The registry of known provider types. Extend by adding an entry.
pub fn provider_types() -> &'static [ProviderType] {
    &[
        ProviderType {
            name: "api",
            requires_base_url: true,
            allows_key: true,
            is_oauth: false,
        },
        ProviderType {
            name: "oauth:codex",
            requires_base_url: false,
            allows_key: false,
            is_oauth: true,
        },
    ]
}

/// Look up a provider type in the registry.
pub fn provider_type(kind: &str) -> Option<&'static ProviderType> {
    provider_types().iter().find(|t| t.name == kind)
}

fn known_types_list() -> String {
    provider_types()
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One provider: an endpoint/account tagged by `type`. The model is NOT a
/// provider field (it lives on the [`Member`] that names this provider).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl Provider {
    /// Whether this provider's bearer comes from an OAuth login (per its type).
    pub fn is_oauth(&self) -> bool {
        provider_type(&self.kind)
            .map(|t| t.is_oauth)
            .unwrap_or(false)
    }
}

/// One member of a model-role pool: a provider plus the model and its call-time
/// traits. This is the full build unit — `stream`/`modalities`/`effort` follow
/// the model/call, so a mixed pool routes each member with its own traits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// A model role (`main`/`cheap`): a dispatch strategy over member build units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPool {
    pub strategy: Strategy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
}

/// The whole `providers.toml`: named providers and the `main`/`cheap` model
/// roles. TOML uses `[providers.<name>]` and `[models.<role>]` tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, Provider>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelPool>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ProvidersConfig {
    /// The provider named `name`, if defined.
    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.get(name)
    }

    /// The model role named `role`, if defined.
    pub fn model(&self, role: &str) -> Option<&ModelPool> {
        self.models.get(role)
    }

    /// The role that references provider `name` through a member, if any (used
    /// to refuse removing a still-referenced provider).
    pub fn role_referencing(&self, provider: &str) -> Option<&str> {
        self.models
            .iter()
            .find(|(_, pool)| pool.members.iter().any(|m| m.provider == provider))
            .map(|(role, _)| role.as_str())
    }
}

/// Parse `providers.toml` text into the two-tier config. Pure; a TOML/shape
/// error, or a provider naming an unknown `type`, is returned (not swallowed) so
/// the editor path can fail loudly.
pub fn parse(text: &str) -> Result<ProvidersConfig> {
    let cfg: ProvidersConfig = toml::from_str(text)
        .map_err(|e| CoreError::Message(format!("invalid providers.toml: {e}")))?;
    for (name, p) in &cfg.providers {
        if provider_type(&p.kind).is_none() {
            return Err(CoreError::Message(format!(
                "provider '{name}' has unknown type '{}' (known types: {})",
                p.kind,
                known_types_list()
            )));
        }
    }
    Ok(cfg)
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
/// any read/parse error yields `None` (a parse error is logged loudly), so a
/// broken file never blocks a runtime read — the caller falls back to env tiers.
/// The write path ([`write_providers`]) is deliberately NOT fail-soft.
pub fn load_from(path: &Path) -> Option<ProvidersConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    match parse(&text) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e,
                "providers.toml is broken and was IGNORED at read time — fix it (e.g. \
                 `fleety config provider list`)");
            None
        }
    }
}

/// Load the `providers.toml` from [`providers_path`], failing soft (see
/// [`load_from`]).
pub fn load() -> Option<ProvidersConfig> {
    load_from(&providers_path())
}

/// Load for *editing*: a missing file yields an empty config, but a present file
/// that fails to parse is an error (so editors never silently clobber a broken
/// file). Distinct from [`load_from`], which fails soft for the runtime.
pub fn load_or_default(path: &Path) -> Result<ProvidersConfig> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(_) => Ok(ProvidersConfig::default()),
    }
}

/// Validate the two-tier config. Pure. Enforces provider `type` field rules (api
/// needs a `base_url` and no oauth-only shape; oauth types carry no
/// `base_url`/`key`), and role referential integrity (every member names a
/// defined provider; a present role has ≥1 member; `single` has exactly one).
/// Each error names the offending item.
pub fn validate(cfg: &ProvidersConfig) -> Result<()> {
    for (name, p) in &cfg.providers {
        let Some(t) = provider_type(&p.kind) else {
            return Err(CoreError::Message(format!(
                "provider '{name}' has unknown type '{}' (known types: {})",
                p.kind,
                known_types_list()
            )));
        };
        let has_base = p
            .base_url
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if t.requires_base_url && !has_base {
            return Err(CoreError::Message(format!(
                "provider '{name}' (type {}) requires a base_url",
                p.kind
            )));
        }
        if !t.requires_base_url && p.base_url.is_some() {
            return Err(CoreError::Message(format!(
                "provider '{name}' (type {}) must not set base_url",
                p.kind
            )));
        }
        if !t.allows_key && p.key.is_some() {
            return Err(CoreError::Message(format!(
                "provider '{name}' (type {}) must not set key",
                p.kind
            )));
        }
    }
    for (role, pool) in &cfg.models {
        if pool.members.is_empty() {
            return Err(CoreError::Message(format!(
                "model role '{role}' has no members"
            )));
        }
        if pool.strategy == Strategy::Single && pool.members.len() != 1 {
            return Err(CoreError::Message(format!(
                "model role '{role}' uses strategy=single but has {} members (need exactly one)",
                pool.members.len()
            )));
        }
        for m in &pool.members {
            if !cfg.providers.contains_key(&m.provider) {
                return Err(CoreError::Message(format!(
                    "model role '{role}' member references undefined provider '{}'",
                    m.provider
                )));
            }
        }
    }
    Ok(())
}

/// Serialize `cfg` to TOML and write it to `path` atomically (temp file + rename
/// in the same directory). The config is validated first; an invalid config is
/// not written — the write path is not fail-soft.
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

// ---- one-time migration: legacy provider-binds-model → two-tier ----

/// Migrate legacy `providers.toml` text (the `[[provider]]` binds-a-model form
/// with `[[group]]`/`[roles]`) into the two-tier shape. Returns `Ok(None)` when
/// the text is already two-tier or has nothing to migrate (idempotent).
/// Otherwise returns the new config plus any human-readable warnings for parts
/// that could not be carried over (never silently dropped).
///
/// Providers that differ only by `model` (same `base_url`+`key`+`auth`) merge
/// into one provider; each old model becomes a [`Member`] carrying that model
/// and its `stream`/`modalities`/`effort`; old roles map to `models.<role>`.
pub fn migrate_providers(text: &str) -> Result<Option<(ProvidersConfig, Vec<String>)>> {
    let raw: toml::Value = text
        .parse()
        .map_err(|e| CoreError::Message(format!("invalid providers.toml: {e}")))?;
    let has_new =
        raw.get("models").is_some() || raw.get("providers").and_then(|v| v.as_table()).is_some();
    let has_old = raw.get("provider").and_then(|v| v.as_array()).is_some()
        || raw.get("group").is_some()
        || raw.get("roles").is_some();
    if has_new || !has_old {
        return Ok(None);
    }

    #[derive(Deserialize)]
    struct OldProvider {
        name: String,
        #[serde(default)]
        base_url: String,
        model: String,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        stream: bool,
        #[serde(default)]
        modalities: Option<String>,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        auth: Option<String>,
    }
    #[derive(Deserialize)]
    struct OldGroup {
        name: String,
        members: Vec<String>,
        strategy: Strategy,
    }
    #[derive(Deserialize, Default)]
    struct OldConfig {
        #[serde(default, rename = "provider")]
        providers: Vec<OldProvider>,
        #[serde(default, rename = "group")]
        groups: Vec<OldGroup>,
        #[serde(default)]
        roles: BTreeMap<String, String>,
    }

    let old: OldConfig = toml::from_str(text)
        .map_err(|e| CoreError::Message(format!("invalid legacy providers.toml: {e}")))?;
    let mut warnings = Vec::new();

    // Dedup old providers by endpoint identity (base_url, key, auth) → one new
    // provider; remember the old→new name mapping so members can point at it.
    type EndpointId = (String, Option<String>, Option<String>);
    let mut endpoints: Vec<(EndpointId, String)> = Vec::new();
    let mut old_to_new: BTreeMap<String, String> = BTreeMap::new();
    let mut providers: BTreeMap<String, Provider> = BTreeMap::new();
    for op in &old.providers {
        let ident = (op.base_url.clone(), op.key.clone(), op.auth.clone());
        let newname = if let Some((_, n)) = endpoints.iter().find(|(e, _)| *e == ident) {
            n.clone()
        } else {
            let kind = if op.auth.as_deref() == Some("oauth:codex") {
                "oauth:codex".to_string()
            } else {
                "api".to_string()
            };
            let provider = if kind == "api" {
                Provider {
                    kind,
                    base_url: Some(op.base_url.clone()),
                    key: op.key.clone(),
                }
            } else {
                Provider {
                    kind,
                    base_url: None,
                    key: None,
                }
            };
            providers.insert(op.name.clone(), provider);
            endpoints.push((ident, op.name.clone()));
            op.name.clone()
        };
        old_to_new.insert(op.name.clone(), newname);
    }

    let member_for = |op: &OldProvider| Member {
        provider: old_to_new
            .get(&op.name)
            .cloned()
            .unwrap_or_else(|| op.name.clone()),
        model: op.model.clone(),
        stream: op.stream,
        modalities: op.modalities.clone(),
        effort: op.effort.clone(),
    };
    let old_by_name: BTreeMap<&str, &OldProvider> =
        old.providers.iter().map(|p| (p.name.as_str(), p)).collect();

    let mut models: BTreeMap<String, ModelPool> = BTreeMap::new();
    for (role, target) in &old.roles {
        if let Some(op) = old_by_name.get(target.as_str()) {
            models.insert(
                role.clone(),
                ModelPool {
                    strategy: Strategy::Single,
                    members: vec![member_for(op)],
                },
            );
        } else if let Some(g) = old.groups.iter().find(|g| g.name == *target) {
            let members: Vec<Member> = g
                .members
                .iter()
                .filter_map(|mn| old_by_name.get(mn.as_str()).map(|op| member_for(op)))
                .collect();
            if members.is_empty() {
                warnings.push(format!(
                    "role '{role}' → group '{target}' had no resolvable members; skipped"
                ));
                continue;
            }
            models.insert(
                role.clone(),
                ModelPool {
                    strategy: g.strategy,
                    members,
                },
            );
        } else {
            warnings.push(format!(
                "role '{role}' → '{target}' matched no provider or group; skipped"
            ));
        }
    }

    Ok(Some((ProvidersConfig { providers, models }, warnings)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- task 1.1: two-tier data model + parse ----

    #[test]
    fn parse_two_tier_round_trips() {
        let text = r#"
            [providers.openai1]
            type = "api"
            base_url = "https://api.openai.com/v1"
            key = "sk-a"

            [providers.codex1]
            type = "oauth:codex"

            [models.main]
            strategy = "failover"
            members = [
              { provider = "openai1", model = "gpt-4o", stream = true, modalities = "text,image", effort = "medium" },
              { provider = "codex1", model = "gpt-5" },
            ]

            [models.cheap]
            strategy = "single"
            members = [ { provider = "openai1", model = "gpt-4o-mini" } ]
        "#;
        let cfg = parse(text).expect("parses");
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.provider("openai1").unwrap().kind, "api");
        assert_eq!(
            cfg.provider("openai1").unwrap().base_url.as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert!(cfg.provider("codex1").unwrap().is_oauth());
        assert!(cfg.provider("codex1").unwrap().base_url.is_none());
        let main = cfg.model("main").unwrap();
        assert_eq!(main.strategy, Strategy::Failover);
        assert_eq!(main.members.len(), 2);
        assert_eq!(main.members[0].model, "gpt-4o");
        assert!(main.members[0].stream);
        assert_eq!(main.members[0].modalities.as_deref(), Some("text,image"));
        // Round-trip through write shape.
        let back = parse(&toml::to_string_pretty(&cfg).unwrap()).expect("re-parse");
        assert_eq!(back, cfg);
    }

    #[test]
    fn parse_unknown_type_lists_known() {
        let text = "[providers.x]\ntype = \"oauth:mystery\"\n";
        let err = parse(text).unwrap_err().to_string();
        assert!(err.contains("unknown type 'oauth:mystery'"), "got: {err}");
        assert!(
            err.contains("api") && err.contains("oauth:codex"),
            "lists known: {err}"
        );
    }

    // ---- task 1.2: validate (referential integrity, field rules) ----

    fn api(base: &str) -> Provider {
        Provider {
            kind: "api".to_string(),
            base_url: Some(base.to_string()),
            key: None,
        }
    }

    fn member(provider: &str, model: &str) -> Member {
        Member {
            provider: provider.to_string(),
            model: model.to_string(),
            stream: false,
            modalities: None,
            effort: None,
        }
    }

    fn cfg_with(providers: &[(&str, Provider)], models: &[(&str, ModelPool)]) -> ProvidersConfig {
        ProvidersConfig {
            providers: providers
                .iter()
                .map(|(n, p)| (n.to_string(), p.clone()))
                .collect(),
            models: models
                .iter()
                .map(|(n, m)| (n.to_string(), m.clone()))
                .collect(),
        }
    }

    #[test]
    fn validate_rejects_undefined_member_provider() {
        let cfg = cfg_with(
            &[("openai1", api("https://x/v1"))],
            &[(
                "main",
                ModelPool {
                    strategy: Strategy::Single,
                    members: vec![member("ghost", "gpt-4o")],
                },
            )],
        );
        let err = validate(&cfg).unwrap_err().to_string();
        assert!(err.contains("undefined provider 'ghost'"), "got: {err}");
    }

    #[test]
    fn validate_single_requires_exactly_one_member() {
        let cfg = cfg_with(
            &[("openai1", api("https://x/v1"))],
            &[(
                "main",
                ModelPool {
                    strategy: Strategy::Single,
                    members: vec![member("openai1", "a"), member("openai1", "b")],
                },
            )],
        );
        assert!(validate(&cfg)
            .unwrap_err()
            .to_string()
            .contains("exactly one"));
    }

    #[test]
    fn validate_rejects_api_without_base_url_and_oauth_with_fields() {
        // api missing base_url.
        let mut bad_api = api("");
        bad_api.base_url = None;
        let cfg = cfg_with(&[("p", bad_api)], &[]);
        assert!(validate(&cfg)
            .unwrap_err()
            .to_string()
            .contains("requires a base_url"));
        // oauth carrying base_url/key.
        let bad_oauth = Provider {
            kind: "oauth:codex".to_string(),
            base_url: Some("https://x".to_string()),
            key: None,
        };
        let cfg = cfg_with(&[("c", bad_oauth)], &[]);
        assert!(validate(&cfg)
            .unwrap_err()
            .to_string()
            .contains("must not set base_url"));
    }

    #[test]
    fn validate_passes_clean_and_reports_reference() {
        let cfg = cfg_with(
            &[("openai1", api("https://x/v1"))],
            &[(
                "main",
                ModelPool {
                    strategy: Strategy::Single,
                    members: vec![member("openai1", "gpt-4o")],
                },
            )],
        );
        assert!(validate(&cfg).is_ok());
        assert_eq!(cfg.role_referencing("openai1"), Some("main"));
        assert_eq!(cfg.role_referencing("nope"), None);
    }

    #[test]
    fn write_rejects_invalid_and_round_trips_valid() {
        let good = cfg_with(
            &[("openai1", api("https://x/v1"))],
            &[(
                "main",
                ModelPool {
                    strategy: Strategy::Failover,
                    members: vec![
                        member("openai1", "gpt-4o"),
                        member("openai1", "gpt-4o-mini"),
                    ],
                },
            )],
        );
        let p = std::env::temp_dir().join(format!("fleety-pv-{}.toml", uuid::Uuid::new_v4()));
        write_providers(&p, &good).expect("write valid");
        assert_eq!(load_from(&p).expect("re-read"), good);
        // Invalid (undefined provider) is refused and leaves the good file intact.
        let bad = cfg_with(&[], &[("main", good.models["main"].clone())]);
        assert!(write_providers(&p, &bad).is_err());
        assert_eq!(load_from(&p).expect("still good"), good);
        let _ = std::fs::remove_file(&p);
    }

    // ---- task 1.3: migration ----

    #[test]
    fn migrate_dedups_providers_and_sinks_traits_to_members() {
        // Two legacy providers, same endpoint, different model, pooled by a group.
        let legacy = r#"
            [[provider]]
            name = "openai-a"
            base_url = "https://api.openai.com/v1"
            key = "sk-x"
            model = "gpt-4o"
            stream = true
            modalities = "text,image"

            [[provider]]
            name = "openai-b"
            base_url = "https://api.openai.com/v1"
            key = "sk-x"
            model = "gpt-4o-mini"

            [[group]]
            name = "openai"
            members = ["openai-a", "openai-b"]
            strategy = "round_robin"

            [roles]
            main = "openai"
            cheap = "openai-b"
        "#;
        let (cfg, warnings) = migrate_providers(legacy).expect("migrate").expect("is old");
        // Same endpoint+key → deduped to ONE provider.
        assert_eq!(
            cfg.providers.len(),
            1,
            "deduped: {:?}",
            cfg.providers.keys().collect::<Vec<_>>()
        );
        let pname = cfg.providers.keys().next().unwrap().clone();
        // main = round_robin pool of both models, both referencing the one provider.
        let main = cfg.model("main").expect("main role");
        assert_eq!(main.strategy, Strategy::RoundRobin);
        assert_eq!(main.members.len(), 2);
        assert!(main.members.iter().all(|m| m.provider == pname));
        let models: Vec<&str> = main.members.iter().map(|m| m.model.as_str()).collect();
        assert!(models.contains(&"gpt-4o") && models.contains(&"gpt-4o-mini"));
        // Traits sank onto the matching member.
        let a = main.members.iter().find(|m| m.model == "gpt-4o").unwrap();
        assert!(a.stream);
        assert_eq!(a.modalities.as_deref(), Some("text,image"));
        // cheap = single, one member (gpt-4o-mini).
        let cheap = cfg.model("cheap").expect("cheap role");
        assert_eq!(cheap.strategy, Strategy::Single);
        assert_eq!(cheap.members[0].model, "gpt-4o-mini");
        assert!(warnings.is_empty(), "clean migration: {warnings:?}");
        // The migrated config validates.
        assert!(validate(&cfg).is_ok());
    }

    #[test]
    fn migrate_is_idempotent_on_new_format() {
        let new = r#"
            [providers.openai1]
            type = "api"
            base_url = "https://x/v1"

            [models.main]
            strategy = "single"
            members = [ { provider = "openai1", model = "gpt-4o" } ]
        "#;
        assert!(migrate_providers(new).expect("no error").is_none());
        // Empty text has nothing to migrate.
        assert!(migrate_providers("").expect("no error").is_none());
    }

    #[test]
    fn migrate_warns_on_unresolvable_role_target() {
        let legacy = r#"
            [[provider]]
            name = "p1"
            base_url = "https://x/v1"
            model = "gpt-4o"

            [roles]
            main = "p1"
            cheap = "ghost-group"
        "#;
        let (cfg, warnings) = migrate_providers(legacy).expect("migrate").expect("is old");
        assert!(cfg.model("main").is_some());
        assert!(cfg.model("cheap").is_none(), "unresolvable role skipped");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("cheap") && warnings[0].contains("ghost-group"));
    }
}
