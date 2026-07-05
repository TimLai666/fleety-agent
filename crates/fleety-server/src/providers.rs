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
    Some(build_provider(
        base_url,
        model,
        key,
        stream,
        modalities.as_deref(),
        effort.as_deref(),
        auth.as_deref(),
        prefix,
    ))
}

/// Whether an auth-mode string selects the Codex OAuth bearer. `None`/`"static"`
/// (or anything else) keeps the static-key path. Pure — unit-testable.
fn auth_is_oauth(auth: Option<&str>) -> bool {
    matches!(auth, Some(a) if a.eq_ignore_ascii_case("oauth:codex"))
}

/// Build one provider from explicit fields (shared by the env path and the
/// `providers.toml` path). Picks the native Gemini vs OpenAI-compatible backend
/// from the model name, resolves modality capabilities (explicit `modalities`
/// wins, else the model-family heuristic, else text-only with a warning), and
/// attaches the family's effort scheme + optional default effort. `label` is
/// only used for log lines (the env prefix or the provider name).
pub fn build_provider(
    base_url: String,
    model: String,
    key: Option<String>,
    stream: bool,
    modalities: Option<&str>,
    effort: Option<&str>,
    auth: Option<&str>,
    label: &str,
) -> Arc<dyn ModelProvider> {
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
        return build_codex_responses(model, caps, default_effort, label);
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

/// Build the Codex Responses provider for the `oauth:codex` auth mode: it calls
/// the ChatGPT backend over the Responses API with the account's OAuth token
/// (refreshed on demand). The configured `base_url`/`key` are ignored — Codex has
/// its own backend, resolved from `FLEETY_CODEX_BACKEND_URL`.
fn build_codex_responses(
    model: String,
    caps: agent_core::model::ModelCapabilities,
    default_effort: Option<agent_core::model::Effort>,
    label: &str,
) -> Arc<dyn ModelProvider> {
    let cfg = fleety_tools::oauth::oauth_config();
    let endpoint = format!("{}/responses", cfg.backend_base_url.trim_end_matches('/'));
    tracing::info!(%endpoint, %model, %label, "using Codex Responses provider (oauth:codex)");
    let auth_src =
        fleety_tools::oauth::OAuthCodexAuth::new(fleety_tools::oauth::default_token_path(), &cfg);
    Arc::new(
        CodexResponses::new(endpoint, model, Arc::new(auth_src))
            .with_capabilities(caps)
            .with_effort(default_effort),
    )
}

/// Build one provider from a `providers.toml` entry.
fn build_from_spec(spec: &fleety_tools::providers_config::ProviderSpec) -> Arc<dyn ModelProvider> {
    build_provider(
        spec.base_url.clone(),
        spec.model.clone(),
        spec.key.clone(),
        spec.stream,
        spec.modalities.as_deref(),
        spec.effort.as_deref(),
        spec.auth.as_deref(),
        &spec.name,
    )
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

    /// Build a registry from a parsed `providers.toml`. Returns `None` when no
    /// providers are defined (so the caller falls back to the env tiers). Groups
    /// become [`PoolProvider`]s over their members; a group with no resolvable
    /// member is skipped with a warning.
    fn from_config(cfg: &fleety_tools::providers_config::ProvidersConfig) -> Option<Self> {
        if cfg.providers.is_empty() {
            return None;
        }
        let mut providers: std::collections::HashMap<String, Arc<dyn ModelProvider>> =
            std::collections::HashMap::new();
        for spec in &cfg.providers {
            providers.insert(spec.name.clone(), build_from_spec(spec));
        }
        for g in &cfg.groups {
            let members: Vec<Arc<dyn ModelProvider>> = g
                .members
                .iter()
                .filter_map(|m| providers.get(m).cloned())
                .collect();
            if members.is_empty() {
                tracing::warn!(group = %g.name,
                    "providers.toml group has no resolvable members; skipping");
                continue;
            }
            let pool = Arc::new(crate::pool::PoolProvider::new(members, g.strategy))
                as Arc<dyn ModelProvider>;
            providers.insert(g.name.clone(), pool);
        }
        let roles: std::collections::HashMap<String, String> =
            cfg.roles.clone().into_iter().collect();
        // Resolve the fallback `main`: the `main` role's target, else a provider
        // literally named "main", else the first defined provider.
        let main = roles
            .get("main")
            .and_then(|n| providers.get(n).cloned())
            .or_else(|| providers.get("main").cloned())
            .or_else(|| {
                cfg.providers
                    .first()
                    .and_then(|s| providers.get(&s.name).cloned())
            })?;
        Some(Self {
            providers,
            roles,
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
    fn providers_toml_builds_named_pool_and_roles() {
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

                [[provider]]
                name = "codex-2"
                base_url = "https://api.openai.com/v1"
                model = "gpt-5"

                [[group]]
                name = "codex"
                members = ["codex-1", "codex-2"]
                strategy = "round_robin"

                [roles]
                main = "codex"
            "#,
        )
        .expect("write providers.toml");
        std::env::set_var("FLEETY_PROVIDERS", &path);
        let tiers = ProviderTiers::from_env();
        // The `main` role maps to the `codex` group, so both resolve to the same
        // pooled provider; an unknown tier falls back to `main`.
        assert!(Arc::ptr_eq(&tiers.resolve("main"), &tiers.resolve("codex")));
        assert!(Arc::ptr_eq(
            &tiers.resolve("zzz-unknown"),
            &tiers.resolve("main")
        ));
        // A bare provider name resolves to that single provider, distinct from the group.
        assert!(!Arc::ptr_eq(
            &tiers.resolve("codex-1"),
            &tiers.resolve("codex")
        ));
        let _ = std::fs::remove_file(&path);
        clear();
    }
}
