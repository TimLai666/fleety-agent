//! Shared Provider/Auth/Model application service.

pub use fleety_tools::provider_service::{
    add_provider, apply_command, catalog_gate, catalog_label, provider_views, remove_provider,
    render_provider_views, set_model, set_provider, unset_model, AuthState, CatalogRequest,
    CatalogState, ModelSelection, ProviderIssue,
};

use std::collections::{BTreeMap, BTreeSet};

use agent_core::{CoreError, Result};
use fleety_protocol::{ClientMsg, ConfigTarget, ServerMsg, WireError};
use fleety_tools::providers_config::ProvidersConfig;
use fleety_tools::transport::{Receiver as Rx, Sender as Tx};

pub struct ProviderSnapshot {
    pub revision: String,
    pub entries: Vec<fleety_protocol::ConfigEntry>,
    pub config: ProvidersConfig,
    pub key_present: BTreeSet<String>,
}

pub fn issue_from_wire(error: WireError) -> ProviderIssue {
    ProviderIssue::new(error.kind, error.message, error.remediation)
}

pub(crate) fn redact_provider_secrets(text: &str, config: &ProvidersConfig) -> String {
    let mut secrets = config
        .providers
        .values()
        .filter_map(|provider| provider.key.as_deref())
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    secrets.sort_unstable_by_key(|key| std::cmp::Reverse(key.len()));
    secrets.dedup();
    secrets.into_iter().fold(text.to_string(), |safe, key| {
        safe.replace(key, "<redacted>")
    })
}

fn redact_wire_provider_secrets(mut error: WireError, config: &ProvidersConfig) -> WireError {
    error.message = redact_provider_secrets(&error.message, config);
    error.remediation = error
        .remediation
        .map(|hint| redact_provider_secrets(&hint, config));
    error
}

pub fn issue_as_error(issue: ProviderIssue) -> CoreError {
    CoreError::Message(match issue.remediation {
        Some(remediation) => format!("{} — {remediation}", issue.message),
        None => issue.message,
    })
}

pub fn validate_server_identity(
    expected: Option<&str>,
    actual: Option<&str>,
    operation: &str,
) -> std::result::Result<(), ProviderIssue> {
    match (expected, actual) {
        (Some(expected), Some(actual)) if expected == actual => Ok(()),
        (None, _) => Err(ProviderIssue::new(
            "server_identity_unavailable",
            format!(
                "{operation} was refused because the original Server did not provide a stable identity"
            ),
            Some("Update the Server and reopen the Provider editor"),
        )),
        _ => Err(ProviderIssue::new(
            "server_identity_changed",
            format!("{operation} was refused because the Server identity changed"),
            Some("Close the editor and verify the selected Server profile"),
        )),
    }
}

fn unexpected(expected: &str, reply: Option<&ServerMsg>) -> ProviderIssue {
    ProviderIssue::new(
        "unexpected_reply",
        format!(
            "Expected {expected}, got {}",
            crate::server_msg_kind_option(reply)
        ),
        Some("Retry the operation"),
    )
}

pub async fn load_snapshot(
    tx: &mut Tx,
    rx: &mut Rx,
    config_protocol: u32,
) -> Result<ProviderSnapshot> {
    if config_protocol < 5 {
        return Err(CoreError::Message(
            "the connected Server is too old for write-only Provider secrets — update it before viewing or editing Providers; no Provider snapshot was requested"
                .to_string(),
        ));
    }
    crate::send(
        tx,
        &ClientMsg::ConfigSnapshot {
            target: ConfigTarget::Server,
        },
    )
    .await?;
    match crate::recv(rx).await? {
        Some(ServerMsg::ConfigSnapshotResult {
            revision,
            entries,
            providers_json,
        }) => {
            let value: serde_json::Value =
                serde_json::from_str(&providers_json).map_err(|error| {
                    CoreError::Message(format!(
                        "The Server returned an unreadable Provider snapshot: {error}"
                    ))
                })?;
            let key_metadata = value
                .get("key_present")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    CoreError::Message(
                        "The Server returned an unsafe Provider snapshot without write-only key metadata; update or restart the Server"
                            .to_string(),
                    )
                })?;
            let mut key_present = BTreeSet::new();
            for (index, item) in key_metadata.iter().enumerate() {
                let name = item.as_str().ok_or_else(|| {
                    CoreError::Message(format!(
                        "The Server returned malformed Provider key metadata at index {index}; expected a Provider name string"
                    ))
                })?;
                if !key_present.insert(name.to_string()) {
                    return Err(CoreError::Message(format!(
                        "The Server returned duplicate Provider key metadata at index {index}; the snapshot was refused"
                    )));
                }
            }
            let config: ProvidersConfig = serde_json::from_value(value).map_err(|error| {
                CoreError::Message(format!(
                    "The Server returned an unreadable Provider snapshot: {error}"
                ))
            })?;
            if config
                .providers
                .keys()
                .any(|name| !fleety_tools::providers_config::valid_provider_name(name))
            {
                return Err(CoreError::Message(
                    "The Server returned malformed Provider key metadata with an invalid Provider name; the snapshot was refused"
                        .to_string(),
                ));
            }
            if config
                .providers
                .values()
                .any(|provider| provider.key.is_some())
            {
                return Err(CoreError::Message(
                    "The Server returned an unsafe Provider snapshot containing a plaintext API key; the snapshot was refused"
                        .to_string(),
                ));
            }
            if key_present
                .iter()
                .any(|name| match config.providers.get(name) {
                    Some(provider) => !fleety_tools::providers_config::provider_types()
                        .iter()
                        .find(|kind| kind.name.eq_ignore_ascii_case(&provider.kind))
                        .is_some_and(|kind| kind.allows_key),
                    None => true,
                })
            {
                return Err(CoreError::Message(
                    "The Server returned inconsistent Provider key metadata; the snapshot was refused"
                        .to_string(),
                ));
            }
            Ok(ProviderSnapshot {
                revision,
                entries,
                config,
                key_present,
            })
        }
        Some(ServerMsg::ConfigResult {
            error: Some(error), ..
        })
        | Some(ServerMsg::Error { error }) => Err(issue_as_error(issue_from_wire(error))),
        reply => Err(issue_as_error(unexpected(
            "a Provider configuration snapshot",
            reply.as_ref(),
        ))),
    }
}

pub async fn load_auth_states(
    tx: &mut Tx,
    rx: &mut Rx,
    config_protocol: u32,
    config: &ProvidersConfig,
    now_secs: u64,
) -> BTreeMap<String, AuthState> {
    let mut states = BTreeMap::new();
    for (name, provider) in &config.providers {
        if !provider.is_oauth() {
            states.insert(name.clone(), AuthState::NotApplicable);
            continue;
        }
        if config_protocol < 3 {
            states.insert(
                name.clone(),
                AuthState::Unavailable(ProviderIssue::new(
                    "unsupported",
                    "Server cannot query per-Provider OAuth state",
                    Some("Update the Server"),
                )),
            );
            continue;
        }
        states.insert(name.clone(), AuthState::Checking);
        let result = async {
            crate::send(
                tx,
                &ClientMsg::CredentialStatus {
                    kind: "codex-oauth".to_string(),
                    provider: Some(name.clone()),
                },
            )
            .await?;
            crate::recv(rx).await
        }
        .await;
        let state = match result {
            Ok(Some(ServerMsg::CredentialStatusResult {
                present,
                expires_at_secs,
                error,
                ..
            })) => AuthState::from_observation(
                present,
                expires_at_secs,
                error.map(issue_from_wire),
                now_secs,
            ),
            Ok(reply) => {
                AuthState::Unavailable(unexpected("a Provider credential status", reply.as_ref()))
            }
            Err(error) => AuthState::Unavailable(ProviderIssue::new(
                "transport",
                error.report().message,
                Some("Retry Provider status"),
            )),
        };
        states.insert(name.clone(), state);
    }
    states
}

pub async fn fetch_catalog(
    tx: &mut Tx,
    rx: &mut Rx,
    config_protocol: u32,
    request: &CatalogRequest,
) -> std::result::Result<Vec<String>, ProviderIssue> {
    if config_protocol < 4 {
        return Err(ProviderIssue::new(
            "unsupported",
            "Server does not support Provider model discovery",
            Some("Update the Server"),
        ));
    }
    crate::send(
        tx,
        &ClientMsg::ProviderModelList {
            provider: request.provider.clone(),
        },
    )
    .await
    .map_err(|error| {
        ProviderIssue::new(
            "transport",
            error.report().message,
            Some("Retry the catalog request"),
        )
    })?;
    match crate::recv(rx).await.map_err(|error| {
        ProviderIssue::new(
            "transport",
            error.report().message,
            Some("Retry the catalog request"),
        )
    })? {
        Some(ServerMsg::ProviderModelListResult {
            provider,
            model_ids,
            error,
        }) if provider == request.provider => match error {
            Some(error) => Err(issue_from_wire(error)),
            None if model_ids.is_empty() => Err(ProviderIssue::new(
                "empty_catalog",
                "Server returned no model IDs",
                Some("Retry or enter a model ID"),
            )),
            None => Ok(model_ids),
        },
        reply => Err(unexpected("a Provider model catalog", reply.as_ref())),
    }
}

pub async fn apply_snapshot(
    tx: &mut Tx,
    rx: &mut Rx,
    base_revision: String,
    config: &ProvidersConfig,
    clear_keys: &BTreeSet<String>,
) -> std::result::Result<(), ProviderIssue> {
    let mut providers_value = serde_json::to_value(config).map_err(|error| {
        ProviderIssue::new(
            "serialize",
            format!("Serialize Provider config: {error}"),
            Some("Retry the operation"),
        )
    })?;
    providers_value
        .as_object_mut()
        .ok_or_else(|| {
            ProviderIssue::new(
                "serialize",
                "Serialize Provider config: expected an object",
                Some("Retry the operation"),
            )
        })?
        .insert(
            "clear_keys".to_string(),
            serde_json::to_value(clear_keys).map_err(|error| {
                ProviderIssue::new(
                    "serialize",
                    format!("Serialize Provider key operations: {error}"),
                    Some("Retry the operation"),
                )
            })?,
        );
    let providers_json = serde_json::to_string(&providers_value).map_err(|error| {
        ProviderIssue::new(
            "serialize",
            format!("Serialize Provider config: {error}"),
            Some("Retry the operation"),
        )
    })?;
    crate::send(
        tx,
        &ClientMsg::ConfigApply {
            target: ConfigTarget::Server,
            base_revision,
            changes: Vec::new(),
            providers_json: Some(providers_json),
        },
    )
    .await
    .map_err(|error| {
        ProviderIssue::new(
            "transport",
            error.report().message,
            Some("Reconnect to the Server and retry"),
        )
    })?;
    match crate::recv(rx).await.map_err(|error| {
        ProviderIssue::new(
            "transport",
            error.report().message,
            Some("Reconnect to the Server and retry"),
        )
    })? {
        Some(ServerMsg::ConfigResult { ok: true, .. }) => Ok(()),
        Some(ServerMsg::ConfigResult {
            error: Some(error), ..
        })
        | Some(ServerMsg::Error { error }) => {
            Err(issue_from_wire(redact_wire_provider_secrets(error, config)))
        }
        reply => Err(unexpected(
            "a Provider configuration result",
            reply.as_ref(),
        )),
    }
}

/// Keep a long-lived Provider editor connection active while the user is
/// thinking.  Reading the reply also lets the transport process WebSocket
/// Ping/Pong frames instead of leaving the Server's liveness deadline idle.
pub async fn keepalive(tx: &mut Tx, rx: &mut Rx) -> Result<()> {
    crate::send(tx, &ClientMsg::ServerStatus).await?;
    match crate::recv(rx).await? {
        Some(ServerMsg::ServerStatusResult { .. }) => Ok(()),
        reply => Err(issue_as_error(unexpected(
            "a Server status reply",
            reply.as_ref(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};

    #[test]
    fn overlapping_provider_secrets_are_fully_redacted() {
        let mut config = ProvidersConfig::default();
        for (name, key) in [("a", "secret"), ("b", "secret-more")] {
            config.providers.insert(
                name.into(),
                fleety_tools::providers_config::Provider {
                    kind: "api".into(),
                    base_url: Some("https://api.example.test/v1".into()),
                    key: Some(key.into()),
                },
            );
        }

        let safe = redact_provider_secrets("Server echoed secret-more", &config);

        assert_eq!(safe, "Server echoed <redacted>");
    }

    fn issue(message: &str) -> ProviderIssue {
        ProviderIssue::new("catalog", message, Some("retry"))
    }

    #[test]
    fn oauth_observation_maps_every_visible_state() {
        assert_eq!(
            AuthState::from_observation(true, None, None, 100),
            AuthState::SignedIn
        );
        assert_eq!(
            AuthState::from_observation(false, None, None, 100),
            AuthState::NotSignedIn
        );
        assert_eq!(
            AuthState::from_observation(true, Some(99), None, 100),
            AuthState::Expired
        );
        assert!(matches!(
            AuthState::from_observation(false, None, Some(issue("offline")), 100),
            AuthState::Unavailable(_)
        ));
        assert_eq!(AuthState::Checking.label(), "Checking");
    }

    #[test]
    fn oauth_not_signed_in_blocks_anonymous_catalog_fetch_with_login_action() {
        let gate = catalog_gate("oauth:codex", &AuthState::NotSignedIn, 4);
        let CatalogState::Unavailable(problem) = gate else {
            panic!("catalog should be unavailable before login")
        };
        assert!(problem.message.contains("Not signed in"));
        assert_eq!(
            problem.remediation.as_deref(),
            Some("Run `fleety provider login <provider>`")
        );
    }

    #[test]
    fn retry_preserves_connection_provider_role_and_previous_error() {
        let mut selection =
            ModelSelection::loading("profile-a/server-id", "tingzhen-codex", "main");
        selection.finish(Err(issue("backend unavailable")));
        let request = selection.retry().expect("retry request");
        assert_eq!(request.connection_id, "profile-a/server-id");
        assert_eq!(request.provider, "tingzhen-codex");
        assert_eq!(request.role, "main");
        let CatalogState::Loading { previous_error } = &selection.catalog else {
            panic!("retry should be loading")
        };
        assert_eq!(
            previous_error.as_ref().map(|error| error.message.as_str()),
            Some("backend unavailable")
        );
    }

    #[test]
    fn manual_recovery_keeps_selection_and_failure_details() {
        let mut selection =
            ModelSelection::loading("profile-a/server-id", "tingzhen-codex", "main");
        selection.finish(Err(issue("catalog denied")));
        selection.enter_manual();
        let CatalogState::Manual { previous_error } = &selection.catalog else {
            panic!("manual recovery state")
        };
        assert_eq!(selection.provider, "tingzhen-codex");
        assert_eq!(selection.role, "main");
        assert_eq!(
            previous_error.as_ref().map(|error| error.message.as_str()),
            Some("catalog denied")
        );
    }

    #[tokio::test]
    async fn retry_recording_server_sees_the_same_provider_on_the_same_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_recorded = recorded.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("websocket");
            for attempt in 0..2 {
                let frame = websocket
                    .next()
                    .await
                    .expect("request frame")
                    .expect("request");
                let message: ClientMsg =
                    serde_json::from_str(frame.to_text().expect("text")).expect("client message");
                server_recorded.lock().expect("record lock").push(message);
                let reply = if attempt == 0 {
                    ServerMsg::ProviderModelListResult {
                        provider: "tingzhen-codex".into(),
                        model_ids: Vec::new(),
                        error: Some(WireError {
                            kind: "backend".into(),
                            message: "catalog temporarily unavailable".into(),
                            remediation: Some("Retry".into()),
                        }),
                    }
                } else {
                    ServerMsg::ProviderModelListResult {
                        provider: "tingzhen-codex".into(),
                        model_ids: vec!["gpt-test".into()],
                        error: None,
                    }
                };
                websocket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        serde_json::to_string(&reply).expect("reply json"),
                    ))
                    .await
                    .expect("reply");
            }
        });

        let connection = fleety_tools::transport::connect(&format!("ws://{address}"), None)
            .await
            .expect("connect");
        let (mut tx, mut rx) = connection.split();
        let mut selection = ModelSelection::loading("profile-a/server-a", "tingzhen-codex", "main");
        let first = fetch_catalog(&mut tx, &mut rx, 4, &selection.request()).await;
        selection.finish(first);
        let request = selection.retry().expect("retry request");
        assert_eq!(request.connection_id, "profile-a/server-a");
        assert_eq!(request.role, "main");
        let CatalogState::Loading { previous_error } = &selection.catalog else {
            panic!("retry must be loading")
        };
        assert_eq!(
            previous_error.as_ref().map(|issue| issue.message.as_str()),
            Some("catalog temporarily unavailable")
        );
        let second = fetch_catalog(&mut tx, &mut rx, 4, &request).await;
        selection.finish(second);
        assert_eq!(
            selection.catalog,
            CatalogState::Available(vec!["gpt-test".into()])
        );
        server.await.expect("server task");

        let recorded = recorded.lock().expect("record lock");
        assert!(matches!(
            recorded.as_slice(),
            [
                ClientMsg::ProviderModelList { provider: first },
                ClientMsg::ProviderModelList { provider: second },
            ] if first == "tingzhen-codex" && second == "tingzhen-codex"
        ));
    }
}
