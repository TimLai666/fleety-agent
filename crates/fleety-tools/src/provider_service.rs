//! Pure Provider/Auth/Model workflow state shared by command and terminal UIs.
//!
//! Transport, browser OAuth, persistence, and rendering stay in their owning
//! layers. This module preserves typed state and errors until those layers map
//! them to effects or output.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::ProviderCmd;
use crate::providers_config::{self as pc, ModelPool, Provider, ProvidersConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIssue {
    pub kind: String,
    pub message: String,
    pub remediation: Option<String>,
}

impl ProviderIssue {
    pub fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        remediation: Option<impl Into<String>>,
    ) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            remediation: remediation.map(Into::into),
        }
    }

    pub fn display(&self) -> String {
        match &self.remediation {
            Some(remediation) => format!("{} — {remediation}", self.message),
            None => self.message.clone(),
        }
    }
}

impl std::fmt::Display for ProviderIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.display())
    }
}

impl std::error::Error for ProviderIssue {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointClass {
    OpenAiHosted,
    CustomApi,
}

impl EndpointClass {
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpenAiHosted => "OpenAI hosted",
            Self::CustomApi => "Custom API",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderView {
    pub name: String,
    pub kind: String,
    pub endpoint: EndpointClass,
    pub key: Option<ApiKeyState>,
    pub auth: AuthState,
    pub catalog: CatalogState,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyState {
    Set,
    NotSet,
}

impl ApiKeyState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Set => "Set",
            Self::NotSet => "Not set",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCommandOutcome {
    pub config: ProvidersConfig,
    pub key_present: BTreeSet<String>,
    pub output: String,
    pub changed: bool,
    pub clear_keys: BTreeSet<String>,
}

fn validation_issue(error: agent_core::CoreError) -> ProviderIssue {
    ProviderIssue::new(
        "invalid_provider",
        error.report().message,
        Some("Fix the Provider fields and retry"),
    )
}

pub fn validate_provider_input(
    name: &str,
    kind: &str,
    base_url: Option<&str>,
    key: Option<&str>,
) -> Result<(), ProviderIssue> {
    let mut config = ProvidersConfig::default();
    config.providers.insert(
        name.to_string(),
        Provider {
            kind: kind.to_string(),
            base_url: base_url.map(ToOwned::to_owned),
            key: key.map(ToOwned::to_owned),
        },
    );
    pc::validate(&config).map_err(validation_issue)
}

pub fn add_provider(
    config: &mut ProvidersConfig,
    name: String,
    kind: String,
    base_url: Option<String>,
    key: Option<String>,
) -> Result<(), ProviderIssue> {
    if config.providers.contains_key(&name) {
        return Err(ProviderIssue::new(
            "already_exists",
            format!("Provider '{name}' already exists"),
            Some("Choose another name or use `fleety provider set`"),
        ));
    }
    validate_provider_input(&name, &kind, base_url.as_deref(), key.as_deref())?;
    config.providers.insert(
        name,
        Provider {
            kind,
            base_url,
            key,
        },
    );
    Ok(())
}

pub fn set_provider(
    config: &mut ProvidersConfig,
    name: &str,
    kind: Option<String>,
    base_url: Option<String>,
    key: Option<String>,
    clear_key: bool,
) -> Result<(), ProviderIssue> {
    let current = config.providers.get(name).cloned().ok_or_else(|| {
        ProviderIssue::new(
            "not_found",
            format!("No Provider named '{name}'"),
            Some("Run `fleety provider list`"),
        )
    })?;
    let next_kind = kind.unwrap_or_else(|| current.kind.clone());
    let kind_changed = next_kind != current.kind;
    let next_type = pc::provider_type(&next_kind);
    let next = Provider {
        kind: next_kind,
        base_url: base_url.or_else(|| {
            (!kind_changed || next_type.is_some_and(|kind| kind.requires_base_url))
                .then_some(current.base_url)
                .flatten()
        }),
        key: if clear_key {
            None
        } else {
            key.or_else(|| {
                (!kind_changed || next_type.is_some_and(|kind| kind.allows_key))
                    .then_some(current.key)
                    .flatten()
            })
        },
    };
    validate_provider_input(
        name,
        &next.kind,
        next.base_url.as_deref(),
        next.key.as_deref(),
    )?;
    config.providers.insert(name.to_string(), next);
    Ok(())
}

pub fn remove_provider(config: &mut ProvidersConfig, name: &str) -> Result<(), ProviderIssue> {
    if !config.providers.contains_key(name) {
        return Err(ProviderIssue::new(
            "not_found",
            format!("No Provider named '{name}'"),
            Some("Run `fleety provider list`"),
        ));
    }
    if let Some(role) = config.role_referencing(name) {
        return Err(ProviderIssue::new(
            "in_use",
            format!("Model role '{role}' uses Provider '{name}'"),
            Some(format!("Unset or update role '{role}' first")),
        ));
    }
    config.providers.remove(name);
    Ok(())
}

pub fn set_model(
    config: &mut ProvidersConfig,
    role: String,
    members: Vec<crate::providers_config::Member>,
    strategy: crate::providers_config::Strategy,
) -> Result<(), ProviderIssue> {
    let mut candidate = config.clone();
    candidate
        .models
        .insert(role.clone(), ModelPool { strategy, members });
    pc::validate(&candidate).map_err(validation_issue)?;
    *config = candidate;
    Ok(())
}

pub fn unset_model(config: &mut ProvidersConfig, role: &str) -> Result<(), ProviderIssue> {
    if config.models.remove(role).is_none() {
        return Err(ProviderIssue::new(
            "not_found",
            format!("No model role '{role}'"),
            Some("Run `fleety model list`"),
        ));
    }
    Ok(())
}

pub fn apply_command(
    current: &ProvidersConfig,
    current_key_present: &BTreeSet<String>,
    command: ProviderCmd,
) -> Result<ProviderCommandOutcome, ProviderIssue> {
    let mut config = current.clone();
    let mut key_present = current_key_present.clone();
    let mut clear_keys = BTreeSet::new();
    let (output, changed) = match command {
        ProviderCmd::ProviderAdd {
            name,
            kind,
            base_url,
            key,
        } => {
            let has_key = key.is_some();
            add_provider(&mut config, name.clone(), kind, base_url, key)?;
            if has_key {
                key_present.insert(name.clone());
            } else {
                key_present.remove(&name);
            }
            (format!("Added Provider '{name}'"), true)
        }
        ProviderCmd::ProviderSet {
            name,
            kind,
            base_url,
            key,
            clear_key,
        } => {
            let sets_key = key.is_some();
            set_provider(&mut config, &name, kind, base_url, key, clear_key)?;
            let allows_key = config
                .providers
                .get(&name)
                .and_then(|provider| pc::provider_type(&provider.kind))
                .is_some_and(|kind| kind.allows_key);
            if clear_key {
                clear_keys.insert(name.clone());
                key_present.remove(&name);
            } else if !allows_key {
                key_present.remove(&name);
            } else if sets_key {
                key_present.insert(name.clone());
            }
            (format!("Updated Provider '{name}'"), true)
        }
        ProviderCmd::ProviderRemove(name) => {
            remove_provider(&mut config, &name)?;
            key_present.remove(&name);
            (format!("Removed Provider '{name}'"), true)
        }
        ProviderCmd::ProviderList => (String::new(), false),
        ProviderCmd::ModelSet {
            role,
            members,
            strategy,
        } => {
            set_model(&mut config, role.clone(), members, strategy)?;
            (format!("Set model role '{role}'"), true)
        }
        ProviderCmd::ModelUnset(role) => {
            unset_model(&mut config, &role)?;
            (format!("Unset model role '{role}'"), true)
        }
        ProviderCmd::ModelList => (render_model_roles(&config, None)?, false),
        ProviderCmd::ModelShow(role) => (render_model_roles(&config, role.as_deref())?, false),
    };
    Ok(ProviderCommandOutcome {
        config,
        key_present,
        output,
        changed,
        clear_keys,
    })
}

fn render_model_roles(
    config: &ProvidersConfig,
    only: Option<&str>,
) -> Result<String, ProviderIssue> {
    let roles: Vec<_> = match only {
        Some(role) => vec![(
            role,
            config.models.get(role).ok_or_else(|| {
                ProviderIssue::new(
                    "not_found",
                    format!("No model role '{role}'"),
                    Some("Run `fleety model list`"),
                )
            })?,
        )],
        None => config
            .models
            .iter()
            .map(|(role, pool)| (role.as_str(), pool))
            .collect(),
    };
    if roles.is_empty() {
        return Ok("No model roles configured".to_string());
    }
    Ok(roles
        .into_iter()
        .map(|(role, pool)| {
            let members = pool
                .members
                .iter()
                .map(|member| format!("{}/{}", member.provider, member.model))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{role}: {:?} · {members}", pool.strategy)
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

pub fn provider_views(
    config: &ProvidersConfig,
    key_present: &BTreeSet<String>,
    auth_states: &BTreeMap<String, AuthState>,
    config_protocol: u32,
) -> Vec<ProviderView> {
    config
        .providers
        .iter()
        .map(|(name, provider)| {
            let auth = auth_states.get(name).cloned().unwrap_or_else(|| {
                if provider.is_oauth() {
                    AuthState::Unavailable(ProviderIssue::new(
                        "unknown",
                        "Provider authentication state is unavailable",
                        Some("Refresh Provider status"),
                    ))
                } else {
                    AuthState::NotApplicable
                }
            });
            let endpoint = if provider.is_oauth() {
                EndpointClass::OpenAiHosted
            } else {
                EndpointClass::CustomApi
            };
            let roles = config
                .models
                .iter()
                .filter(|(_, pool)| pool.members.iter().any(|member| member.provider == *name))
                .map(|(role, _)| role.clone())
                .collect();
            let key = pc::provider_types()
                .iter()
                .find(|kind| kind.name.eq_ignore_ascii_case(&provider.kind))
                .is_some_and(|kind| kind.allows_key)
                .then(|| {
                    if key_present.contains(name) {
                        ApiKeyState::Set
                    } else {
                        ApiKeyState::NotSet
                    }
                });
            ProviderView {
                name: name.clone(),
                kind: provider.kind.clone(),
                endpoint,
                key,
                catalog: catalog_gate(&provider.kind, &auth, config_protocol),
                auth,
                roles,
            }
        })
        .collect()
}

pub fn catalog_label(state: &CatalogState) -> &'static str {
    match state {
        CatalogState::Idle => "Ready",
        CatalogState::Loading { .. } => "Loading",
        CatalogState::Available(_) => "Available",
        CatalogState::Failed(_) => "Failed",
        CatalogState::Unavailable(_) => "Unavailable",
        CatalogState::Manual { .. } => "Manual ID",
    }
}

pub fn render_provider_views(views: &[ProviderView]) -> String {
    if views.is_empty() {
        return "No Providers configured".to_string();
    }
    views
        .iter()
        .map(|view| {
            let key = view
                .key
                .map(|state| format!(" · key={}", state.label()))
                .unwrap_or_default();
            format!(
                "{}: type={} · endpoint={}{} · auth={} · catalog={} · roles={}",
                view.name,
                view.kind,
                view.endpoint.label(),
                key,
                view.auth.label(),
                catalog_label(&view.catalog),
                if view.roles.is_empty() {
                    "none".to_string()
                } else {
                    view.roles.join(",")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    NotApplicable,
    Checking,
    SignedIn,
    NotSignedIn,
    Expired,
    Unavailable(ProviderIssue),
}

impl AuthState {
    pub fn from_observation(
        present: bool,
        expires_at_secs: Option<u64>,
        error: Option<ProviderIssue>,
        now_secs: u64,
    ) -> Self {
        if let Some(error) = error {
            return Self::Unavailable(error);
        }
        if !present {
            return Self::NotSignedIn;
        }
        if expires_at_secs.is_some_and(|expiry| expiry <= now_secs) {
            return Self::Expired;
        }
        Self::SignedIn
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::NotApplicable => "Not applicable",
            Self::Checking => "Checking",
            Self::SignedIn => "Signed in",
            Self::NotSignedIn => "Not signed in",
            Self::Expired => "Expired",
            Self::Unavailable(_) => "Unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogState {
    Idle,
    Loading {
        previous_error: Option<ProviderIssue>,
    },
    Available(Vec<String>),
    Failed(ProviderIssue),
    Unavailable(ProviderIssue),
    Manual {
        previous_error: Option<ProviderIssue>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRequest {
    pub connection_id: String,
    pub provider: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub connection_id: String,
    pub provider: String,
    pub role: String,
    pub catalog: CatalogState,
}

impl ModelSelection {
    pub fn loading(
        connection_id: impl Into<String>,
        provider: impl Into<String>,
        role: impl Into<String>,
    ) -> Self {
        Self {
            connection_id: connection_id.into(),
            provider: provider.into(),
            role: role.into(),
            catalog: CatalogState::Loading {
                previous_error: None,
            },
        }
    }

    pub fn request(&self) -> CatalogRequest {
        CatalogRequest {
            connection_id: self.connection_id.clone(),
            provider: self.provider.clone(),
            role: self.role.clone(),
        }
    }

    pub fn finish(&mut self, result: Result<Vec<String>, ProviderIssue>) {
        self.catalog = match result {
            Ok(models) if !models.is_empty() => CatalogState::Available(models),
            Ok(_) => CatalogState::Failed(ProviderIssue::new(
                "empty_catalog",
                "Server returned no model IDs",
                Some("Retry or enter a model ID"),
            )),
            Err(error) => CatalogState::Failed(error),
        };
    }

    pub fn retry(&mut self) -> Option<CatalogRequest> {
        let previous_error = match &self.catalog {
            CatalogState::Failed(error) => Some(error.clone()),
            CatalogState::Unavailable(error) => Some(error.clone()),
            CatalogState::Manual { previous_error } => previous_error.clone(),
            _ => return None,
        };
        self.catalog = CatalogState::Loading { previous_error };
        Some(self.request())
    }

    pub fn enter_manual(&mut self) {
        let previous_error = match &self.catalog {
            CatalogState::Failed(error) => Some(error.clone()),
            CatalogState::Loading { previous_error } => previous_error.clone(),
            CatalogState::Unavailable(error) => Some(error.clone()),
            CatalogState::Manual { previous_error } => previous_error.clone(),
            CatalogState::Idle | CatalogState::Available(_) => None,
        };
        self.catalog = CatalogState::Manual { previous_error };
    }
}

pub fn catalog_gate(provider_kind: &str, auth: &AuthState, config_protocol: u32) -> CatalogState {
    if config_protocol < 4 {
        return CatalogState::Unavailable(ProviderIssue::new(
            "unsupported",
            "Server does not support Provider model discovery",
            Some("Update the Server"),
        ));
    }
    if !provider_kind.eq_ignore_ascii_case("oauth:codex") {
        return CatalogState::Idle;
    }
    match auth {
        AuthState::SignedIn => CatalogState::Idle,
        AuthState::NotSignedIn => CatalogState::Unavailable(ProviderIssue::new(
            "not_signed_in",
            "Not signed in to this Provider",
            Some("Run `fleety provider login <provider>`"),
        )),
        AuthState::Expired => CatalogState::Unavailable(ProviderIssue::new(
            "expired",
            "Provider login has expired",
            Some("Run `fleety provider login <provider>`"),
        )),
        AuthState::Checking => CatalogState::Unavailable(ProviderIssue::new(
            "checking",
            "Provider sign-in state is still being checked",
            Some("Retry"),
        )),
        AuthState::Unavailable(error) => CatalogState::Unavailable(error.clone()),
        AuthState::NotApplicable => CatalogState::Unavailable(ProviderIssue::new(
            "invalid_auth_state",
            "OAuth Provider has no authentication state",
            Some("Refresh Provider status"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers_config::{Member, Strategy};

    fn oauth_provider() -> Provider {
        Provider {
            kind: "oauth:codex".into(),
            base_url: None,
            key: None,
        }
    }

    #[test]
    fn validation_is_typed_and_rejects_bad_names_and_endpoints_before_mutation() {
        let bad_name =
            validate_provider_input("../escape", "api", Some("https://example.test/v1"), None)
                .expect_err("unsafe name");
        assert_eq!(bad_name.kind, "invalid_provider");
        assert_eq!(
            bad_name.remediation.as_deref(),
            Some("Fix the Provider fields and retry")
        );

        let bad_endpoint = validate_provider_input("safe", "api", Some("example.test/v1"), None)
            .expect_err("relative endpoint");
        assert_eq!(bad_endpoint.kind, bad_name.kind);
        assert_eq!(bad_endpoint.remediation, bad_name.remediation);

        let mut config = ProvidersConfig::default();
        let error = add_provider(
            &mut config,
            "safe".into(),
            "api".into(),
            Some("example.test/v1".into()),
            None,
        )
        .expect_err("invalid add");
        assert_eq!(error, bad_endpoint);
        assert!(config.providers.is_empty(), "validation precedes mutation");

        let blank_key =
            validate_provider_input("safe", "api", Some("https://example.test/v1"), Some(""))
                .expect_err("blank API key");
        assert_eq!(blank_key.kind, "invalid_provider");
    }

    #[test]
    fn provider_view_includes_type_endpoint_auth_catalog_and_every_bound_role() {
        let mut config = ProvidersConfig::default();
        config.providers.insert("codex".into(), oauth_provider());
        for role in ["cheap", "main"] {
            config.models.insert(
                role.into(),
                ModelPool {
                    strategy: Strategy::Single,
                    members: vec![Member {
                        provider: "codex".into(),
                        model: "gpt-test".into(),
                        stream: false,
                        modalities: None,
                        effort: None,
                    }],
                },
            );
        }
        let auth = BTreeMap::from([("codex".into(), AuthState::NotSignedIn)]);
        let views = provider_views(&config, &BTreeSet::new(), &auth, 4);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].kind, "oauth:codex");
        assert_eq!(views[0].endpoint, EndpointClass::OpenAiHosted);
        assert_eq!(views[0].auth, AuthState::NotSignedIn);
        assert!(matches!(views[0].catalog, CatalogState::Unavailable(_)));
        assert_eq!(views[0].roles, vec!["cheap", "main"]);
        let rendered = render_provider_views(&views);
        for field in [
            "type=oauth:codex",
            "endpoint=OpenAI hosted",
            "auth=Not signed in",
            "catalog=Unavailable",
            "roles=cheap,main",
        ] {
            assert!(rendered.contains(field), "missing {field}: {rendered}");
        }
    }

    #[test]
    fn api_provider_view_reports_non_secret_key_state() {
        let mut config = ProvidersConfig::default();
        config.providers.insert(
            "openai".into(),
            Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        let views = provider_views(&config, &BTreeSet::new(), &BTreeMap::new(), 5);
        let rendered = render_provider_views(&views);

        assert!(rendered.contains("key=Not set"), "{rendered}");
    }

    #[test]
    fn provider_commands_preserve_and_transition_key_presence_without_secret_output() {
        let added = apply_command(
            &ProvidersConfig::default(),
            &BTreeSet::new(),
            ProviderCmd::ProviderAdd {
                name: "openai".into(),
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        )
        .expect("add without key");
        assert!(!added.key_present.contains("openai"));

        let added_with_key = apply_command(
            &ProvidersConfig::default(),
            &BTreeSet::new(),
            ProviderCmd::ProviderAdd {
                name: "keyed".into(),
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: Some("add-secret".into()),
            },
        )
        .expect("add with key");
        assert!(added_with_key.key_present.contains("keyed"));
        assert!(!added_with_key.output.contains("add-secret"));

        let set = apply_command(
            &added.config,
            &added.key_present,
            ProviderCmd::ProviderSet {
                name: "openai".into(),
                kind: None,
                base_url: None,
                key: Some("transition-secret".into()),
                clear_key: false,
            },
        )
        .expect("set key");
        assert!(set.key_present.contains("openai"));

        let mut redacted_after_set = set.config.clone();
        redacted_after_set
            .providers
            .get_mut("openai")
            .expect("openai")
            .key = None;
        let kept = apply_command(
            &redacted_after_set,
            &BTreeSet::from(["openai".to_string()]),
            ProviderCmd::ProviderSet {
                name: "openai".into(),
                kind: None,
                base_url: Some("https://other.example.test/v1".into()),
                key: None,
                clear_key: false,
            },
        )
        .expect("preserve key");
        assert!(kept.key_present.contains("openai"));

        let oauth = apply_command(
            &redacted_after_set,
            &BTreeSet::from(["openai".to_string()]),
            ProviderCmd::ProviderSet {
                name: "openai".into(),
                kind: Some("oauth:codex".into()),
                base_url: None,
                key: None,
                clear_key: false,
            },
        )
        .expect("change to OAuth");
        assert!(!oauth.key_present.contains("openai"));
        assert!(oauth.clear_keys.is_empty());

        let cleared = apply_command(
            &kept.config,
            &kept.key_present,
            ProviderCmd::ProviderSet {
                name: "openai".into(),
                kind: None,
                base_url: None,
                key: None,
                clear_key: true,
            },
        )
        .expect("clear key");
        assert!(!cleared.key_present.contains("openai"));
        assert!(!cleared.output.contains("transition-secret"));
    }

    #[test]
    fn typed_command_mutates_snapshot_without_touching_files() {
        let current = ProvidersConfig::default();
        let outcome = apply_command(
            &current,
            &BTreeSet::new(),
            ProviderCmd::ProviderAdd {
                name: "codex".into(),
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        )
        .expect("valid command");
        assert!(outcome.changed);
        assert!(outcome.config.providers.contains_key("codex"));
        assert!(current.providers.is_empty());
    }

    #[test]
    fn clear_key_is_an_explicit_sidecar_intent() {
        let mut current = ProvidersConfig::default();
        current.providers.insert(
            "api".into(),
            Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        let outcome = apply_command(
            &current,
            &BTreeSet::from(["api".to_string()]),
            ProviderCmd::ProviderSet {
                name: "api".into(),
                kind: None,
                base_url: None,
                key: None,
                clear_key: true,
            },
        )
        .expect("clear command");
        assert_eq!(outcome.clear_keys, BTreeSet::from(["api".to_string()]));
        assert!(outcome.config.provider("api").expect("api").key.is_none());
    }

    #[test]
    fn retry_preserves_request_identity_and_previous_error() {
        let mut selection = ModelSelection::loading("server-a", "codex", "main");
        selection.finish(Err(ProviderIssue::new(
            "backend",
            "backend denied catalog",
            Some("Retry"),
        )));
        let retry = selection.retry().expect("retry");
        assert_eq!(retry.connection_id, "server-a");
        assert_eq!(retry.provider, "codex");
        assert_eq!(retry.role, "main");
        let CatalogState::Loading { previous_error } = selection.catalog else {
            panic!("loading after retry")
        };
        assert_eq!(
            previous_error.map(|issue| issue.message),
            Some("backend denied catalog".into())
        );
    }
}
