//! `fleety config` — inspect and edit settings from the terminal.
//!
//! Backed by the shared typed registry in `fleety_tools::config`. `list/get`
//! show the resolved value and its source (env / config / default), secrets
//! masked; CLI-owned `set/unset` go through the CLI owner service after validating the key;
//! `open` is routed by the CLI into the shared staged Settings workspace. Read
//! precedence stays env → config → default, so an explicit env var always wins.

use std::path::Path;

use agent_core::{CoreError, Result};
use fleety_protocol::ConfigTarget;
#[cfg(test)]
use fleety_protocol::{ClientMsg, ServerMsg};
use fleety_tools::config::{self, ConfigMap, Owner};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Auto,
    Server,
    Daemon,
    Cli,
    Device(String),
}

/// Split a leading-or-embedded `--target <server|local|<device-id>>` out of the
/// config args, returning the target (default `Server`) and the remaining args.
/// Pure. `local` is handled by this CLI; `server`/`device` go over the wire.
pub fn split_target(args: &[String]) -> Result<(Target, Vec<String>)> {
    let mut target = Target::Auto;
    let mut seen = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--target" {
            if seen {
                return Err(CoreError::Message(
                    "--target may be specified only once".to_string(),
                ));
            }
            let v = args.get(i + 1).ok_or_else(|| {
                CoreError::Message(
                    "--target needs server, daemon, cli, local, or a device id".to_string(),
                )
            })?;
            target = match v.as_str() {
                "server" => Target::Server,
                "daemon" => Target::Daemon,
                "cli" | "local" => Target::Cli,
                other if other.starts_with('-') => {
                    return Err(CoreError::Message(format!(
                        "invalid config target '{other}'"
                    )))
                }
                other => Target::Device(other.to_string()),
            };
            seen = true;
            i += 2;
            continue;
        }
        rest.push(args[i].clone());
        i += 1;
    }
    Ok((target, rest))
}

fn command_owner(args: &[String]) -> Result<Option<Owner>> {
    match args.first().map(String::as_str) {
        Some("provider" | "model") => Ok(Some(Owner::Server)),
        Some("get" | "set" | "unset") => {
            let key = args.get(1).ok_or_else(|| {
                CoreError::Message(format!(
                    "config {} needs a setting key",
                    args.first().map(String::as_str).unwrap_or_default()
                ))
            })?;
            config::owner_for_key(key).map(Some)
        }
        Some("list") | None | Some("open" | "edit") => Ok(None),
        Some(other) => Err(CoreError::Message(format!(
            "unknown config command '{other}'"
        ))),
    }
}

/// Resolve automatic ownership and reject explicit target/owner mismatches
/// before any file or network I/O.
pub fn resolve_target(target: Target, args: &[String], device_id: &str) -> Result<Target> {
    let owner = command_owner(args)?;
    let resolved = match target {
        Target::Auto => match owner {
            Some(Owner::Server) => Target::Server,
            Some(Owner::Daemon) => Target::Device(device_id.to_string()),
            Some(Owner::Cli) => Target::Cli,
            None => Target::Auto,
        },
        Target::Daemon => Target::Device(device_id.to_string()),
        other => other,
    };
    if let Some(owner) = owner {
        let matches = matches!(
            (&resolved, owner),
            (Target::Server, Owner::Server)
                | (Target::Cli, Owner::Cli)
                | (Target::Daemon | Target::Device(_), Owner::Daemon)
        );
        if !matches {
            let owner_name = match owner {
                Owner::Server => "server",
                Owner::Daemon => "daemon",
                Owner::Cli => "cli",
            };
            return Err(CoreError::Message(format!(
                "this setting is owned by {owner_name}; choose `--owner {owner_name}`"
            )));
        }
    }
    Ok(resolved)
}

pub fn wire_target(target: &Target) -> Result<ConfigTarget> {
    match target {
        Target::Server => Ok(ConfigTarget::Server),
        Target::Daemon => Err(CoreError::Message(
            "daemon target must be resolved to a device id".to_string(),
        )),
        Target::Device(id) => Ok(ConfigTarget::Device(id.clone())),
        Target::Cli => Ok(ConfigTarget::Local),
        Target::Auto => Err(CoreError::Message(
            "config list/edit needs an explicit owner or the interactive panel".to_string(),
        )),
    }
}

/// Whether `args` is exactly the interactive provider editor invocation.
fn is_provider_edit(args: &[String]) -> bool {
    matches!(
        (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str)
        ),
        (Some("provider"), Some("edit"))
    )
}

/// Pure routing: `provider edit` is available only against its server owner.
pub fn is_remote_provider_edit(args: &[String], target: &Target) -> bool {
    is_provider_edit(args) && matches!(target, Target::Server)
}

/// The version gate for remote Provider editing. Config protocol 5 makes API
/// keys write-only and merges omitted keys as Keep on the Server. Refuse older
/// Servers before requesting a snapshot so a legacy plaintext key never enters
/// the CLI process.
fn provider_edit_support_err(config_protocol: u32) -> Option<CoreError> {
    (config_protocol < 5).then(|| {
        CoreError::Message(
            "the connected server is too old for remote provider editing — update it first \
             (run `fleety update` on the server host); provider configuration is server-owned \
             and is never written directly by the CLI"
                .to_string(),
        )
    })
}

/// Interactive provider editing against the connected server: snapshot the
/// server's providers, edit them in memory, and apply the result under the
/// snapshot's optimistic-lock revision. A concurrent-edit conflict closes the
/// editor and reloads from a fresh snapshot instead of overwriting.
pub async fn provider_edit_remote() -> Result<()> {
    let mut input = crate::workspace::WorkspaceInput::terminal();
    let (tx, rx, config_protocol, target, server_fingerprint) =
        crate::connect_hello_for_auth_transaction()
            .await
            .map_err(provider_editor_connect_error)?;
    provider_edit_remote_loop(
        tx,
        rx,
        config_protocol,
        target,
        server_fingerprint,
        None,
        &mut input,
    )
    .await
}

pub(crate) async fn provider_edit_remote_on_target(
    target: &fleety_tools::connection::Resolved,
    expected_fingerprint: Option<&str>,
    terminal: &mut ratatui::DefaultTerminal,
    input: &mut crate::workspace::WorkspaceInput,
) -> Result<()> {
    let (tx, rx, config_protocol, fingerprint, committed_target) =
        crate::connect_hello_for_auth_target_refreshed(target)
            .await
            .map_err(provider_editor_connect_error)?;
    crate::provider_service::validate_server_identity(
        expected_fingerprint,
        fingerprint.as_deref(),
        "Provider editor launch",
    )
    .map_err(crate::provider_service::issue_as_error)?;
    provider_edit_remote_loop(
        tx,
        rx,
        config_protocol,
        committed_target,
        fingerprint,
        Some(terminal),
        input,
    )
    .await
}

async fn provider_edit_remote_loop(
    tx: fleety_tools::transport::Sender,
    rx: fleety_tools::transport::Receiver,
    config_protocol: u32,
    target: fleety_tools::connection::Resolved,
    server_fingerprint: Option<String>,
    terminal: Option<&mut ratatui::DefaultTerminal>,
    input: &mut crate::workspace::WorkspaceInput,
) -> Result<()> {
    let mut terminal = terminal;
    let mut initial_connection = Some((tx, rx, config_protocol));
    loop {
        let (mut tx, mut rx, config_protocol) = match initial_connection.take() {
            Some(connection) => connection,
            None => {
                let (tx, rx, protocol, fingerprint) = crate::connect_hello_for_auth_target(&target)
                    .await
                    .map_err(provider_editor_connect_error)?;
                crate::provider_service::validate_server_identity(
                    server_fingerprint.as_deref(),
                    fingerprint.as_deref(),
                    "Provider editor reconnect",
                )
                .map_err(crate::provider_service::issue_as_error)?;
                (tx, rx, protocol)
            }
        };
        if let Some(err) = provider_edit_support_err(config_protocol) {
            return Err(err);
        }
        let snapshot =
            crate::provider_service::load_snapshot(&mut tx, &mut rx, config_protocol).await?;
        let (mut revision, editor_input) = provider_editor_input(snapshot);

        // Credential status is server-owned. A status failure is deliberately
        // non-fatal so the editor never turns an unavailable query into a false
        // "not signed in" claim or blocks unrelated config edits.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let auth_states = crate::provider_service::load_auth_states(
            &mut tx,
            &mut rx,
            config_protocol,
            &editor_input.config,
            now_secs,
        )
        .await;
        let connection_id = target.url_owned();

        // The editor loop is synchronous (crossterm events); each save runs the
        // async apply on the runtime from inside it.
        let handle = tokio::runtime::Handle::current();
        let io = std::rc::Rc::new(tokio::sync::Mutex::new((&mut tx, &mut rx)));
        let catalog_handle = handle.clone();
        let catalog_target = target.clone();
        let catalog_connection_id = connection_id.clone();
        let catalog_server_fingerprint = server_fingerprint.clone();
        let save = |edited: &fleety_tools::providers_config::ProvidersConfig,
                    clear_keys: &std::collections::BTreeSet<String>| {
            handle.block_on(async {
                let mut io = io.lock().await;
                let (apply_tx, apply_rx) = &mut *io;
                match crate::provider_service::apply_snapshot(
                    apply_tx,
                    apply_rx,
                    revision.clone(),
                    edited,
                    clear_keys,
                )
                .await
                {
                    Ok(()) => {
                        // Our own write moved the server's revision; refresh
                        // it so the next save in this session doesn't
                        // conflict with our own edit.
                        Ok(provider_refresh_outcome(
                            &mut revision,
                            refresh_provider_revision(apply_tx, apply_rx, config_protocol).await,
                        ))
                    }
                    Err(issue) if issue.kind == "conflict" => {
                        Ok(crate::provider_tui::SaveOutcome::Conflict(issue.message))
                    }
                    Err(issue) => Err(crate::provider_service::issue_as_error(issue)),
                }
            })
        };
        let fetch =
            move |request: &crate::provider_service::CatalogRequest,
                  cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>| {
                if request.connection_id != catalog_connection_id {
                    return Err(crate::provider_service::ProviderIssue::new(
                        "target_changed",
                        "Provider catalog request no longer matches this Server",
                        Some("Reopen the Provider editor"),
                    ));
                }
                catalog_handle.block_on(async {
                    tokio::select! {
                        result = async {
                            let (
                                mut catalog_tx,
                                mut catalog_rx,
                                catalog_protocol,
                                catalog_fingerprint,
                            ) = crate::connect_hello_for_auth_target(&catalog_target)
                                .await
                                .map_err(|error| {
                                    crate::provider_service::ProviderIssue::new(
                                        "transport",
                                        error.report().message,
                                        Some("Reconnect to this Server and retry"),
                                    )
                                })?;
                            crate::provider_service::validate_server_identity(
                                catalog_server_fingerprint.as_deref(),
                                catalog_fingerprint.as_deref(),
                                "Provider catalog fetch",
                            )?;
                            crate::provider_service::fetch_catalog(
                                &mut catalog_tx,
                                &mut catalog_rx,
                                catalog_protocol,
                                request,
                            )
                            .await
                        } => result,
                        () = wait_for_catalog_cancel(cancellation) => {
                            Err(crate::provider_service::ProviderIssue::new(
                                "cancelled",
                                "Provider catalog request was cancelled",
                                None::<String>,
                            ))
                        }
                    }
                })
            };
        let outcome = tokio::task::block_in_place(|| {
            if let Some(terminal) = terminal.as_deref_mut() {
                crate::provider_tui::run_with_terminal(
                    terminal,
                    editor_input,
                    save,
                    fetch,
                    crate::provider_tui::ProviderEditorContext {
                        auth_states,
                        connection_id: connection_id.clone(),
                        config_protocol,
                    },
                    input,
                )
            } else {
                crate::provider_tui::run_with_saver_and_fetcher(
                    editor_input,
                    save,
                    fetch,
                    auth_states,
                    connection_id.clone(),
                    config_protocol,
                    input,
                )
            }
        })?;
        // An OAuth action the editor asked for: the just-added/edited provider is
        // already applied to the server (the save above ran the ConfigApply).
        // Embedded Settings temporarily restores its caller-owned terminal so
        // the browser flow can use the plain terminal, then reopens the editor
        // on a fresh snapshot. Standalone mode already owns this transition in
        // the wrapper around the editor.
        if let Some(req) = outcome.auth_request {
            let embedded = terminal.is_some();
            if embedded {
                ratatui::restore();
            }
            crate::config_panel::run_auth_action_on_target(
                &req,
                &target,
                server_fingerprint.as_deref(),
                input,
            )
            .await;
            if embedded {
                if let Some(terminal) = terminal.as_deref_mut() {
                    *terminal = ratatui::init();
                }
            }
            continue;
        }
        // A concurrent-edit conflict: reload from a fresh snapshot and reopen.
        match outcome.conflict {
            None => return Ok(()),
            Some(msg) => {
                println!(
                    "{} — reloading the current server configuration…",
                    crate::terminal_safe_text(&msg)
                );
                continue;
            }
        }
    }
}

fn provider_editor_input(
    snapshot: crate::provider_service::ProviderSnapshot,
) -> (String, crate::provider_tui::ProviderEditorInput) {
    (
        snapshot.revision,
        crate::provider_tui::ProviderEditorInput {
            config: snapshot.config,
            key_present: snapshot.key_present,
        },
    )
}

async fn refresh_provider_revision(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
    config_protocol: u32,
) -> Result<String> {
    Ok(
        crate::provider_service::load_snapshot(tx, rx, config_protocol)
            .await?
            .revision,
    )
}

fn provider_refresh_outcome(
    revision: &mut String,
    refresh: Result<String>,
) -> crate::provider_tui::SaveOutcome {
    match refresh {
        Ok(fresh_revision) => {
            *revision = fresh_revision;
            crate::provider_tui::SaveOutcome::Saved
        }
        Err(error) => {
            crate::provider_tui::SaveOutcome::SavedRefreshRequired(error.report().message)
        }
    }
}

async fn wait_for_catalog_cancel(cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;

    while !cancellation.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn provider_editor_connect_error(error: CoreError) -> CoreError {
    CoreError::Message(format!(
        "could not reach the Server whose Providers this would edit: {} — pair this device first \
         (`fleety pair <code>`) or select it with `fleety connection use <name>`; Provider \
         configuration is never written locally by the CLI",
        error.report().message
    ))
}
/// Run a non-interactive CLI-owned `config` subcommand. `open`/legacy `edit`
/// are intercepted by `main` and routed into the shared Settings workspace.
pub fn run(args: &[String]) -> Result<()> {
    if matches!(args.first().map(String::as_str), Some("provider" | "model")) {
        return Err(CoreError::Message(
            "provider and model configuration is owned by the server; use `fleety config \
             --owner server ...` (the CLI never edits providers.toml directly)"
                .to_string(),
        ));
    }
    // The CLI's config path is always the local target (main.rs routes remote
    // operations and interactive Settings), so restrict it to CLI scopes.
    config::run_scoped(args, Some(config::LOCAL_SCOPES))
}

/// Persist a complete staged CLI-owner snapshot without replacing settings
/// owned by another runtime. All interactive CLI-owner applies pass through
/// this boundary instead of calling the file serializer directly.
pub(crate) fn apply_cli_owner(path: &Path, map: &ConfigMap) -> Result<()> {
    config::replace_scopes_strict(path, config::CLI_SCOPES, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum RefreshFailureReply {
        Error,
        Close,
        WrongReply,
    }

    async fn provider_refresh_failure(
        reply: RefreshFailureReply,
    ) -> crate::provider_tui::SaveOutcome {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider refresh server");
        let address = listener.local_addr().expect("provider refresh address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept refresh client");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept refresh websocket");
            let frame = websocket
                .next()
                .await
                .expect("refresh frame")
                .expect("read refresh frame");
            let request: ClientMsg =
                serde_json::from_str(frame.to_text().expect("refresh request is text"))
                    .expect("parse refresh request");
            assert!(matches!(
                request,
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Server
                }
            ));
            match reply {
                RefreshFailureReply::Error => websocket
                    .send(Message::Text(
                        serde_json::to_string(&ServerMsg::Error {
                            error: fleety_protocol::WireError {
                                kind: "snapshot_failed".into(),
                                message: "fresh snapshot unavailable".into(),
                                remediation: None,
                            },
                        })
                        .expect("serialize snapshot error"),
                    ))
                    .await
                    .expect("send snapshot error"),
                RefreshFailureReply::WrongReply => websocket
                    .send(Message::Text(
                        serde_json::to_string(&ServerMsg::ConfigResult {
                            ok: true,
                            output: "not a snapshot".into(),
                            effect: None,
                            error: None,
                        })
                        .expect("serialize wrong reply"),
                    ))
                    .await
                    .expect("send wrong reply"),
                RefreshFailureReply::Close => {}
            }
        });
        let connection = fleety_tools::transport::connect(&format!("ws://{address}"), None)
            .await
            .expect("connect provider refresh client");
        let (mut tx, mut rx) = connection.split();
        let refresh = refresh_provider_revision(&mut tx, &mut rx, 5).await;
        server.await.expect("provider refresh server task");
        let mut stale_revision = "stale-r1".to_string();
        let outcome = provider_refresh_outcome(&mut stale_revision, refresh);
        assert_eq!(
            stale_revision, "stale-r1",
            "a failed refresh cannot replace the revision"
        );
        outcome
    }

    fn refresh_required_reason(outcome: crate::provider_tui::SaveOutcome) -> String {
        match outcome {
            crate::provider_tui::SaveOutcome::SavedRefreshRequired(reason) => reason,
            _ => panic!("refresh failure must lock the Provider editor"),
        }
    }

    #[tokio::test]
    async fn provider_refresh_error_requires_reopen() {
        let error =
            refresh_required_reason(provider_refresh_failure(RefreshFailureReply::Error).await);
        assert!(error.contains("fresh snapshot unavailable"), "{error}");
    }

    #[tokio::test]
    async fn provider_refresh_close_requires_reopen() {
        let error =
            refresh_required_reason(provider_refresh_failure(RefreshFailureReply::Close).await);
        assert!(
            error.contains("closed") || error.contains("ended"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn provider_refresh_wrong_reply_requires_reopen() {
        let error = refresh_required_reason(
            provider_refresh_failure(RefreshFailureReply::WrongReply).await,
        );
        assert!(
            error
                .to_ascii_lowercase()
                .contains("expected a provider configuration snapshot"),
            "{error}"
        );
    }

    #[test]
    fn successful_provider_refresh_replaces_the_revision_and_remains_editable() {
        let mut revision = "stale-r1".to_string();
        let outcome = provider_refresh_outcome(&mut revision, Ok("fresh-r2".into()));
        assert!(matches!(outcome, crate::provider_tui::SaveOutcome::Saved));
        assert_eq!(revision, "fresh-r2");
    }

    #[test]
    fn provider_editor_input_preserves_snapshot_key_presence() {
        let mut config = fleety_tools::providers_config::ProvidersConfig::default();
        config.providers.insert(
            "openai".into(),
            fleety_tools::providers_config::Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        let snapshot = crate::provider_service::ProviderSnapshot {
            revision: "r1".into(),
            entries: Vec::new(),
            config,
            key_present: std::collections::BTreeSet::from(["openai".to_string()]),
        };

        let (revision, input) = provider_editor_input(snapshot);

        assert_eq!(revision, "r1");
        assert!(input.key_present.contains("openai"));
        assert!(input
            .config
            .provider("openai")
            .expect("openai")
            .key
            .is_none());
    }

    #[test]
    fn split_target_extracts_and_defaults() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Default is automatic ownership, args untouched.
        let (t, rest) = split_target(&s(&["set", "FLEETY_MODEL", "gpt-5"])).unwrap();
        assert_eq!(t, Target::Auto);
        assert_eq!(rest, s(&["set", "FLEETY_MODEL", "gpt-5"]));
        // --target local is stripped.
        let (t, rest) = split_target(&s(&["--target", "local", "list"])).unwrap();
        assert_eq!(t, Target::Cli);
        assert_eq!(rest, s(&["list"]));
        // A non-server/local value is a device id; stripped from the middle.
        let (t, rest) = split_target(&s(&["provider", "--target", "pi", "list"])).unwrap();
        assert_eq!(t, Target::Device("pi".into()));
        assert_eq!(rest, s(&["provider", "list"]));
    }

    #[test]
    fn provider_edit_routes_by_target_and_gates_by_version() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Server target uses the remote flow. CLI/local is not a provider owner,
        // so other dispatch code rejects it before any file access.
        assert!(is_remote_provider_edit(
            &s(&["provider", "edit"]),
            &Target::Server
        ));
        assert!(!is_remote_provider_edit(
            &s(&["provider", "edit"]),
            &Target::Cli
        ));
        assert!(!is_remote_provider_edit(
            &s(&["provider", "list"]),
            &Target::Server
        ));

        // Version gate: only protocol 5 guarantees write-only Provider keys and
        // Server-side Keep merging, so older snapshots must not be requested.
        let msg = provider_edit_support_err(1)
            .expect("old server refused")
            .to_string();
        assert!(msg.contains("update"), "gate names the remedy: {msg}");
        assert!(msg.contains("server-owned"), "gate names ownership: {msg}");
        assert!(provider_edit_support_err(0).is_some());
        assert!(provider_edit_support_err(2).is_some());
        assert!(
            provider_edit_support_err(4).is_some(),
            "protocol 4 still returned plaintext Provider keys"
        );
        assert!(provider_edit_support_err(5).is_none());
    }

    #[test]
    fn catalog_reconnect_requires_the_original_server_identity() {
        let validate = |expected, actual| {
            crate::provider_service::validate_server_identity(expected, actual, "catalog")
        };
        assert!(validate(Some("server-a"), Some("server-a")).is_ok());

        let changed = validate(Some("server-a"), Some("server-b"))
            .expect_err("different Server must be rejected");
        assert_eq!(changed.kind, "server_identity_changed");

        let missing = validate(None, Some("server-a"))
            .expect_err("unverifiable original Server must be rejected");
        assert_eq!(missing.kind, "server_identity_unavailable");

        let disappeared = validate(Some("server-a"), None)
            .expect_err("missing reconnect identity must be rejected");
        assert_eq!(disappeared.kind, "server_identity_changed");
    }

    #[tokio::test]
    async fn catalog_cancel_wait_finishes_after_the_signal() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let cancellation = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&cancellation);
        let waiter = tokio::spawn(wait_for_catalog_cancel(cancellation));
        signal.store(true, Ordering::Release);

        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("cancellation wait deadline")
            .expect("cancellation wait task");
    }

    #[test]
    fn owner_route_matrix_and_target_mismatches() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            resolve_target(Target::Auto, &s(&["set", "FLEETY_ADDR", "x"]), "dev").unwrap(),
            Target::Server
        );
        assert_eq!(
            resolve_target(Target::Auto, &s(&["set", "FLEETY_TZ", "UTC"]), "dev").unwrap(),
            Target::Device("dev".into())
        );
        assert_eq!(
            resolve_target(Target::Daemon, &s(&["list"]), "dev").unwrap(),
            Target::Device("dev".into())
        );
        assert_eq!(
            resolve_target(
                Target::Auto,
                &s(&["set", "FLEETY_VOICE_AUDIO", "auto"]),
                "dev"
            )
            .unwrap(),
            Target::Cli
        );
        assert!(resolve_target(Target::Server, &s(&["set", "FLEETY_TZ", "UTC"]), "dev").is_err());
        assert!(resolve_target(Target::Cli, &s(&["provider", "edit"]), "dev").is_err());
        assert!(split_target(&s(&["--target"])).is_err());
    }
}
