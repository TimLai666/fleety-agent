//! `fleety auth` — sign in to ChatGPT (Codex OAuth) so providers configured with
//! `auth = oauth:codex` can call the model without a static API key. Tokens are
//! stored locally (`~/.fleety/codex-oauth.json`, 0600 on Unix); this command
//! never prints them.

use std::io::{Read, Write};
use std::net::TcpListener;

use agent_core::{CoreError, Result};
use fleety_tools::oauth;

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
        Some("status") => status(),
        Some("logout") => logout(),
        _ => {
            println!("usage: fleety auth <login|status|logout> [--no-browser]");
            Ok(())
        }
    }
}

/// Run the PKCE authorization-code login: open the browser, capture the code on a
/// loopback listener, exchange it, and store the tokens.
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
    oauth::save_tokens(&oauth::default_token_path(), &tokens)?;
    if let Err(e) = oauth::append_auth_audit("login", now_secs()) {
        tracing::warn!(report = ?e.report(), "could not record auth audit");
    }
    println!(
        "Signed in. Tokens saved to {}.",
        oauth::default_token_path().display()
    );
    Ok(())
}

/// Report whether the user is signed in and when the token expires — never the
/// token values.
pub fn status() -> Result<()> {
    println!("{}", status_line());
    Ok(())
}

/// The status message (no token values), split out so it is unit-testable.
fn status_line() -> String {
    match oauth::load_tokens(&oauth::default_token_path()) {
        Some(t) => {
            let state = if oauth::needs_refresh(t.expires_at_secs, now_secs()) {
                "expired (will refresh on next use)"
            } else {
                "valid"
            };
            format!(
                "Signed in to ChatGPT (Codex OAuth). Access token: {state}; expires at unix {}.",
                t.expires_at_secs
            )
        }
        None => "Not signed in. Run `fleety auth login`.".to_string(),
    }
}

/// Remove the stored tokens.
pub fn logout() -> Result<()> {
    oauth::clear_tokens(&oauth::default_token_path())?;
    if let Err(e) = oauth::append_auth_audit("logout", now_secs()) {
        tracing::warn!(report = ?e.report(), "could not record auth audit");
    }
    println!("Signed out; local tokens removed.");
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
    fn status_hides_tokens_and_logout_clears_and_audits() {
        // Redirect the token store + audit to temp paths (process-global env, so
        // this is the only auth test that sets them — avoids a parallel-test race).
        let dir = std::env::temp_dir().join(format!("fleety-authcmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk");
        let tokens_path = dir.join("codex-oauth.json");
        let audit_path = dir.join("auth-audit.jsonl");
        std::env::set_var("FLEETY_CODEX_TOKENS", &tokens_path);
        std::env::set_var("FLEETY_CODEX_AUDIT", &audit_path);

        // Logged out.
        assert!(status_line().contains("Not signed in"));

        // Sign in with a secret access token; status must not leak it.
        let tokens = oauth::Tokens {
            access_token: "SECRET-ACCESS".into(),
            refresh_token: "SECRET-REFRESH".into(),
            expires_at_secs: now_secs() + 3600,
            token_type: "Bearer".into(),
            account_id: None,
        };
        oauth::save_tokens(&oauth::default_token_path(), &tokens).expect("save");
        let line = status_line();
        assert!(line.contains("Signed in"));
        assert!(!line.contains("SECRET-ACCESS"));
        assert!(!line.contains("SECRET-REFRESH"));

        // Logout removes the token file and records an audit event.
        logout().expect("logout");
        assert!(oauth::load_tokens(&oauth::default_token_path()).is_none());
        let audit = std::fs::read_to_string(&audit_path).expect("audit");
        assert!(audit.contains("\"logout\""));

        std::env::remove_var("FLEETY_CODEX_TOKENS");
        std::env::remove_var("FLEETY_CODEX_AUDIT");
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
