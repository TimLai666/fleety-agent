//! Model-provider construction and the named provider registry.
//!
//! [`build`] turns `{prefix}_BASE_URL` + `{prefix}_MODEL` (+ optional
//! `{prefix}_KEY` / `{prefix}_STREAM` / `_MODALITIES` / `_EFFORT`) into a
//! provider via [`build_provider`], which the `providers.toml` path reuses.
//! [`ProviderTiers`] is a named registry: without a `providers.toml` it holds
//! just `"main"` and `"cheap"` from the environment (cheap aliases main when
//! unset, so selecting `cheap` never errors); with one it holds every named
//! provider and group (groups as [`crate::pool::PoolProvider`]s) plus a
//! role→name map. Subagents (and the main turn) select by tier/role name; an
//! unknown selector falls back to `main`.

use std::sync::Arc;

use agent_core::{CodexResponses, Gemini, ModelProvider, OpenAiCompat};

use crate::echo::EchoProvider;

/// Build a provider from environment variables under `prefix`. The model name
/// is the bare `{prefix}` var; the base URL is `{prefix}_BASE_URL`, with
/// optional `{prefix}_KEY` and `{prefix}_STREAM`. So `"FLEETY_MODEL"` reads
/// `FLEETY_MODEL` together with `FLEETY_MODEL_BASE_URL`, and
/// `"FLEETY_CHEAP_MODEL"` reads its `_CHEAP_` twins. Returns `None` when base
/// URL or model is unset/empty, so the caller decides the fallback.
/// Construction does not open a connection, so this is safe without a reachable
/// endpoint.
pub fn build(prefix: &str) -> Option<Arc<dyn ModelProvider>> {
    // The model name lives in the bare prefix var (`FLEETY_MODEL` /
    // `FLEETY_CHEAP_MODEL`); base URL / key / stream are suffixed.
    let base_url = std::env::var(format!("{prefix}_BASE_URL")).ok()?;
    let model = std::env::var(prefix).ok()?;
    if base_url.is_empty() || model.is_empty() {
        return None;
    }
    let key = std::env::var(format!("{prefix}_KEY"))
        .ok()
        .filter(|k| !k.is_empty());
    let stream = std::env::var(format!("{prefix}_STREAM")).as_deref() == Ok("1");
    let modalities = std::env::var(format!("{prefix}_MODALITIES"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    let effort = std::env::var(format!("{prefix}_EFFORT")).ok();
    let auth = std::env::var(format!("{prefix}_AUTH")).ok();
    Some(build_provider(ProviderBuild {
        base_url,
        model,
        key,
        stream,
        modalities,
        effort,
        auth,
        provider_name: None, // env bootstrap has no named provider
        label: prefix.to_string(),
    }))
}

/// Whether an auth-mode string selects the Codex OAuth bearer. `None`/`"static"`
/// (or anything else) keeps the static-key path. Pure — unit-testable.
fn auth_is_oauth(auth: Option<&str>) -> bool {
    matches!(auth, Some(a) if a.eq_ignore_ascii_case("oauth:codex"))
}

/// The fields needed to build one provider (shared by the env path and the
/// `providers.toml` path). A named struct rather than eight positional
/// arguments so callers can't transpose the three consecutive `Option`s
/// (modalities / effort / auth) — a swap the compiler wouldn't catch.
pub struct ProviderBuild {
    pub base_url: String,
    pub model: String,
    pub key: Option<String>,
    pub stream: bool,
    /// Comma-separated input modalities (e.g. `"text,image"`); `None` derives
    /// from the model-family heuristic.
    pub modalities: Option<String>,
    /// Default reasoning effort (`low`/`medium`/`high`); `None` sends none.
    pub effort: Option<String>,
    /// Auth mode: `None`/`"static"` uses `key`; `"oauth:codex"` sources the
    /// bearer from the Codex OAuth token store.
    pub auth: Option<String>,
    /// The `providers.toml` provider name, used to source the **per-provider**
    /// Codex token store for `oauth:codex`. `None` for the env bootstrap path
    /// (which has no named provider) — such a build falls back to the legacy
    /// global path, which is cleared on boot, so it reports logged-out.
    pub provider_name: Option<String>,
    /// Used only for log lines (the env prefix or the provider name).
    pub label: String,
}

/// Build one provider from [`ProviderBuild`]. Picks the native Gemini vs
/// OpenAI-compatible backend from the model name, resolves modality
/// capabilities (explicit `modalities` wins, else the model-family heuristic,
/// else text-only with a warning), and attaches the family's effort scheme +
/// optional default effort.
pub fn build_provider(cfg: ProviderBuild) -> Arc<dyn ModelProvider> {
    let ProviderBuild {
        base_url,
        model,
        key,
        stream,
        modalities,
        effort,
        auth,
        provider_name,
        label,
    } = cfg;
    let modalities = modalities.as_deref();
    let effort = effort.as_deref();
    let auth = auth.as_deref();
    let label = label.as_str();
    // Modality capabilities: explicit `modalities` (e.g. "text,image") wins;
    // otherwise derive from the model-family heuristic. Capable providers route
    // attachments natively; others degrade unsupported ones to a text note.
    let caps = match modalities {
        Some(s) if !s.trim().is_empty() => agent_core::model::parse_modalities(s),
        _ if looks_multimodal(&model) => agent_core::model::ModelCapabilities::ALL,
        _ => {
            tracing::warn!(
                %model, %label,
                "model name doesn't match a known multimodal family; treating it as text-only — \
                 set modalities (e.g. text,image) to override"
            );
            agent_core::model::ModelCapabilities::TEXT_ONLY
        }
    };
    // Reasoning effort: the family's encoding scheme (from the model name) plus
    // an optional default. When the scheme is None or no effort is set, no effort
    // field is sent.
    let default_effort = effort.and_then(agent_core::model::Effort::parse);
    let scheme = effort_scheme_for(&model);
    // Codex OAuth mode talks to the ChatGPT backend over the Responses API — a
    // different provider entirely, ignoring base_url/key.
    if auth_is_oauth(auth) {
        return build_codex_responses(model, caps, default_effort, label, provider_name.as_deref());
    }
    if agent_core::gemini::looks_like_gemini_model(&model) {
        tracing::info!(%base_url, %model, stream, %label, "using native Gemini provider");
        Arc::new(
            Gemini::new(base_url, model, key)
                .with_streaming(stream)
                .with_capabilities(caps)
                .with_effort_config(scheme, default_effort),
        )
    } else {
        tracing::info!(%base_url, %model, stream, %label, "using OpenAI-compatible provider");
        Arc::new(
            OpenAiCompat::new(base_url, model, key)
                .with_streaming(stream)
                .with_capabilities(caps)
                .with_effort_config(scheme, default_effort),
        )
    }
}

/// The Codex token store an `oauth:codex` provider reads: its OWN per-provider
/// file (`codex-oauth/<name>.json`) when named in providers.toml, else the legacy
/// global path for the env bootstrap path (cleared on boot, so it reads as
/// logged-out until a per-provider login). Pure selection — unit-tested.
fn codex_token_path(provider_name: Option<&str>) -> std::path::PathBuf {
    match provider_name {
        Some(name) => fleety_tools::oauth::token_path_for(name),
        None => fleety_tools::oauth::default_token_path(),
    }
}

/// Build the Codex Responses provider for the `oauth:codex` auth mode: it calls
/// the ChatGPT backend over the Responses API with the account's OAuth token
/// (refreshed on demand). The configured `base_url`/`key` are ignored — Codex has
/// its own backend, resolved from `FLEETY_CODEX_BACKEND_URL`.
fn build_codex_responses(
    model: String,
    caps: agent_core::model::ModelCapabilities,
    default_effort: Option<agent_core::model::Effort>,
    label: &str,
    provider_name: Option<&str>,
) -> Arc<dyn ModelProvider> {
    let cfg = fleety_tools::oauth::oauth_config();
    let endpoint = format!("{}/responses", cfg.backend_base_url.trim_end_matches('/'));
    tracing::info!(%endpoint, %model, %label, provider = provider_name.unwrap_or("(env)"), "using Codex Responses provider (oauth:codex)");
    if provider_name.is_none() {
        // Env-bootstrapped oauth:codex (no providers.toml name) reads the legacy
        // global store, which is cleared every boot and can never be written
        // (credentials are per-provider now) — so it can never sign in. Warn once
        // at build time; a named providers.toml oauth:codex provider is required.
        tracing::warn!(
            %label,
            "oauth:codex configured via env has no provider name to sign in as; it cannot \
             authenticate — add a named oauth:codex provider in providers.toml and \
             `fleety auth login <provider>`"
        );
    }
    let auth_src = fleety_tools::oauth::OAuthCodexAuth::new(codex_token_path(provider_name), &cfg);
    Arc::new(
        CodexResponses::new(endpoint, model, Arc::new(auth_src))
            .with_capabilities(caps)
            .with_effort(default_effort),
    )
}

/// Build one runtime provider from a model-role member: its named provider's
/// endpoint/auth (from `providers.toml`) plus the member's model and call-time
/// traits (`stream`/`modalities`/`effort`). Returns `None` when the member names
/// a provider that isn't defined (the pool skips it with a warning).
fn build_member(
    cfg: &fleety_tools::providers_config::ProvidersConfig,
    m: &fleety_tools::providers_config::Member,
) -> Option<Arc<dyn ModelProvider>> {
    let provider = cfg.providers.get(&m.provider)?;
    let (base_url, key, auth) = if provider.is_oauth() {
        // oauth types carry no base_url/key; the bearer comes from the token store.
        (String::new(), None, Some(provider.kind.clone()))
    } else {
        (
            provider.base_url.clone().unwrap_or_default(),
            provider.key.clone(),
            None,
        )
    };
    Some(build_provider(ProviderBuild {
        base_url,
        model: m.model.clone(),
        key,
        stream: m.stream,
        modalities: m.modalities.clone(),
        effort: m.effort.clone(),
        auth,
        provider_name: Some(m.provider.clone()),
        label: format!("{}/{}", m.provider, m.model),
    }))
}

/// At server boot: migrate a legacy `providers.toml` (provider-binds-model form)
/// in place, then refuse to boot on a present-but-broken or referentially
/// incomplete two-tier config (design M5) — rather than silently degrading to the
/// echo stub. A missing file is fine (the env `FLEETY_MODEL_*` bootstrap seed
/// takes over). Called once from `main`; the caller exits on `Err`.
pub fn migrate_and_check() -> agent_core::Result<()> {
    use fleety_tools::providers_config as pc;
    let path = pc::providers_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(()); // no structured config → env bootstrap seed
    };
    if let Some((cfg, warnings)) = pc::migrate_providers(&text)? {
        for w in &warnings {
            tracing::warn!(migration = %w, "providers.toml migration carried this over with a note");
        }
        pc::write_providers(&path, &cfg)?; // validates before writing
        tracing::info!("migrated legacy providers.toml to the two-tier provider/model shape");
        return Ok(());
    }
    // Already two-tier: parse + validate; a broken/incomplete config is fatal.
    let cfg = pc::parse(&text)?;
    pc::validate(&cfg)?;
    Ok(())
}

/// Which reasoning-effort encoding (if any) a model family accepts. Conservative:
/// only models known to take an effort field get a scheme, so an effort value is
/// never sent to a model that would reject it.
fn effort_scheme_for(model: &str) -> agent_core::model::EffortScheme {
    use agent_core::model::EffortScheme;
    let m = model.to_ascii_lowercase();
    if m.contains("gemini-2.5") || m.contains("gemini-2-5") {
        EffortScheme::GeminiThinking
    } else if m.contains("o1") || m.contains("o3") || m.contains("o4") || m.contains("gpt-5") {
        EffortScheme::OpenAiReasoning
    } else {
        EffortScheme::None
    }
}

/// The main model provider (`FLEETY_MODEL_*`), falling back to the offline echo
/// stub when unset.
pub fn build_main() -> Arc<dyn ModelProvider> {
    build("FLEETY_MODEL").unwrap_or_else(|| {
        // Loud on purpose: without this, a first `docker compose up` looks
        // broken (the agent only parrots input) and the logs show no error.
        tracing::warn!(
            "FLEETY_MODEL is not set — running in ECHO mode: the agent will only echo input, \
             no real model is called. Set FLEETY_MODEL_BASE_URL + FLEETY_MODEL (env or \
             `fleety-server config set …`) to enable one."
        );
        Arc::new(EchoProvider)
    })
}

/// A named registry of model providers plus a role→name map. A subagent (or the
/// main turn) selects by tier/role name; the selection only changes which
/// provider runs, never the agent's policy, gate, or audit.
///
/// Without a `providers.toml` this holds exactly `"main"` and `"cheap"` built
/// from the environment (cheap aliases main when unset), so the zero-config
/// behavior is unchanged. With a `providers.toml` it holds every named provider
/// and group (groups as [`PoolProvider`]s), and `roles` maps `main`/`cheap`/any
/// tier name to a provider or group.
#[derive(Clone)]
pub struct ProviderTiers {
    providers: std::collections::HashMap<String, Arc<dyn ModelProvider>>,
    roles: std::collections::HashMap<String, String>,
    /// The resolved fallback used for unknown selectors.
    main: Arc<dyn ModelProvider>,
}

impl ProviderTiers {
    /// Build from `providers.toml` if present and non-empty; otherwise fall back
    /// to the environment tiers (`FLEETY_MODEL_*` / `FLEETY_CHEAP_MODEL_*`).
    pub fn from_env() -> Self {
        if let Some(cfg) = fleety_tools::providers_config::load() {
            if let Some(tiers) = Self::from_config(&cfg) {
                return tiers;
            }
        }
        Self::from_env_tiers()
    }

    /// The legacy two-tier env build: `main` from `FLEETY_MODEL_*` (echo stub
    /// when unset), `cheap` from `FLEETY_CHEAP_MODEL_*` (aliases main when unset).
    fn from_env_tiers() -> Self {
        Self::new(build_main(), build("FLEETY_CHEAP_MODEL"))
    }

    /// Build a registry from a parsed two-tier `providers.toml`. Returns `None`
    /// when no model roles are defined (so the caller falls back to the env
    /// tiers). Each model role becomes an entry resolvable by its name: a
    /// single-member role is that member's provider; a multi-member role is a
    /// [`PoolProvider`] over the members built from provider + member traits.
    fn from_config(cfg: &fleety_tools::providers_config::ProvidersConfig) -> Option<Self> {
        if cfg.models.is_empty() {
            return None;
        }
        let mut providers: std::collections::HashMap<String, Arc<dyn ModelProvider>> =
            std::collections::HashMap::new();
        for (role, pool) in &cfg.models {
            let members: Vec<Arc<dyn ModelProvider>> = pool
                .members
                .iter()
                .filter_map(|m| build_member(cfg, m))
                .collect();
            if members.is_empty() {
                tracing::warn!(role = %role,
                    "providers.toml model role has no resolvable members; skipping");
                continue;
            }
            let provider: Arc<dyn ModelProvider> = if members.len() == 1 {
                Arc::clone(&members[0])
            } else {
                Arc::new(crate::pool::PoolProvider::new(members, pool.strategy))
            };
            providers.insert(role.clone(), provider);
        }
        // Fallback `main` for unknown selectors: the `main` role, else `cheap`,
        // else any resolved role.
        let main = providers
            .get("main")
            .or_else(|| providers.get("cheap"))
            .cloned()
            .or_else(|| providers.values().next().cloned())?;
        Some(Self {
            providers,
            roles: std::collections::HashMap::new(),
            main,
        })
    }

    /// Construct from explicit providers (a `None` cheap aliases main). Holds
    /// just `"main"` and `"cheap"`, like the legacy env build. Used by tests and
    /// callers that wire providers directly.
    pub fn new(main: Arc<dyn ModelProvider>, cheap: Option<Arc<dyn ModelProvider>>) -> Self {
        let cheap = cheap.unwrap_or_else(|| Arc::clone(&main));
        let mut providers = std::collections::HashMap::new();
        providers.insert("main".to_string(), Arc::clone(&main));
        providers.insert("cheap".to_string(), cheap);
        Self {
            providers,
            roles: std::collections::HashMap::new(),
            main,
        }
    }

    /// The main-tier provider (the resolved fallback).
    pub fn main(&self) -> Arc<dyn ModelProvider> {
        Arc::clone(&self.main)
    }

    /// Resolve a tier/role selector: map it through `roles` if it's a role name,
    /// then look it up among the named providers/groups; an unknown selector
    /// resolves to `main`. Never errors.
    pub fn resolve(&self, tier: &str) -> Arc<dyn ModelProvider> {
        let target = self.roles.get(tier).map(String::as_str).unwrap_or(tier);
        self.providers
            .get(target)
            .cloned()
            .unwrap_or_else(|| Arc::clone(&self.main))
    }
}

/// Light heuristic: does this model name belong to a family that handles
/// images / audio / video? Used only for a startup warning — a miss is fine.
pub(crate) fn looks_multimodal(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    const HINTS: &[&str] = &[
        "gpt-4o",
        "gpt-4-vision",
        "gpt-4-turbo",
        "gpt-5",
        "o1",
        "o3",
        "claude-3",
        "claude-sonnet-4",
        "claude-opus-4",
        "claude-haiku-4",
        "claude-fable",
        "gemini-1.5",
        "gemini-2",
        "gemini-pro-vision",
        "llava",
        "llama-3-vision",
        "llama-3.2-vision",
        "pixtral",
        "qwen2-vl",
        "qwen2.5-vl",
        "molmo",
        "internvl",
    ];
    HINTS.iter().any(|h| m.contains(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    const KEYS: &[&str] = &[
        "FLEETY_MODEL_BASE_URL",
        "FLEETY_MODEL",
        "FLEETY_MODEL_KEY",
        "FLEETY_MODEL_STREAM",
        "FLEETY_CHEAP_MODEL_BASE_URL",
        "FLEETY_CHEAP_MODEL",
        "FLEETY_CHEAP_MODEL_KEY",
        "FLEETY_CHEAP_MODEL_STREAM",
    ];

    fn clear() {
        for k in KEYS {
            std::env::remove_var(k);
        }
        // Point providers.toml at a path that won't exist so `from_env` takes the
        // legacy env path regardless of any real ~/.fleety/providers.toml.
        std::env::set_var("FLEETY_PROVIDERS", "");
    }

    #[test]
    #[serial]
    fn cheap_unset_aliases_main() {
        clear();
        let tiers = ProviderTiers::from_env();
        // "Tier resolution and fallback": cheap unset → cheap resolves to main.
        assert!(Arc::ptr_eq(&tiers.resolve("cheap"), &tiers.resolve("main")));
        clear();
    }

    #[test]
    #[serial]
    fn cheap_set_builds_distinct_provider() {
        clear();
        std::env::set_var("FLEETY_CHEAP_MODEL_BASE_URL", "https://cheap.example/v1");
        std::env::set_var("FLEETY_CHEAP_MODEL", "gpt-4o-mini");
        let tiers = ProviderTiers::from_env();
        // "Optional second economy provider": configured cheap is a distinct provider.
        assert!(!Arc::ptr_eq(
            &tiers.resolve("cheap"),
            &tiers.resolve("main")
        ));
        clear();
    }

    #[test]
    fn auth_mode_selects_oauth_only_for_codex() {
        assert!(auth_is_oauth(Some("oauth:codex")));
        assert!(auth_is_oauth(Some("OAuth:Codex"))); // case-insensitive
        assert!(!auth_is_oauth(None)); // default → static
        assert!(!auth_is_oauth(Some("static")));
        assert!(!auth_is_oauth(Some("oauth:other")));
    }

    #[test]
    fn codex_token_path_uses_the_providers_own_name() {
        // A named provider reads its OWN per-provider token file; the env path
        // (no name) falls back to the legacy global path. (Assumes FLEETY_CODEX_TOKENS
        // is unset — no test sets it.)
        let named = codex_token_path(Some("tingzhen-codex"));
        assert!(named
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("codex-oauth/tingzhen-codex.json"));
        let other = codex_token_path(Some("work-codex"));
        assert_ne!(
            named, other,
            "different providers get different token files"
        );
        let env = codex_token_path(None);
        assert!(env
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(".fleety/codex-oauth.json"));
    }

    #[test]
    #[serial]
    fn build_reads_auth_mode_and_yields_a_provider() {
        clear();
        std::env::set_var("FLEETY_MODEL_BASE_URL", "https://api.example/v1");
        std::env::set_var("FLEETY_MODEL", "gpt-5");
        // Default (no _AUTH) builds a provider (static path).
        assert!(build("FLEETY_MODEL").is_some());
        // oauth:codex also builds a provider (bearer source attached internally).
        std::env::set_var("FLEETY_MODEL_AUTH", "oauth:codex");
        assert!(build("FLEETY_MODEL").is_some());
        clear();
        std::env::remove_var("FLEETY_MODEL_AUTH");
    }

    #[test]
    #[serial]
    fn build_is_none_when_unset_or_partial() {
        clear();
        assert!(build("FLEETY_CHEAP_MODEL").is_none());
        // Only base_url set, model missing → still None.
        std::env::set_var("FLEETY_CHEAP_MODEL_BASE_URL", "https://cheap.example/v1");
        assert!(build("FLEETY_CHEAP_MODEL").is_none());
        clear();
    }

    #[test]
    #[serial]
    fn unknown_tier_resolves_to_main() {
        clear();
        let tiers = ProviderTiers::from_env();
        assert!(Arc::ptr_eq(
            &tiers.resolve("anything-else"),
            &tiers.resolve("main")
        ));
        clear();
    }

    #[test]
    #[serial]
    fn providers_toml_builds_role_pools_and_one_provider_two_roles() {
        clear();
        let path =
            std::env::temp_dir().join(format!("fleety-providers-{}.toml", uuid::Uuid::new_v4()));
        // One provider serves gpt-4o to `main` (a two-member failover pool) and
        // gpt-4o-mini to `cheap` (single) — no duplicated provider.
        std::fs::write(
            &path,
            r#"
                [providers.openai1]
                type = "api"
                base_url = "https://api.openai.com/v1"
                key = "sk-a"

                [models.main]
                strategy = "failover"
                members = [
                  { provider = "openai1", model = "gpt-4o" },
                  { provider = "openai1", model = "gpt-4o-2" },
                ]

                [models.cheap]
                strategy = "single"
                members = [ { provider = "openai1", model = "gpt-4o-mini" } ]
            "#,
        )
        .expect("write providers.toml");
        std::env::set_var("FLEETY_PROVIDERS", &path);
        let tiers = ProviderTiers::from_env();
        // Both roles resolve; an unknown selector falls back to main.
        assert!(Arc::ptr_eq(
            &tiers.resolve("zzz-unknown"),
            &tiers.resolve("main")
        ));
        // main (a pool) and cheap (a single) are distinct providers.
        assert!(!Arc::ptr_eq(
            &tiers.resolve("main"),
            &tiers.resolve("cheap")
        ));
        let _ = std::fs::remove_file(&path);
        clear();
    }

    #[test]
    #[serial]
    fn migrate_and_check_allows_missing_and_rejects_broken() {
        clear();
        let path =
            std::env::temp_dir().join(format!("fleety-providers-{}.toml", uuid::Uuid::new_v4()));
        std::env::set_var("FLEETY_PROVIDERS", &path);
        // No providers.toml → the env bootstrap seed is allowed to take over.
        assert!(migrate_and_check().is_ok());
        // A two-tier config whose member references an undefined provider is fatal.
        std::fs::write(
            &path,
            "[models.main]\nstrategy = \"single\"\nmembers = [ { provider = \"ghost\", model = \"m\" } ]\n",
        )
        .expect("write");
        assert!(
            migrate_and_check().is_err(),
            "broken structured config must refuse to boot"
        );
        let _ = std::fs::remove_file(&path);
        clear();
    }

    #[test]
    #[serial]
    fn migrate_and_check_upgrades_legacy_in_place() {
        clear();
        let path =
            std::env::temp_dir().join(format!("fleety-providers-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            r#"
                [[provider]]
                name = "codex-1"
                base_url = "https://api.openai.com/v1"
                model = "gpt-5"

                [[group]]
                name = "codex"
                members = ["codex-1"]
                strategy = "failover"

                [roles]
                main = "codex"
            "#,
        )
        .expect("write legacy");
        std::env::set_var("FLEETY_PROVIDERS", &path);
        migrate_and_check().expect("migrate ok");
        // The file is now two-tier and reloads cleanly.
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("[providers."), "migrated shape: {text}");
        assert!(text.contains("[models."), "migrated shape: {text}");
        // The migrated config builds a runtime without panicking.
        let _ = ProviderTiers::from_env().resolve("main");
        let _ = std::fs::remove_file(&path);
        clear();
    }
}
