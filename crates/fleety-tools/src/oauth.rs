//! Codex ChatGPT OAuth: PKCE helpers, a protected on-disk token store, and a
//! bearer source that refreshes automatically. Tokens live in the Agent home
//! (`~/.fleety/codex-oauth.json`, 0600 on Unix), never in the config or providers
//! file. Endpoints and the client id are overridable via config; see `config.rs`.

use std::path::{Path, PathBuf};

use agent_core::{CoreError, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// base64url without padding — the encoding PKCE and the token store use.
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a high-entropy PKCE `code_verifier` (RFC 7636): 43+ chars from the
/// unreserved set. Entropy comes from two v4 UUIDs (32 random bytes) — no extra
/// dependency — base64url-encoded to 43 chars.
pub fn generate_verifier() -> String {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    b64url(&bytes)
}

/// The S256 `code_challenge` for a verifier: `base64url(sha256(verifier))`.
pub fn challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    b64url(&hasher.finalize())
}

/// A random `state` value for CSRF protection on the authorization redirect.
pub fn generate_state() -> String {
    b64url(uuid::Uuid::new_v4().as_bytes())
}

/// Extract the ChatGPT account id from an OAuth `id_token` (a JWT). Reads the
/// `chatgpt_account_id` claim, falling back to the nested OpenAI auth claim and
/// then the first organization id (matching the Codex CLI / heddle behavior).
/// Pure; a malformed token yields `None` rather than an error.
pub fn parse_account_id(id_token: &str) -> Option<String> {
    let payload_b64 = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let direct = claims.get("chatgpt_account_id").and_then(Value::as_str);
    let nested = claims
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(Value::as_str);
    let first_org = claims
        .get("organizations")
        .and_then(Value::as_array)
        .and_then(|orgs| {
            orgs.iter()
                .find_map(|o| o.get("id").and_then(Value::as_str))
        });
    direct.or(nested).or(first_org).map(String::from)
}

/// The persisted OAuth tokens for the logged-in ChatGPT account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds at which `access_token` expires.
    pub expires_at_secs: u64,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    /// The ChatGPT account id decoded from the login `id_token` (sent as the
    /// `chatgpt-account-id` header on Codex Responses calls). Absent for older
    /// token files or when the token carried no account claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

/// The token store path: `~/.fleety/codex-oauth.json`, or `FLEETY_CODEX_TOKENS`
/// when set (used by tests and unusual installs). Never the config/providers file.
pub fn default_token_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_CODEX_TOKENS") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".fleety").join("codex-oauth.json")
}

/// Load tokens from `path`. A missing or corrupt file reads as "logged out"
/// (`None`) rather than an error, so a bad file never wedges the agent.
pub fn load_tokens(path: &Path) -> Option<Tokens> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist tokens to `path`, creating the directory and restricting the file to
/// owner-only (mode 0600) on Unix. Never written into the config/providers file.
pub fn save_tokens(path: &Path, tokens: &Tokens) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create token dir: {e}")))?;
    }
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| CoreError::Message(format!("serialize tokens: {e}")))?;
    std::fs::write(path, json).map_err(|e| CoreError::Message(format!("write tokens: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Remove the token store (logout). A missing file is not an error.
pub fn clear_tokens(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::Message(format!("remove tokens: {e}"))),
    }
}

/// Local audit trail for auth events (`~/.fleety/auth-audit.jsonl`). Login/logout
/// happen at the CLI, not over a device connection, so they are recorded here
/// (never with token values). `FLEETY_CODEX_AUDIT` overrides the path for tests.
pub fn auth_audit_path() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_CODEX_AUDIT") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".fleety").join("auth-audit.jsonl")
}

/// Append an auth-audit event (e.g. `"login"`, `"logout"`) with a timestamp.
/// Best-effort: a failure is returned so callers can warn, but never carries a
/// token value.
pub fn append_auth_audit(event: &str, now_secs: u64) -> Result<()> {
    let path = auth_audit_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create audit dir: {e}")))?;
    }
    let line = format!(
        "{}\n",
        serde_json::json!({ "ts_secs": now_secs, "event": event })
    );
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| CoreError::Message(format!("open auth audit: {e}")))?;
    f.write_all(line.as_bytes())
        .map_err(|e| CoreError::Message(format!("write auth audit: {e}")))
}

/// Treat the access token as expired this many seconds early, so a request never
/// races the true expiry.
const EXPIRY_SKEW_SECS: u64 = 60;

/// Whether the access token should be refreshed at `now` (expired or within the
/// skew window). Pure — the clock is a parameter.
pub fn needs_refresh(expires_at_secs: u64, now_secs: u64) -> bool {
    now_secs.saturating_add(EXPIRY_SKEW_SECS) >= expires_at_secs
}

/// What to do to get a valid bearer, given the stored tokens and the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BearerPlan {
    /// No usable tokens — the user must log in.
    NotLoggedIn,
    /// The stored access token is still valid; use it as-is.
    Use(String),
    /// The access token is stale; refresh with this refresh token.
    Refresh(String),
}

/// Decide how to obtain a valid bearer from the current tokens at `now`. Pure, so
/// the clock and token state are both injectable in tests.
pub fn plan_bearer(tokens: Option<&Tokens>, now_secs: u64) -> BearerPlan {
    match tokens {
        None => BearerPlan::NotLoggedIn,
        Some(t) if t.refresh_token.is_empty() && t.access_token.is_empty() => {
            BearerPlan::NotLoggedIn
        }
        Some(t) if needs_refresh(t.expires_at_secs, now_secs) => {
            if t.refresh_token.is_empty() {
                BearerPlan::NotLoggedIn
            } else {
                BearerPlan::Refresh(t.refresh_token.clone())
            }
        }
        Some(t) => BearerPlan::Use(t.access_token.clone()),
    }
}

/// Endpoints and client id for the Codex ChatGPT OAuth flow. Overridable via
/// config; the caller passes the resolved values.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    pub authorize_endpoint: String,
    pub token_endpoint: String,
    pub backend_base_url: String,
}

/// Resolve one OAuth setting via the shared config precedence (env → config.toml
/// → the registry default). The endpoints have stable defaults in the registry;
/// the client id defaults to empty (login then errors actionably if unset). The
/// endpoint/client-id values MUST match the current Codex CLI — see the change's
/// Open Questions; they are overridable without a rebuild.
fn config_value(key: &str) -> String {
    if let Ok(v) = std::env::var(key) {
        if !v.is_empty() {
            return v;
        }
    }
    let map = crate::config::load(&crate::config::config_path());
    crate::config::resolve(key, &map)
        .map(|r| r.value)
        .filter(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Resolve the OAuth configuration from env/config/registry defaults.
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        client_id: config_value("FLEETY_CODEX_CLIENT_ID"),
        authorize_endpoint: config_value("FLEETY_CODEX_AUTHORIZE_URL"),
        token_endpoint: config_value("FLEETY_CODEX_TOKEN_URL"),
        backend_base_url: config_value("FLEETY_CODEX_BACKEND_URL"),
    }
}

/// The fixed loopback port the Codex OAuth client id is registered with. The
/// redirect URI must match exactly (`http://localhost:1455/auth/callback`), so
/// this is not a free/random port — `auth.openai.com` rejects a mismatch.
pub const CODEX_LOOPBACK_PORT: u16 = 1455;

/// The callback path the redirect URI uses (paired with `CODEX_LOOPBACK_PORT`).
pub const CODEX_CALLBACK_PATH: &str = "/auth/callback";

/// Identifies this client to the Codex authorization flow. Free-form; overridable
/// via `FLEETY_CODEX_ORIGINATOR`.
fn originator() -> String {
    std::env::var("FLEETY_CODEX_ORIGINATOR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "fleety".to_string())
}

/// Build the authorization URL for the Codex PKCE flow. Pure so its query is
/// testable. Includes the Codex "simplified flow" params the client id expects.
pub fn authorize_url(
    config: &OAuthConfig,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> String {
    let q = |s: &str| {
        // Minimal percent-encoding for the values we place in the query.
        s.replace('%', "%25")
            .replace(' ', "%20")
            .replace('&', "%26")
            .replace('?', "%3F")
            .replace('#', "%23")
            .replace(':', "%3A")
            .replace('/', "%2F")
    };
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}&scope={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator={}",
        config.authorize_endpoint,
        q(&config.client_id),
        q(redirect_uri),
        q(challenge),
        q(state),
        q("openid profile email offline_access"),
        q(&originator())
    )
}

/// Exchange an authorization `code` (with its PKCE verifier) for tokens. Async
/// HTTP; an error is actionable.
pub async fn exchange_code(
    client: &reqwest::Client,
    config: &OAuthConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    now_secs: u64,
) -> Result<Tokens> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", verifier),
        ("redirect_uri", redirect_uri),
    ];
    let resp = client
        .post(&config.token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("token exchange request failed: {e}")))?;
    if !resp.status().is_success() {
        let code = resp.status();
        return Err(CoreError::Provider(format!(
            "token exchange returned HTTP {code}"
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| CoreError::Provider(format!("token exchange returned non-JSON: {e}")))?;
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Provider("token exchange response missing access_token".into()))?
        .to_string();
    let refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    let token_type = body
        .get("token_type")
        .and_then(Value::as_str)
        .unwrap_or("Bearer")
        .to_string();
    let account_id = body
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(parse_account_id);
    Ok(Tokens {
        access_token,
        refresh_token,
        expires_at_secs: now_secs.saturating_add(expires_in),
        token_type,
        account_id,
    })
}

/// Exchange a refresh token for a fresh access token at `token_endpoint`. Returns
/// the new tokens (carrying the old refresh token forward when the server does not
/// issue a new one). An HTTP or parse failure is an actionable error.
pub async fn refresh_access_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    refresh_token: &str,
    prev_account_id: Option<&str>,
    now_secs: u64,
) -> Result<Tokens> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    let resp = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("token refresh request failed: {e}")))?;
    if !resp.status().is_success() {
        let code = resp.status();
        return Err(CoreError::Provider(format!(
            "token refresh returned HTTP {code}; run `fleety auth login` to sign in again"
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| CoreError::Provider(format!("token refresh returned non-JSON: {e}")))?;
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Provider("token refresh response missing access_token".into()))?
        .to_string();
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    let new_refresh = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| refresh_token.to_string());
    let token_type = body
        .get("token_type")
        .and_then(Value::as_str)
        .unwrap_or("Bearer")
        .to_string();
    // Refresh responses usually omit id_token; keep the previously captured
    // account id when so.
    let account_id = body
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(parse_account_id)
        .or_else(|| prev_account_id.map(String::from));
    Ok(Tokens {
        access_token,
        refresh_token: new_refresh,
        expires_at_secs: now_secs.saturating_add(expires_in),
        token_type,
        account_id,
    })
}

/// A bearer source backed by the OAuth token store: reads the tokens, refreshes
/// the access token on expiry (persisting the result), and yields it. When no
/// tokens are stored it returns an actionable "log in" error.
pub struct OAuthBearer {
    token_path: PathBuf,
    token_endpoint: String,
    client_id: String,
    client: reqwest::Client,
}

impl OAuthBearer {
    pub fn new(token_path: PathBuf, config: &OAuthConfig) -> Self {
        Self {
            token_path,
            token_endpoint: config.token_endpoint.clone(),
            client_id: config.client_id.clone(),
            client: reqwest::Client::new(),
        }
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// A Codex credentials source (bearer + account id) backed by the OAuth token
/// store, for the Codex Responses provider. Refreshes on expiry like `OAuthBearer`.
pub struct OAuthCodexAuth {
    token_path: PathBuf,
    token_endpoint: String,
    client_id: String,
    client: reqwest::Client,
}

impl OAuthCodexAuth {
    pub fn new(token_path: PathBuf, config: &OAuthConfig) -> Self {
        Self {
            token_path,
            token_endpoint: config.token_endpoint.clone(),
            client_id: config.client_id.clone(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl agent_core::CodexAuth for OAuthCodexAuth {
    async fn credentials(&self) -> Result<agent_core::CodexCreds> {
        let tokens = load_tokens(&self.token_path);
        let now = OAuthBearer::now();
        match plan_bearer(tokens.as_ref(), now) {
            BearerPlan::NotLoggedIn => Err(CoreError::Provider(
                "not signed in to ChatGPT; run `fleety auth login` to authenticate".into(),
            )),
            BearerPlan::Use(access) => Ok(agent_core::CodexCreds {
                bearer: access,
                account_id: tokens.and_then(|t| t.account_id),
            }),
            BearerPlan::Refresh(refresh) => {
                let prev_account = tokens.as_ref().and_then(|t| t.account_id.as_deref());
                let fresh = refresh_access_token(
                    &self.client,
                    &self.token_endpoint,
                    &self.client_id,
                    &refresh,
                    prev_account,
                    now,
                )
                .await?;
                save_tokens(&self.token_path, &fresh)?;
                Ok(agent_core::CodexCreds {
                    bearer: fresh.access_token,
                    account_id: fresh.account_id,
                })
            }
        }
    }
}

#[async_trait::async_trait]
impl agent_core::BearerSource for OAuthBearer {
    async fn bearer(&self) -> Result<Option<String>> {
        let tokens = load_tokens(&self.token_path);
        let now = Self::now();
        match plan_bearer(tokens.as_ref(), now) {
            BearerPlan::NotLoggedIn => Err(CoreError::Provider(
                "not signed in to ChatGPT; run `fleety auth login` to authenticate".into(),
            )),
            BearerPlan::Use(access) => Ok(Some(access)),
            BearerPlan::Refresh(refresh) => {
                let prev_account = tokens.as_ref().and_then(|t| t.account_id.as_deref());
                let fresh = refresh_access_token(
                    &self.client,
                    &self.token_endpoint,
                    &self.client_id,
                    &refresh,
                    prev_account,
                    now,
                )
                .await?;
                save_tokens(&self.token_path, &fresh)?;
                Ok(Some(fresh.access_token))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn pkce_matches_rfc7636_test_vector() {
        // RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    fn temp_token_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("fleety-oauth-{}", uuid::Uuid::new_v4()))
            .join("codex-oauth.json")
    }

    #[test]
    fn token_store_round_trips_and_is_owner_only() {
        let path = temp_token_path();
        // Missing file → logged out.
        assert_eq!(load_tokens(&path), None);

        let tokens = Tokens {
            access_token: "at-1".into(),
            refresh_token: "rt-1".into(),
            expires_at_secs: 1_000,
            token_type: "Bearer".into(),
            account_id: Some("acc-1".into()),
        };
        save_tokens(&path, &tokens).expect("save");
        assert_eq!(load_tokens(&path).as_ref(), Some(&tokens));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be owner-only");
        }

        // Corrupt file → logged out, not an error.
        std::fs::write(&path, "not json").expect("clobber");
        assert_eq!(load_tokens(&path), None);

        // Logout removes the file; a second logout is fine.
        clear_tokens(&path).expect("clear");
        assert_eq!(load_tokens(&path), None);
        clear_tokens(&path).expect("clear again (missing ok)");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    fn fake_jwt(claims: serde_json::Value) -> String {
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("header.{payload}.sig")
    }

    #[test]
    fn parse_account_id_reads_claim_with_fallbacks() {
        // Direct claim.
        assert_eq!(
            parse_account_id(&fake_jwt(json!({ "chatgpt_account_id": "acc-1" }))).as_deref(),
            Some("acc-1")
        );
        // Nested OpenAI auth claim.
        assert_eq!(
            parse_account_id(&fake_jwt(
                json!({ "https://api.openai.com/auth": { "chatgpt_account_id": "acc-2" } })
            ))
            .as_deref(),
            Some("acc-2")
        );
        // First organization id.
        assert_eq!(
            parse_account_id(&fake_jwt(json!({ "organizations": [{ "id": "org-9" }] }))).as_deref(),
            Some("org-9")
        );
        // Malformed / no claim → None, never an error.
        assert_eq!(parse_account_id("not-a-jwt"), None);
        assert_eq!(parse_account_id(&fake_jwt(json!({ "sub": "x" }))), None);
    }

    #[test]
    fn authorize_url_carries_pkce_and_codex_flow_params() {
        let config = OAuthConfig {
            client_id: "app_TEST".into(),
            authorize_endpoint: "https://auth.openai.com/oauth/authorize".into(),
            token_endpoint: "https://auth.openai.com/oauth/token".into(),
            backend_base_url: "https://backend.example".into(),
        };
        let redirect = format!("http://localhost:{CODEX_LOOPBACK_PORT}{CODEX_CALLBACK_PATH}");
        let url = authorize_url(&config, &redirect, "chal-123", "st-1");
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("client_id=app_TEST"));
        assert!(url.contains("code_challenge=chal-123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=st-1"));
        // The Codex simplified-flow params the client id expects.
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("id_token_add_organizations=true"));
        assert!(url.contains("originator="));
        // Redirect uri is percent-encoded and points at the fixed loopback port.
        assert!(url.contains("1455"));
    }

    #[test]
    fn plan_bearer_uses_clock_and_token_state() {
        // No tokens → must log in.
        assert_eq!(plan_bearer(None, 100), BearerPlan::NotLoggedIn);

        let t = Tokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at_secs: 1_000,
            token_type: "Bearer".into(),
            account_id: None,
        };
        // Well before expiry → use the stored token.
        assert_eq!(plan_bearer(Some(&t), 500), BearerPlan::Use("at".into()));
        // Inside the skew window (>= expires - 60) → refresh.
        assert_eq!(plan_bearer(Some(&t), 950), BearerPlan::Refresh("rt".into()));
        assert!(needs_refresh(1_000, 950));
        assert!(!needs_refresh(1_000, 500));

        // Expired but no refresh token → must log in again.
        let no_refresh = Tokens {
            refresh_token: "".into(),
            ..t.clone()
        };
        assert_eq!(
            plan_bearer(Some(&no_refresh), 2_000),
            BearerPlan::NotLoggedIn
        );
    }

    /// One-shot HTTP responder: accepts a single connection, returns the given
    /// JSON body with `status`, and hands back the captured request line.
    fn serve_once_json(status: &str, body: String) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = std::sync::mpsc::channel();
        let status = status.to_string();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn exchange_code_captures_account_id_from_id_token() {
        let id_token = fake_jwt(json!({ "chatgpt_account_id": "acct-XYZ" }));
        let body = json!({
            "access_token": "at", "refresh_token": "rt", "expires_in": 3600,
            "token_type": "Bearer", "id_token": id_token
        })
        .to_string();
        let (base, _rx) = serve_once_json("200 OK", body);
        let config = OAuthConfig {
            client_id: "cid".into(),
            authorize_endpoint: "https://a/authorize".into(),
            token_endpoint: base,
            backend_base_url: "https://b".into(),
        };
        let client = reqwest::Client::new();
        let tokens = exchange_code(
            &client,
            &config,
            "code",
            "verifier",
            "http://localhost:1455/auth/callback",
            0,
        )
        .await
        .expect("exchange");
        assert_eq!(tokens.account_id.as_deref(), Some("acct-XYZ"));
    }

    #[tokio::test]
    async fn codex_auth_returns_bearer_and_account_or_errors() {
        use agent_core::CodexAuth;
        let config = OAuthConfig {
            client_id: "cid".into(),
            authorize_endpoint: "https://a/authorize".into(),
            token_endpoint: "https://a/token".into(),
            backend_base_url: "https://b".into(),
        };

        // Logged out → actionable error.
        let missing =
            std::env::temp_dir().join(format!("fleety-cauth-{}.json", uuid::Uuid::new_v4()));
        let auth = OAuthCodexAuth::new(missing, &config);
        assert!(auth
            .credentials()
            .await
            .expect_err("logged out")
            .report()
            .message
            .contains("auth login"));

        // Logged in with a valid (unexpired) token → bearer + account id.
        let path = temp_token_path();
        save_tokens(
            &path,
            &Tokens {
                access_token: "AT".into(),
                refresh_token: "RT".into(),
                expires_at_secs: 9_999_999_999, // far future → not expired
                token_type: "Bearer".into(),
                account_id: Some("acc-7".into()),
            },
        )
        .expect("save");
        let auth = OAuthCodexAuth::new(path.clone(), &config);
        let creds = auth.credentials().await.expect("creds");
        assert_eq!(creds.bearer, "AT");
        assert_eq!(creds.account_id.as_deref(), Some("acc-7"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn oauth_bearer_errors_actionably_when_not_logged_in() {
        use agent_core::BearerSource;
        // Point at a token path that does not exist → not signed in.
        let missing =
            std::env::temp_dir().join(format!("fleety-noauth-{}.json", uuid::Uuid::new_v4()));
        let config = OAuthConfig {
            client_id: "cid".into(),
            authorize_endpoint: "https://auth.example/authorize".into(),
            token_endpoint: "https://auth.example/token".into(),
            backend_base_url: "https://backend.example".into(),
        };
        let bearer = OAuthBearer::new(missing, &config);
        let err = bearer.bearer().await.expect_err("should not be signed in");
        assert!(err.report().message.contains("auth login"));
    }

    #[tokio::test]
    async fn refresh_exchanges_and_reports_failures() {
        // Success: returns fresh tokens, carrying the old refresh token forward
        // when the server omits a new one.
        let (base, rx) = serve_once_json(
            "200 OK",
            r#"{"access_token":"new-at","expires_in":3600,"token_type":"Bearer"}"#.to_string(),
        );
        let client = reqwest::Client::new();
        let fresh = refresh_access_token(
            &client,
            &base,
            "client-x",
            "old-rt",
            Some("acc-prev"),
            1_000,
        )
        .await
        .expect("refresh");
        assert_eq!(fresh.access_token, "new-at");
        assert_eq!(fresh.refresh_token, "old-rt"); // carried forward
        assert_eq!(fresh.expires_at_secs, 1_000 + 3600);
        assert_eq!(fresh.account_id.as_deref(), Some("acc-prev")); // kept (no id_token in refresh)
        let req = rx.recv_timeout(Duration::from_secs(5)).expect("request");
        assert!(req.contains("grant_type=refresh_token"));
        assert!(req.contains("client_id=client-x"));

        // Failure: an HTTP error becomes an actionable "log in again" message.
        let (base, _) = serve_once_json("400 Bad Request", "nope".to_string());
        let err = refresh_access_token(&client, &base, "client-x", "bad-rt", None, 1_000)
            .await
            .expect_err("should fail");
        assert!(err.report().message.contains("auth login"));
    }

    #[test]
    fn generated_verifier_is_valid_length_and_charset() {
        let v = generate_verifier();
        assert!(
            (43..=128).contains(&v.len()),
            "len {} out of range",
            v.len()
        );
        // Unreserved set for base64url-no-pad: A-Z a-z 0-9 - _
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        // Two calls differ (random).
        assert_ne!(generate_verifier(), generate_verifier());
        // The challenge is deterministic for a given verifier.
        assert_eq!(challenge_for(&v), challenge_for(&v));
    }
}
