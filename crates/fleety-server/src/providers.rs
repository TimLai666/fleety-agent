//! Model-provider construction and tiers.
//!
//! [`build`] turns `{prefix}_BASE_URL` + `{prefix}_MODEL` (+ optional
//! `{prefix}_KEY` / `{prefix}_STREAM`) into a provider, mirroring exactly how
//! the main model is configured. [`ProviderTiers`] holds the main provider plus
//! an optional cheap "economy" provider: when the cheap model is unset, the
//! cheap tier aliases main (the same `Arc`), so selecting `cheap` always yields
//! a valid provider and never errors. Subagents pick a tier per spawn.

use std::sync::Arc;

use agent_core::{Gemini, ModelProvider, OpenAiCompat};

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
    if !looks_multimodal(&model) {
        tracing::warn!(
            %model, %prefix,
            "configured model name doesn't match a known multimodal family — \
             image/audio/video attachments may be rejected or ignored by the provider"
        );
    }
    Some(if agent_core::gemini::looks_like_gemini_model(&model) {
        tracing::info!(%base_url, %model, stream, %prefix, "using native Gemini provider");
        Arc::new(Gemini::new(base_url, model, key).with_streaming(stream))
    } else {
        tracing::info!(%base_url, %model, stream, %prefix, "using OpenAI-compatible provider");
        Arc::new(OpenAiCompat::new(base_url, model, key).with_streaming(stream))
    })
}

/// The main model provider (`FLEETY_MODEL_*`), falling back to the offline echo
/// stub when unset.
pub fn build_main() -> Arc<dyn ModelProvider> {
    build("FLEETY_MODEL").unwrap_or_else(|| Arc::new(EchoProvider))
}

/// Main + optional cheap "economy" provider. A subagent selects a tier per
/// spawn; the tier only changes which provider runs, never the agent's policy,
/// gate, or audit.
#[derive(Clone)]
pub struct ProviderTiers {
    main: Arc<dyn ModelProvider>,
    cheap: Arc<dyn ModelProvider>,
}

impl ProviderTiers {
    /// Build both tiers from the environment. The cheap tier comes from
    /// `FLEETY_CHEAP_MODEL_*`; when that is unset it aliases the main provider.
    pub fn from_env() -> Self {
        Self::new(build_main(), build("FLEETY_CHEAP_MODEL"))
    }

    /// Construct from explicit providers. A `None` cheap aliases main.
    pub fn new(main: Arc<dyn ModelProvider>, cheap: Option<Arc<dyn ModelProvider>>) -> Self {
        let cheap = cheap.unwrap_or_else(|| Arc::clone(&main));
        Self { main, cheap }
    }

    /// The main-tier provider.
    pub fn main(&self) -> Arc<dyn ModelProvider> {
        Arc::clone(&self.main)
    }

    /// Resolve a tier selector: `"cheap"` → the cheap provider; anything else
    /// (including `"main"`) → the main provider. Never errors.
    pub fn resolve(&self, tier: &str) -> Arc<dyn ModelProvider> {
        match tier {
            "cheap" => Arc::clone(&self.cheap),
            _ => Arc::clone(&self.main),
        }
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
}
