//! `fleety auth` — sign in to ChatGPT (Codex OAuth) for the **connected
//! server**: the PKCE browser flow runs here (the authorization page must open
//! in front of the user), but the exchanged tokens are delivered over the
//! authenticated connection and stored on the server — the machine whose
//! provider (`auth = oauth:codex`) actually calls the model. No token is
//! persisted on the CLI host, and this command never prints token values.

use std::io::{Read, Write};
use std::net::TcpListener;

use agent_core::{CoreError, Result};
use fleety_protocol::{ClientMsg, ServerMsg};
use fleety_tools::providers_config::ProvidersConfig;
use fleety_tools::transport::{Receiver as Rx, Sender as Tx};
use fleety_tools::{connection, oauth};

use crate::{
    connect_hello_for_auth, connect_hello_for_auth_target, connect_hello_for_auth_transaction,
    recv, send,
};

/// The credential kind this command manages on the server.
const CREDENTIAL_KIND: &str = "codex-oauth";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Dispatch `fleety auth <sub>`. Codex credentials are per provider, so `login`
/// and `logout` take the provider name; `status` takes an optional one (no name
/// lists every `oauth:codex` provider).
pub async fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("login") => {
            let mut provider = None;
            let mut no_browser = false;
            for arg in &args[1..] {
                if arg == "--no-browser" && !no_browser {
                    no_browser = true;
                } else if arg.starts_with('-') || provider.is_some() {
                    return Err(CoreError::Message(format!(
                        "unexpected auth login argument '{arg}'. Usage: fleety auth login <provider> [--no-browser]"
                    )));
                } else {
                    provider = Some(arg.clone());
                }
            }
            let provider = provider.ok_or_else(|| usage_error("login"))?;
            login(&provider, no_browser).await
        }
        Some("logout") if args.len() == 2 && !args[1].starts_with('-') => logout(&args[1]).await,
        Some("logout") => Err(usage_error("logout")),
        Some("status")
            if args.len() <= 2 && args.get(1).map_or(true, |a| !a.starts_with('-')) =>
        {
            status(args.get(1).cloned()).await
        }
        Some("help" | "--help" | "-h") if args.len() == 1 => {
            println!(
                "usage: fleety auth <login <provider> | logout <provider> | status [<provider>]> \
                 [--no-browser]"
            );
            Ok(())
        }
        None => Err(CoreError::Message(
            "usage: fleety auth <login <provider> | logout <provider> | status [<provider>]> [--no-browser]"
                .to_string(),
        )),
        Some(other) => Err(CoreError::Message(format!(
            "unknown auth command '{other}'. Usage: fleety auth <login|logout|status>"
        ))),
    }
}

/// A missing-provider usage error naming an example. Pure.
fn usage_error(sub: &str) -> CoreError {
    CoreError::Message(format!(
        "`fleety auth {sub}` needs a provider name (an oauth:codex provider), e.g. \
         `fleety auth {sub} my-codex`. List them with `fleety auth status`."
    ))
}

/// Validate that `provider` names an `oauth:codex` provider in the connected
/// server's config, erroring by name otherwise. Pure.
fn validate_codex_provider(cfg: &ProvidersConfig, provider: &str) -> Result<()> {
    match cfg.providers.get(provider) {
        Some(p) if p.kind.eq_ignore_ascii_case("oauth:codex") => Ok(()),
        Some(p) => Err(CoreError::Message(format!(
            "provider '{provider}' is type '{}', not oauth:codex — `fleety auth` signs in only \
             Codex providers",
            p.kind
        ))),
        None => Err(CoreError::Message(format!(
            "no such provider '{provider}' on the server — add it first with `fleety config` \
             (Providers, type oauth:codex)"
        ))),
    }
}

/// The `oauth:codex` provider names in the config, in display order. Pure.
fn codex_provider_names(cfg: &ProvidersConfig) -> Vec<String> {
    cfg.providers
        .iter()
        .filter(|(_, p)| p.kind.eq_ignore_ascii_case("oauth:codex"))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Pull the connected server's provider config (to validate a provider name and
/// enumerate the `oauth:codex` ones). Reuses the same `ConfigSnapshot` the config
/// panel uses.
async fn fetch_providers(
    tx: &mut Tx,
    rx: &mut Rx,
    config_protocol: u32,
) -> Result<ProvidersConfig> {
    Ok(
        crate::provider_service::load_snapshot(tx, rx, config_protocol)
            .await?
            .config,
    )
}

/// The version gate: protocol 5 adds write-only Provider snapshots. Refuse an
/// older Server before any Provider snapshot or browser flow can expose a key.
fn credential_support_err(config_protocol: u32) -> Option<CoreError> {
    (config_protocol < 5).then(|| {
        CoreError::Message(
            "the connected server is too old to store per-provider Codex credentials — update it \
             first (run `fleety update` on the server host, or let fleet convergence catch it up), \
             then re-run `fleety auth login <provider>`"
                .to_string(),
        )
    })
}

/// Human-readable name of the server a credential operation acted on.
fn server_label(target: &connection::Resolved) -> String {
    match &target.source {
        connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => {
            format!(
                "'{}' ({})",
                crate::terminal_safe_text(name),
                crate::terminal_safe_endpoint(&target.url)
            )
        }
        _ => crate::terminal_safe_endpoint(&target.url),
    }
}

/// The status line for the server-side credential — presence and expiry only
/// (by construction this function never sees a token value).
fn remote_status_line(
    present: bool,
    expires_at_secs: Option<u64>,
    detail: Option<&str>,
    server: &str,
) -> String {
    let state = fleety_tools::provider_service::AuthState::from_observation(
        present,
        expires_at_secs,
        None,
        now_secs(),
    );
    if matches!(
        state,
        fleety_tools::provider_service::AuthState::NotSignedIn
    ) {
        return format!(
            "Not signed in on Server {server}. Run `fleety provider login <provider>`."
        );
    }
    if matches!(state, fleety_tools::provider_service::AuthState::Expired) {
        return format!(
            "Login expired on Server {server}. Run `fleety provider login <provider>` again."
        );
    }
    let expiry = expires_at_secs
        .map(|s| format!("expires at unix {s}"))
        .unwrap_or_else(|| "no expiry recorded".to_string());
    let detail = detail
        .map(|d| format!(" ({})", crate::terminal_safe_text(d)))
        .unwrap_or_default();
    format!(
        "Signed in to ChatGPT (Codex OAuth) on Server {server}{detail}. Access token: {expiry}."
    )
}

/// Remove a leftover token file from the pre-server-side era on this CLI host.
/// Returns the note to print when one was cleaned up.
fn cleanup_legacy_local_file(path: &std::path::Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let clear_result = oauth::clear_tokens(path);
    let path = terminal_safe_path(path);
    let note = match clear_result {
        Ok(()) => format!(
            "Removed the leftover local token file at {} — credentials now live on the server.",
            path
        ),
        Err(e) => format!(
            "A leftover local token file at {} is no longer used, but could not be removed: {}",
            path,
            crate::terminal_safe_text(&e.report().message)
        ),
    };
    Some(note)
}

/// The note `auth status` prints when a stale local token file still exists.
fn legacy_local_note(path: &std::path::Path) -> Option<String> {
    path.exists().then(|| {
        let path = terminal_safe_path(path);
        format!(
            "Note: the local token file at {} is no longer read by any flow; re-run \
             `fleety auth login` to store credentials on the server (login also cleans it up).",
            path
        )
    })
}

fn terminal_safe_path(path: &std::path::Path) -> String {
    crate::terminal_safe_text(&path.display().to_string())
}

/// Map a `CredentialResult` reply to a `Result`, surfacing the server's error.
fn credential_result(reply: Option<ServerMsg>) -> Result<()> {
    match reply {
        Some(ServerMsg::CredentialResult { ok: true, .. }) => Ok(()),
        Some(ServerMsg::CredentialResult { error: Some(e), .. }) => {
            Err(CoreError::Message(match e.remediation {
                Some(r) => format!("{} — {r}", e.message),
                None => e.message,
            }))
        }
        Some(ServerMsg::CredentialResult { .. }) => Err(CoreError::Message(
            "the server refused the credential operation without a reason".to_string(),
        )),
        other => Err(CoreError::Provider(format!(
            "expected a credential reply, got {other:?}"
        ))),
    }
}

/// Run the PKCE authorization-code login: open the browser, capture the code on
/// a loopback listener, exchange it, and deliver the tokens to the connected
/// server for storage (nothing is persisted on this host).
pub async fn login(provider: &str, no_browser: bool) -> Result<()> {
    login_with_target(provider, no_browser, None).await
}

/// Run login against an already-resolved Server. Interactive settings uses
/// this so a profile change in another process cannot redirect credentials
/// between the snapshot/save and OAuth steps.
pub(crate) async fn login_on_target(
    provider: &str,
    no_browser: bool,
    target: &connection::Resolved,
    expected_fingerprint: Option<&str>,
) -> Result<()> {
    login_with_target(
        provider,
        no_browser,
        Some((target.clone(), expected_fingerprint.map(ToOwned::to_owned))),
    )
    .await
}

async fn login_with_target(
    provider: &str,
    no_browser: bool,
    fixed_target: Option<(connection::Resolved, Option<String>)>,
) -> Result<()> {
    let config = oauth::oauth_config();
    if config.client_id.is_empty() {
        return Err(CoreError::Message(
            "Codex OAuth client id is not configured. Set it with \
             `fleety config set FLEETY_CODEX_CLIENT_ID <id>` (the Codex CLI public client id), \
             then re-run `fleety auth login`."
                .into(),
        ));
    }

    // Gate on the server BEFORE the browser opens: an unreachable, unpaired, or
    // too-old server must not cost the user a full authorization round-trip.
    // This probe connection is then dropped — the authorization can take minutes
    // and an idle link would trip the keepalive; delivery reconnects afresh.
    let (target, preflight_fingerprint) = {
        let (mut tx, mut rx, config_protocol, target, fingerprint) = if let Some((
            target,
            expected_fingerprint,
        )) = fixed_target
        {
            let (tx, rx, protocol, fingerprint) = connect_hello_for_auth_target(&target)
                .await
                .map_err(|e| {
                    CoreError::Message(format!(
                        "could not reach the Server that owns this Provider login: {} — retry without switching the selected profile",
                        e.report().message
                    ))
                })?;
            crate::provider_service::validate_server_identity(
                expected_fingerprint.as_deref(),
                fingerprint.as_deref(),
                "Provider login",
            )
            .map_err(crate::provider_service::issue_as_error)?;
            (tx, rx, protocol, target, fingerprint)
        } else {
            connect_hello_for_auth_transaction().await.map_err(|e| {
                CoreError::Message(format!(
                    "could not reach the server that would store this login: {} — check the \
                     connection (`fleety status`), pair this device first (`fleety pair <code>`), \
                     or set the server URL with `fleety init <ws-url>`",
                    e.report().message
                ))
            })?
        };
        if let Some(err) = credential_support_err(config_protocol) {
            return Err(err);
        }
        // Validate the provider before the browser opens: it must be an
        // oauth:codex provider on this server.
        let providers = fetch_providers(&mut tx, &mut rx, config_protocol).await?;
        validate_codex_provider(&providers, provider)?;
        let fingerprint = fingerprint.ok_or_else(|| {
            CoreError::Message(
                "the server does not advertise a stable identity fingerprint; update it before OAuth login so credentials cannot be delivered to a different server"
                    .to_string(),
            )
        })?;
        (target, fingerprint)
    };

    let verifier = oauth::generate_verifier();
    let challenge = oauth::challenge_for(&verifier);
    let state = oauth::generate_state();

    // The Codex client id is registered with a fixed redirect URI, so the
    // loopback port and path are not free — they must match exactly. Bind the
    // listener *before* opening the browser: if the fixed port is busy, fail fast
    // with an actionable message instead of sending the user through the whole
    // authorization only for the redirect to land on a dead port. This happens
    // before any token is read or written, so a busy port never touches the
    // stored tokens.
    let port = oauth::CODEX_LOOPBACK_PORT;
    let listener = bind_loopback(port)?;
    let redirect_uri = format!("http://localhost:{port}{}", oauth::CODEX_CALLBACK_PATH);

    let url = oauth::authorize_url(&config, &redirect_uri, &challenge, &state);
    present_authorization(&url, no_browser)?;

    let code = wait_for_code(&listener, &state)?;

    let client = reqwest::Client::new();
    let tokens = oauth::exchange_code(
        &client,
        &config,
        &code,
        &verifier,
        &redirect_uri,
        now_secs(),
    )
    .await?;

    // Deliver to the server that owns the credential from here on. A failure
    // fails the whole login — the tokens are dropped, never stored locally
    // (re-running login is cheap; a silent local fallback would recreate the
    // very split-brain this flow removes).
    let payload_json = serde_json::to_string(&tokens)
        .map_err(|e| CoreError::Message(format!("serialize tokens: {e}")))?;
    let (mut tx, mut rx, config_protocol, delivery_fingerprint) =
        connect_hello_for_auth_target(&target).await.map_err(|e| {
            CoreError::Message(format!(
                "authorization succeeded, but the server could not be reached to store it: {} — \
             re-run `fleety auth login` once the connection is back",
                e.report().message
            ))
        })?;
    if let Some(err) = credential_support_err(config_protocol) {
        return Err(err);
    }
    if delivery_fingerprint.as_deref() != Some(preflight_fingerprint.as_str()) {
        return Err(CoreError::Message(
            "the server identity changed during OAuth login; credential delivery was refused. Verify the selected server and re-run login"
                .to_string(),
        ));
    }
    send(
        &mut tx,
        &ClientMsg::CredentialPut {
            kind: CREDENTIAL_KIND.to_string(),
            provider: Some(provider.to_string()),
            payload_json,
        },
    )
    .await?;
    credential_result(recv(&mut rx).await?)?;

    if let Err(e) = oauth::append_auth_audit("login", now_secs()) {
        tracing::warn!(report = ?e.report(), "could not record auth audit");
    }
    println!(
        "Signed in provider '{}'. Credentials delivered to server {}.",
        crate::terminal_safe_text(provider),
        server_label(&target)
    );
    if let Some(note) = cleanup_legacy_local_file(&oauth::default_token_path()) {
        println!("{note}");
    }
    Ok(())
}

/// Report sign-in state and expiry per provider — never token values (the status
/// frame carries none by shape). With a provider name, reports just that one;
/// with none, lists every `oauth:codex` provider on the server.
pub async fn status(provider: Option<String>) -> Result<()> {
    let (mut tx, mut rx, config_protocol, target) = connect_hello_for_auth().await?;
    if let Some(err) = credential_support_err(config_protocol) {
        return Err(err);
    }
    let label = server_label(&target);
    let names = match provider {
        Some(p) => {
            // Validate the name the same way login/logout do, so a typo or a
            // non-oauth provider reports "no such provider" rather than the
            // misleading "not signed in".
            let cfg = fetch_providers(&mut tx, &mut rx, config_protocol).await?;
            validate_codex_provider(&cfg, &p)?;
            vec![p]
        }
        None => {
            let cfg = fetch_providers(&mut tx, &mut rx, config_protocol).await?;
            let names = codex_provider_names(&cfg);
            if names.is_empty() {
                println!(
                    "No oauth:codex providers configured on server {label}. Add one with \
                     `fleety config` (Providers, type oauth:codex), then \
                     `fleety auth login <provider>`."
                );
                return Ok(());
            }
            names
        }
    };
    for name in &names {
        send(
            &mut tx,
            &ClientMsg::CredentialStatus {
                kind: CREDENTIAL_KIND.to_string(),
                provider: Some(name.clone()),
            },
        )
        .await?;
        match recv(&mut rx).await? {
            Some(ServerMsg::CredentialStatusResult { error: Some(e), .. }) => {
                return Err(CoreError::Message(match e.remediation {
                    Some(r) => format!("{} — {r}", e.message),
                    None => e.message,
                }))
            }
            Some(ServerMsg::CredentialStatusResult {
                present,
                expires_at_secs,
                detail,
                ..
            }) => {
                println!(
                    "{}: {}",
                    crate::terminal_safe_text(name),
                    remote_status_line(present, expires_at_secs, detail.as_deref(), &label)
                );
            }
            other => {
                return Err(CoreError::Provider(format!(
                    "expected a credential status reply, got {other:?}"
                )))
            }
        }
    }
    if let Some(note) = legacy_local_note(&oauth::default_token_path()) {
        println!("{note}");
    }
    Ok(())
}

/// Remove a provider's credential stored on the connected server.
pub async fn logout(provider: &str) -> Result<()> {
    logout_with_target(provider, None).await
}

pub(crate) async fn logout_on_target(
    provider: &str,
    target: &connection::Resolved,
    expected_fingerprint: Option<&str>,
) -> Result<()> {
    logout_with_target(
        provider,
        Some((target.clone(), expected_fingerprint.map(ToOwned::to_owned))),
    )
    .await
}

async fn logout_with_target(
    provider: &str,
    fixed_target: Option<(connection::Resolved, Option<String>)>,
) -> Result<()> {
    let (mut tx, mut rx, config_protocol, target) =
        if let Some((target, expected_fingerprint)) = fixed_target {
            let (tx, rx, protocol, fingerprint) = connect_hello_for_auth_target(&target).await?;
            crate::provider_service::validate_server_identity(
                expected_fingerprint.as_deref(),
                fingerprint.as_deref(),
                "Provider logout",
            )
            .map_err(crate::provider_service::issue_as_error)?;
            (tx, rx, protocol, target)
        } else {
            connect_hello_for_auth().await?
        };
    if let Some(err) = credential_support_err(config_protocol) {
        return Err(err);
    }
    // Validate the provider so a typo doesn't "succeed" as a no-op delete.
    let cfg = fetch_providers(&mut tx, &mut rx, config_protocol).await?;
    validate_codex_provider(&cfg, provider)?;
    send(
        &mut tx,
        &ClientMsg::CredentialDelete {
            kind: CREDENTIAL_KIND.to_string(),
            provider: Some(provider.to_string()),
        },
    )
    .await?;
    credential_result(recv(&mut rx).await?)?;
    if let Err(e) = oauth::append_auth_audit("logout", now_secs()) {
        tracing::warn!(report = ?e.report(), "could not record auth audit");
    }
    println!(
        "Signed out provider '{}' on server {}.",
        crate::terminal_safe_text(provider),
        server_label(&target)
    );
    Ok(())
}

/// Bind the fixed Codex OAuth loopback listener, failing fast with an actionable
/// message when the port is already in use. The redirect URI is registered to
/// this exact port, so login cannot fall back to another one — the fix is to
/// free the port. Split out so the pre-check is unit-testable without running the
/// full login flow, and does not read or write any tokens.
fn bind_loopback(port: u16) -> Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        CoreError::Message(format!(
            "the OAuth loopback port {port} is already in use ({e}). The Codex redirect URI is \
             registered to this fixed port, so login cannot use a different one. Free it — close a \
             stuck earlier `fleety auth login`, or stop whatever else is bound to port {port} — and \
             then retry."
        ))
    })
}

#[derive(Debug, Clone, Copy)]
enum AuthorizationDelivery {
    BrowserOpened,
    ClipboardCopied,
    ExplicitManualFallback,
}

fn authorization_location(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    crate::terminal_safe_endpoint(without_query)
}

fn write_authorization_instructions(
    mut writer: impl Write,
    url: &str,
    delivery: AuthorizationDelivery,
) -> std::io::Result<()> {
    let location = authorization_location(url);
    match delivery {
        AuthorizationDelivery::BrowserOpened => writeln!(
            writer,
            "Opening authorization page:\n{location}\n\nThe one-time OAuth query was sent directly to your browser and was not printed."
        ),
        AuthorizationDelivery::ClipboardCopied => writeln!(
            writer,
            "Authorization URL copied to the clipboard. Paste it into your browser to continue:\n{location}\n\nThe one-time OAuth query was not printed."
        ),
        AuthorizationDelivery::ExplicitManualFallback => writeln!(
            writer,
            "WARNING: The system clipboard is unavailable. Because you explicitly chose --no-browser, Fleety will print the full one-time authorization URL below. Your terminal logs and recordings may capture its OAuth session values. Do not share them.\n\n{}",
            crate::terminal_safe_field(url)
        ),
    }
}

fn copy_authorization_url(url: &str) -> std::result::Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_text(url.to_string())
        .map_err(|error| error.to_string())
}

fn present_authorization(url: &str, no_browser: bool) -> Result<()> {
    let delivery = if no_browser {
        match copy_authorization_url(url) {
            Ok(()) => AuthorizationDelivery::ClipboardCopied,
            Err(error) => {
                tracing::debug!(%error, "clipboard unavailable for manual OAuth delivery");
                AuthorizationDelivery::ExplicitManualFallback
            }
        }
    } else {
        match open_browser(url) {
            Ok(()) => AuthorizationDelivery::BrowserOpened,
            Err(browser_error) => match copy_authorization_url(url) {
                Ok(()) => {
                    tracing::debug!(%browser_error, "browser launch failed; copied OAuth URL to clipboard");
                    AuthorizationDelivery::ClipboardCopied
                }
                Err(clipboard_error) => {
                    return Err(CoreError::Message(format!(
                        "could not open a browser or copy the authorization URL to the clipboard ({browser_error}; {clipboard_error}). Re-run with `--no-browser` to explicitly allow a terminal fallback"
                    )))
                }
            },
        }
    };
    write_authorization_instructions(std::io::stdout().lock(), url, delivery)
        .map_err(|error| CoreError::Message(format!("write OAuth instructions: {error}")))
}

/// Open `url` through the platform browser without involving a shell. Failure
/// is returned so the caller can copy the URL safely or ask for explicit
/// `--no-browser` consent instead of dumping OAuth query values.
fn open_browser(url: &str) -> std::result::Result<(), String> {
    // Windows: NOT `cmd /C start` — start runs through cmd's parser, and an
    // unquoted URL (no spaces, so Command adds no quotes) is split at every `&`,
    // executing the query parameters as commands and truncating the URL. The
    // url.dll handler takes the URL as a plain argument, no shell involved.
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let mut child = cmd.spawn().map_err(|error| error.to_string())?;
    confirm_launcher_started(
        || {
            child
                .try_wait()
                .map(|status| status.map(|status| status.success()))
        },
        std::time::Duration::from_millis(300),
    )
}

/// Give a platform URL launcher a short opportunity to report an immediate
/// failure. A launcher that is still alive after the grace period is treated as
/// successfully handed off; Fleety never waits for the browser itself to exit.
fn confirm_launcher_started(
    mut poll: impl FnMut() -> std::io::Result<Option<bool>>,
    grace: std::time::Duration,
) -> std::result::Result<(), String> {
    let deadline = std::time::Instant::now() + grace;
    loop {
        match poll().map_err(|error| error.to_string())? {
            Some(true) => return Ok(()),
            Some(false) => return Err("browser launcher exited unsuccessfully".into()),
            None if std::time::Instant::now() >= deadline => return Ok(()),
            None => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

/// Parse the `code` out of a callback request line (`GET /callback?code=..&state=..`),
/// verifying the state matches. Pure so it is unit-testable.
fn parse_callback(request_line: &str, expected_state: &str) -> Result<String> {
    // request_line looks like: "GET /auth/callback?code=abc&state=xyz HTTP/1.1"
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let version = parts.next().unwrap_or("");
    if method != "GET" || !version.starts_with("HTTP/") || parts.next().is_some() {
        return Err(CoreError::Message(
            "malformed authorization redirect request".into(),
        ));
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    if path != oauth::CODEX_CALLBACK_PATH {
        return Err(CoreError::Message(
            "authorization redirect used an unexpected callback path".into(),
        ));
    }
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = decode_form_component(raw_name, "query parameter name")?;
        let label = match name.as_str() {
            "code" => "code",
            "state" => "state",
            _ => "query parameter",
        };
        let value = decode_form_component(raw_value, label)?;
        match name.as_str() {
            "code" if code.is_some() => return Err(duplicate_callback_parameter("code")),
            "code" => code = Some(value),
            "state" if state.is_some() => return Err(duplicate_callback_parameter("state")),
            "state" => state = Some(value),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err(CoreError::Message(
            "authorization state did not match; aborting login (possible CSRF). Re-run `fleety auth login`."
                .into(),
        ));
    }
    code.ok_or_else(|| CoreError::Message("authorization redirect carried no code".into()))
}

fn decode_form_component(value: &str, parameter: &str) -> Result<String> {
    let input = value.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                let high = input.get(index + 1).and_then(|byte| hex_value(*byte));
                let low = input.get(index + 2).and_then(|byte| hex_value(*byte));
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(invalid_callback_encoding(parameter));
                };
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| invalid_callback_encoding(parameter))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn invalid_callback_encoding(parameter: &str) -> CoreError {
    CoreError::Message(format!(
        "authorization redirect contained invalid {parameter} form encoding; aborting login. Re-run `fleety auth login`."
    ))
}

fn duplicate_callback_parameter(parameter: &str) -> CoreError {
    CoreError::Message(format!(
        "authorization redirect contained duplicate {parameter} parameters; aborting login. Re-run `fleety auth login`."
    ))
}

/// Accept loopback connections until one valid callback arrives or the shared
/// deadline expires. Port probes, stale tabs, and malformed requests receive a
/// 400 response but cannot consume the OAuth session.
fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String> {
    wait_for_code_with_timeout(
        listener,
        expected_state,
        std::time::Duration::from_secs(180),
    )
}

fn wait_for_code_with_timeout(
    listener: &TcpListener,
    expected_state: &str,
    timeout: std::time::Duration,
) -> Result<String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| CoreError::Message(format!("could not configure OAuth callback: {e}")))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Err(callback_timeout(timeout));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let remaining = deadline.saturating_duration_since(now);
                let read_timeout = remaining.min(std::time::Duration::from_millis(500));
                let _ = stream.set_read_timeout(Some(read_timeout));
                let result = read_callback(&mut stream, expected_state);
                write_callback_response(&mut stream, result.is_ok());
                if let Ok(code) = result {
                    return Ok(code);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(CoreError::Message(format!("loopback accept failed: {e}"))),
        }
    }
}

fn callback_timeout(timeout: std::time::Duration) -> CoreError {
    CoreError::Message(format!(
        "OAuth callback did not arrive within {} seconds. Close any stale browser tab and re-run `fleety auth login`",
        timeout.as_secs()
    ))
}

fn read_callback(stream: &mut impl Read, expected_state: &str) -> Result<String> {
    let mut buf = [0u8; 4096];
    let n = stream
        .read(&mut buf)
        .map_err(|error| CoreError::Message(format!("could not read OAuth callback: {error}")))?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let request_line = request.lines().next().unwrap_or("");
    parse_callback(request_line, expected_state)
}

fn write_callback_response(stream: &mut impl Write, accepted: bool) {
    let (status, page): (&str, &str) = if accepted {
        ("200 OK", "Signed in to Fleety. You can close this window.")
    } else {
        (
            "400 Bad Request",
            "Login failed. You can close this window.",
        )
    };
    let body = format!("<html><body>{page}</body></html>");
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_oauth_actions_require_the_original_server_identity() {
        assert!(crate::provider_service::validate_server_identity(
            Some("server-a"),
            Some("server-a"),
            "Provider login"
        )
        .is_ok());
        assert!(crate::provider_service::validate_server_identity(
            Some("server-a"),
            Some("server-b"),
            "Provider login"
        )
        .is_err());
        assert!(crate::provider_service::validate_server_identity(
            None,
            Some("server-a"),
            "Provider logout"
        )
        .is_err());
        assert!(crate::provider_service::validate_server_identity(
            Some("server-a"),
            None,
            "Provider logout"
        )
        .is_err());
    }

    #[test]
    fn usage_error_names_the_subcommand_and_an_example() {
        let e = usage_error("login").to_string();
        assert!(e.contains("login") && e.contains("provider"));
    }

    #[test]
    fn oauth_authorization_output_hides_session_values_for_browser_and_clipboard() {
        let url = "https://auth.openai.com/oauth/authorize?state=STATE-SENTINEL&code_challenge=CHALLENGE-SENTINEL&client_id=CLIENT-SENTINEL";

        for delivery in [
            AuthorizationDelivery::BrowserOpened,
            AuthorizationDelivery::ClipboardCopied,
        ] {
            let mut captured = Vec::new();
            write_authorization_instructions(&mut captured, url, delivery)
                .expect("capture authorization instructions");
            let captured = String::from_utf8(captured).expect("UTF-8 output");

            assert!(
                captured.contains("https://auth.openai.com/oauth/authorize"),
                "sanitized location missing: {captured}"
            );
            for secret in [
                "STATE-SENTINEL",
                "CHALLENGE-SENTINEL",
                "CLIENT-SENTINEL",
                "state=",
                "code_challenge=",
                "client_id=",
            ] {
                assert!(!captured.contains(secret), "leaked {secret}: {captured}");
            }
        }
    }

    #[test]
    fn no_browser_manual_fallback_is_explicit_and_warns_before_full_url() {
        let url = "https://auth.openai.com/oauth/authorize?state=STATE-SENTINEL&code_challenge=CHALLENGE-SENTINEL";
        let mut captured = Vec::new();
        write_authorization_instructions(
            &mut captured,
            url,
            AuthorizationDelivery::ExplicitManualFallback,
        )
        .expect("capture manual fallback");
        let captured = String::from_utf8(captured).expect("UTF-8 output");

        let warning = captured.find("WARNING").expect("prominent warning");
        let exposed_url = captured.find(url).expect("manual URL");
        assert!(
            warning < exposed_url,
            "warning must precede the URL: {captured}"
        );
        assert!(
            captured.contains("--no-browser"),
            "opt-in must be named: {captured}"
        );
        assert!(
            captured.contains("terminal logs"),
            "risk must be clear: {captured}"
        );
    }

    #[test]
    fn launcher_probe_rejects_immediate_nonzero_exit() {
        let mut polls = 0;
        let error = confirm_launcher_started(
            || {
                polls += 1;
                Ok(Some(false))
            },
            std::time::Duration::from_secs(1),
        )
        .expect_err("nonzero launcher exit must trigger safe fallback");
        assert_eq!(polls, 1);
        assert!(error.contains("unsuccessfully"), "{error}");
    }

    #[test]
    fn launcher_probe_accepts_success_or_a_still_running_launcher() {
        assert!(confirm_launcher_started(
            || Ok(Some(true)),
            std::time::Duration::from_secs(1)
        )
        .is_ok());

        let started = std::time::Instant::now();
        assert!(confirm_launcher_started(
            || Ok(None),
            std::time::Duration::from_millis(20)
        )
        .is_ok());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn launcher_probe_propagates_poll_errors() {
        let error = confirm_launcher_started(
            || Err(std::io::Error::other("probe failed")),
            std::time::Duration::from_secs(1),
        )
        .expect_err("poll errors must trigger safe fallback");
        assert!(error.contains("probe failed"), "{error}");
    }

    #[test]
    fn validate_codex_provider_checks_existence_and_type() {
        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "codex1".into(),
            fleety_tools::providers_config::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        cfg.providers.insert(
            "openai1".into(),
            fleety_tools::providers_config::Provider {
                kind: "api".into(),
                base_url: Some("https://u/v1".into()),
                key: None,
            },
        );
        // An oauth:codex provider validates.
        assert!(validate_codex_provider(&cfg, "codex1").is_ok());
        // A non-oauth provider errors by name and type.
        let e = validate_codex_provider(&cfg, "openai1")
            .unwrap_err()
            .to_string();
        assert!(e.contains("openai1") && e.contains("oauth:codex"));
        // A missing provider errors by name.
        let e = validate_codex_provider(&cfg, "ghost")
            .unwrap_err()
            .to_string();
        assert!(e.contains("ghost"));
        // Enumeration lists only the codex providers.
        assert_eq!(codex_provider_names(&cfg), vec!["codex1".to_string()]);
    }

    #[test]
    fn parse_callback_extracts_code_and_checks_state() {
        let line = "GET /auth/callback?code=abc123&state=st-1 HTTP/1.1";
        assert_eq!(parse_callback(line, "st-1").expect("ok"), "abc123");

        // State mismatch aborts.
        assert!(parse_callback(line, "other").is_err());
        // Missing code errors.
        assert!(
            parse_callback("GET /auth/callback?state=st-1 HTTP/1.1", "st-1").is_err()
        );
        assert!(
            parse_callback("POST /auth/callback?code=x&state=st-1 HTTP/1.1", "st-1").is_err()
        );
        assert!(parse_callback("GET /wrong?code=x&state=st-1 HTTP/1.1", "st-1").is_err());
    }

    #[test]
    fn parse_callback_decodes_form_urlencoded_code_and_state() {
        let line = "GET /auth/callback?code=part%2Bone+two%20three&state=state+with%20spaces HTTP/1.1";
        assert_eq!(
            parse_callback(line, "state with spaces").expect("encoded callback should parse"),
            "part+one two three"
        );
    }

    #[test]
    fn parse_callback_rejects_malformed_or_non_utf8_encoding() {
        for encoded in ["%", "%2", "%GG", "%FF"] {
            let line =
                format!("GET /auth/callback?code={encoded}&state=expected HTTP/1.1");
            let message = parse_callback(&line, "expected")
                .expect_err("invalid form encoding must fail closed")
                .to_string();
            assert!(
                message.contains("code"),
                "parameter missing from: {message}"
            );
            assert!(
                message.contains("fleety auth login"),
                "remediation missing from: {message}"
            );
        }
    }

    #[test]
    fn parse_callback_rejects_duplicate_code_and_state() {
        for (query, parameter) in [
            ("code=first&code=second&state=expected", "code"),
            ("code=first&c%6fde=second&state=expected", "code"),
            ("code=only&state=expected&state=expected", "state"),
        ] {
            let line = format!("GET /auth/callback?{query} HTTP/1.1");
            let message = parse_callback(&line, "expected")
                .expect_err("duplicate security parameter must fail closed")
                .to_string();
            assert!(
                message.contains("duplicate") && message.contains(parameter),
                "duplicate parameter was not identified: {message}"
            );
            assert!(
                message.contains("fleety auth login"),
                "remediation missing from: {message}"
            );
        }
    }

    #[test]
    fn version_gate_refuses_old_servers_before_any_flow() {
        // Provider flows need protocol 5 so snapshots cannot carry plaintext keys.
        let err = credential_support_err(1).expect("old server refused");
        let msg = err.to_string();
        assert!(msg.contains("update"), "gate names the remedy: {msg}");
        assert!(credential_support_err(0).is_some());
        assert!(credential_support_err(3).is_some());
        assert!(credential_support_err(4).is_some());
        assert!(credential_support_err(5).is_none());
    }

    #[test]
    fn remote_status_line_reports_server_state_without_secrets() {
        // Signed out.
        let out = remote_status_line(false, None, None, "'home' (ws://mini:8787)");
        assert!(out.contains("Not signed in"));
        assert!(out.contains("ws://mini:8787"), "names the server: {out}");

        // Signed in: presence + expiry + detail only — the function cannot even
        // receive a token value (shape-level guarantee).
        let out = remote_status_line(
            true,
            Some(u64::MAX),
            Some("account abc"),
            "'home' (ws://mini:8787)",
        );
        assert!(out.contains("Signed in"));
        assert!(out.contains(&u64::MAX.to_string()));
        assert!(out.contains("account abc"));

        let expired = remote_status_line(true, Some(1), None, "'home' (ws://mini:8787)");
        assert!(expired.contains("expired"));

        let hostile = remote_status_line(
            true,
            None,
            Some("wss://u:p@host/x?token=SECRET#tail\u{1b}]52;c;STEAL\u{7}\nnext"),
            "server",
        );
        assert!(!hostile.contains("SECRET"), "{hostile}");
        assert!(!hostile.contains('\u{1b}'), "{hostile}");
        assert!(!hostile.contains('\u{7}'), "{hostile}");
        assert!(hostile.contains("token=<redacted>"), "{hostile}");
    }

    #[test]
    fn server_label_names_profile_or_url() {
        let profiled = connection::Resolved {
            url: "ws://mini:8787".into(),
            token: Some("t".into()),
            source: connection::Source::Profile("home".into()),
        };
        assert_eq!(server_label(&profiled), "'home' (ws://mini:8787)");
        let discovered = connection::Resolved {
            url: "ws://found:8787".into(),
            token: None,
            source: connection::Source::Mdns,
        };
        assert_eq!(server_label(&discovered), "ws://found:8787");

        let hostile = connection::Resolved {
            url: "wss://user:pass@host/path?token=SECRET#tail".into(),
            token: None,
            source: connection::Source::Profile("bad\u{1b}\nprofile".into()),
        };
        let label = server_label(&hostile);
        assert!(!label.contains("pass"), "{label}");
        assert!(!label.contains("SECRET"), "{label}");
        assert!(!label.contains('\u{1b}'), "{label}");
        assert!(label.contains("bad\\u{1b}\\nprofile"), "{label}");
    }

    #[test]
    fn legacy_local_file_is_cleaned_up_and_flagged() {
        let dir = std::env::temp_dir().join(format!("fleety-authleg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let path = dir.join("codex-oauth.json");

        // Nothing there → no note, no cleanup message.
        assert!(legacy_local_note(&path).is_none());
        assert!(cleanup_legacy_local_file(&path).is_none());

        // A leftover pre-server-side file → status flags it, login cleanup
        // removes it and says where credentials live now.
        std::fs::write(&path, "{}").expect("write");
        let note = legacy_local_note(&path).expect("flagged");
        assert!(note.contains("no longer read"));
        let cleaned = cleanup_legacy_local_file(&path).expect("cleaned");
        assert!(cleaned.contains("now live on the server"));
        assert!(!path.exists(), "login cleanup removes the leftover file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_token_path_is_terminal_safe_before_it_enters_a_note() {
        let path = std::path::Path::new(
            "wss://user:pass@host/token?secret=SECRET#tail\u{1b}]52;c;STEAL\u{7}\r\nnext",
        );
        let shown = terminal_safe_path(path);
        for secret in ["pass", "SECRET", "#tail"] {
            assert!(!shown.contains(secret), "leaked {secret}: {shown}");
        }
        for control in ['\u{1b}', '\u{7}', '\r', '\n'] {
            assert!(!shown.contains(control), "kept {control:?}: {shown}");
        }
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains("\\r\\nnext"), "{shown}");
    }

    #[test]
    fn bind_loopback_fails_fast_when_port_busy() {
        // Occupy an ephemeral port, then assert the pre-check reports it as busy
        // with an actionable message — without opening a browser or touching
        // tokens (the check is pure w.r.t. the token store).
        let occupied = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = occupied.local_addr().expect("addr").port();
        let err = bind_loopback(port).expect_err("busy port must fail fast");
        let msg = err.to_string();
        assert!(msg.contains("fixed port"), "message not actionable: {msg}");
        assert!(
            msg.contains("retry"),
            "message should tell the user to retry: {msg}"
        );
        assert!(!msg.contains('{'), "no Debug dump in the message: {msg}");

        // A free port binds successfully (login would proceed).
        drop(occupied);
        let listener = bind_loopback(port).expect("free port binds");
        drop(listener);
    }

    #[test]
    fn wait_for_code_captures_from_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        // Simulate the browser hitting the redirect URI.
        let h = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
            s.write_all(
                b"GET /auth/callback?code=the-code&state=st-9 HTTP/1.1\r\n\r\n",
            )
                .expect("write");
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf);
        });
        let code = wait_for_code(&listener, "st-9").expect("code");
        assert_eq!(code, "the-code");
        h.join().expect("join");
    }

    #[test]
    fn wait_for_code_ignores_noise_and_security_errors_until_valid_callback() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let h = std::thread::spawn(move || {
            for request in [
                "GET / HTTP/1.1\r\n\r\n",
                "GET /auth/callback?code=attacker&state=wrong HTTP/1.1\r\n\r\n",
                "GET /auth/callback?code=first&code=second&state=expected HTTP/1.1\r\n\r\n",
                "GET /auth/callback?code=real-code&state=expected HTTP/1.1\r\n\r\n",
            ] {
                let mut stream =
                    std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
                stream.write_all(request.as_bytes()).expect("write");
                let mut response = String::new();
                stream.read_to_string(&mut response).expect("read response");
                if request.contains("real-code") {
                    assert!(response.contains("200 OK"), "{response}");
                } else {
                    assert!(response.contains("400 Bad Request"), "{response}");
                }
            }
        });

        let code = wait_for_code_with_timeout(
            &listener,
            "expected",
            std::time::Duration::from_secs(2),
        )
        .expect("valid callback after noise");
        assert_eq!(code, "real-code");
        h.join().expect("join");
    }

    #[test]
    fn wrong_state_never_completes_the_oauth_session() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let h = std::thread::spawn(move || {
            let mut stream =
                std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
            stream
                .write_all(
                    b"GET /auth/callback?code=attacker&state=wrong HTTP/1.1\r\n\r\n",
                )
                .expect("write");
            let mut response = String::new();
            stream.read_to_string(&mut response).expect("read response");
            assert!(response.contains("400 Bad Request"), "{response}");
        });

        let error = wait_for_code_with_timeout(
            &listener,
            "expected",
            std::time::Duration::from_millis(150),
        )
        .expect_err("wrong state must not be accepted");
        assert!(error.to_string().contains("did not arrive"), "{error}");
        h.join().expect("join");
    }

    #[test]
    fn wait_for_code_has_a_finite_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let started = std::time::Instant::now();
        let err =
            wait_for_code_with_timeout(&listener, "never", std::time::Duration::from_millis(25))
                .expect_err("missing callback must time out");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(err.to_string().contains("re-run"));
    }
}
