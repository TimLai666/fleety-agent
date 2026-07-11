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
use fleety_tools::{connection, oauth};

use crate::{connect_hello_for_auth, recv, send};

/// The credential kind this command manages on the server.
const CREDENTIAL_KIND: &str = "codex-oauth";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Dispatch `fleety auth <sub>`.
pub async fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("login") => login(args.iter().any(|a| a == "--no-browser")).await,
        Some("status") => status().await,
        Some("logout") => logout().await,
        _ => {
            println!("usage: fleety auth <login|status|logout> [--no-browser]");
            Ok(())
        }
    }
}

/// The version gate: a server that does not advertise credential support
/// (config protocol 2) cannot store the login — refuse up front, before any
/// browser opens, with the fix spelled out.
fn credential_support_err(config_protocol: u32) -> Option<CoreError> {
    (config_protocol < 2).then(|| {
        CoreError::Message(
            "the connected server is too old to store credentials remotely — update it first \
             (run `fleety update` on the server host, or let fleet convergence catch it up), \
             then re-run `fleety auth login`"
                .to_string(),
        )
    })
}

/// Human-readable name of the server a credential operation acted on.
fn server_label(target: &connection::Resolved) -> String {
    match &target.source {
        connection::Source::Profile(name) => format!("'{name}' ({})", target.url),
        _ => target.url.clone(),
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
    if !present {
        return format!("Not signed in on server {server}. Run `fleety auth login`.");
    }
    let expiry = expires_at_secs
        .map(|s| format!("expires at unix {s}"))
        .unwrap_or_else(|| "no expiry recorded".to_string());
    let detail = detail.map(|d| format!(" ({d})")).unwrap_or_default();
    format!(
        "Signed in to ChatGPT (Codex OAuth) on server {server}{detail}. Access token: {expiry}."
    )
}

/// Remove a leftover token file from the pre-server-side era on this CLI host.
/// Returns the note to print when one was cleaned up.
fn cleanup_legacy_local_file(path: &std::path::Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let note = match oauth::clear_tokens(path) {
        Ok(()) => format!(
            "Removed the leftover local token file at {} — credentials now live on the server.",
            path.display()
        ),
        Err(e) => format!(
            "A leftover local token file at {} is no longer used, but could not be removed: {}",
            path.display(),
            e.report().message
        ),
    };
    Some(note)
}

/// The note `auth status` prints when a stale local token file still exists.
fn legacy_local_note(path: &std::path::Path) -> Option<String> {
    path.exists().then(|| {
        format!(
            "Note: the local token file at {} is no longer read by any flow; re-run \
             `fleety auth login` to store credentials on the server (login also cleans it up).",
            path.display()
        )
    })
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
pub async fn login(no_browser: bool) -> Result<()> {
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
    {
        let (_tx, _rx, config_protocol, _target) = connect_hello_for_auth().await.map_err(|e| {
            CoreError::Message(format!(
                "could not reach the server that would store this login: {} — check the \
                     connection (`fleety status`), pair this device first (`fleety pair <code>`), \
                     or set the server URL with `fleety init <ws-url>`",
                e.report().message
            ))
        })?;
        if let Some(err) = credential_support_err(config_protocol) {
            return Err(err);
        }
    }

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
    println!("Open this URL to authorize (opens automatically unless --no-browser):\n\n{url}\n");
    if !no_browser {
        open_browser(&url);
    }

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
    let (mut tx, mut rx, config_protocol, target) =
        connect_hello_for_auth().await.map_err(|e| {
            CoreError::Message(format!(
                "authorization succeeded, but the server could not be reached to store it: {} — \
             re-run `fleety auth login` once the connection is back",
                e.report().message
            ))
        })?;
    if let Some(err) = credential_support_err(config_protocol) {
        return Err(err);
    }
    send(
        &mut tx,
        &ClientMsg::CredentialPut {
            kind: CREDENTIAL_KIND.to_string(),
            payload_json,
        },
    )
    .await?;
    credential_result(recv(&mut rx).await?)?;

    if let Err(e) = oauth::append_auth_audit("login", now_secs()) {
        tracing::warn!(report = ?e.report(), "could not record auth audit");
    }
    println!(
        "Signed in. Credentials delivered to server {}.",
        server_label(&target)
    );
    if let Some(note) = cleanup_legacy_local_file(&oauth::default_token_path()) {
        println!("{note}");
    }
    Ok(())
}

/// Report whether the connected server holds a credential and when it expires —
/// never the token values (the status frame carries none by shape).
pub async fn status() -> Result<()> {
    let (mut tx, mut rx, config_protocol, target) = connect_hello_for_auth().await?;
    if let Some(err) = credential_support_err(config_protocol) {
        return Err(err);
    }
    send(
        &mut tx,
        &ClientMsg::CredentialStatus {
            kind: CREDENTIAL_KIND.to_string(),
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
                "{}",
                remote_status_line(
                    present,
                    expires_at_secs,
                    detail.as_deref(),
                    &server_label(&target)
                )
            );
        }
        other => {
            return Err(CoreError::Provider(format!(
                "expected a credential status reply, got {other:?}"
            )))
        }
    }
    if let Some(note) = legacy_local_note(&oauth::default_token_path()) {
        println!("{note}");
    }
    Ok(())
}

/// Remove the credential stored on the connected server.
pub async fn logout() -> Result<()> {
    let (mut tx, mut rx, config_protocol, target) = connect_hello_for_auth().await?;
    if let Some(err) = credential_support_err(config_protocol) {
        return Err(err);
    }
    send(
        &mut tx,
        &ClientMsg::CredentialDelete {
            kind: CREDENTIAL_KIND.to_string(),
        },
    )
    .await?;
    credential_result(recv(&mut rx).await?)?;
    if let Err(e) = oauth::append_auth_audit("logout", now_secs()) {
        tracing::warn!(report = ?e.report(), "could not record auth audit");
    }
    println!("Signed out on server {}.", server_label(&target));
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

/// Best-effort: open `url` in the platform browser. Failure is non-fatal — the
/// URL was already printed for the user to open manually.
fn open_browser(url: &str) {
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
    if let Err(e) = cmd.spawn() {
        tracing::debug!(%e, "could not launch a browser; the URL was printed for manual use");
    }
}

/// Parse the `code` out of a callback request line (`GET /callback?code=..&state=..`),
/// verifying the state matches. Pure so it is unit-testable.
fn parse_callback(request_line: &str, expected_state: &str) -> Result<String> {
    // request_line looks like: "GET /callback?code=abc&state=xyz HTTP/1.1"
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| CoreError::Message("malformed authorization redirect".into()))?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("code", v)) => code = Some(v.to_string()),
            Some(("state", v)) => state = Some(v.to_string()),
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

/// Accept one loopback connection, parse the callback, verify state, and reply
/// with a small close-the-window page. Returns the authorization code.
fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String> {
    let (mut stream, _) = listener
        .accept()
        .map_err(|e| CoreError::Message(format!("loopback accept failed: {e}")))?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let request_line = request.lines().next().unwrap_or("");
    let result = parse_callback(request_line, expected_state);
    let (status, page): (&str, &str) = if result.is_ok() {
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
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_callback_extracts_code_and_checks_state() {
        let line = "GET /callback?code=abc123&state=st-1 HTTP/1.1";
        assert_eq!(parse_callback(line, "st-1").expect("ok"), "abc123");

        // State mismatch aborts.
        assert!(parse_callback(line, "other").is_err());
        // Missing code errors.
        assert!(parse_callback("GET /callback?state=st-1 HTTP/1.1", "st-1").is_err());
    }

    #[test]
    fn version_gate_refuses_old_servers_before_any_flow() {
        // config protocol < 2 → refuse with the fix; 2+ → proceed.
        let err = credential_support_err(1).expect("old server refused");
        let msg = err.to_string();
        assert!(msg.contains("update"), "gate names the remedy: {msg}");
        assert!(credential_support_err(0).is_some());
        assert!(credential_support_err(2).is_none());
        assert!(credential_support_err(3).is_none(), "future versions pass");
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
            Some(1234),
            Some("account abc"),
            "'home' (ws://mini:8787)",
        );
        assert!(out.contains("Signed in"));
        assert!(out.contains("1234"));
        assert!(out.contains("account abc"));
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
            s.write_all(b"GET /callback?code=the-code&state=st-9 HTTP/1.1\r\n\r\n")
                .expect("write");
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf);
        });
        let code = wait_for_code(&listener, "st-9").expect("code");
        assert_eq!(code, "the-code");
        h.join().expect("join");
    }
}
