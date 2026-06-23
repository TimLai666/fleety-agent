//! Read-only HTTP fetch tool: connect to platforms/APIs over HTTP.
//!
//! SSRF-guarded — only `http`/`https`, and loopback/private-network hosts are
//! refused unless `FLEETY_ALLOW_PRIVATE_NET=1`. Body is size-capped.

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const DEFAULT_MAX_BYTES: usize = 100_000;

/// Register the web tools (read-only fetch + general HTTP request).
pub fn register(registry: &mut ToolRegistry) {
    registry.register(Box::new(FetchUrl));
    registry.register(Box::new(HttpRequest));
}

/// Validate scheme + host (SSRF guard) for an outbound URL.
fn guard_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| CoreError::Message(format!("invalid url '{raw}': {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CoreError::Message(format!(
            "unsupported url scheme '{}'; only http/https are allowed",
            url.scheme()
        )));
    }
    let host = url
        .host_str()
        .ok_or_else(|| CoreError::Message("url has no host".to_string()))?;
    if is_blocked_host(host) && !allow_private() {
        return Err(CoreError::Message(format!(
            "refusing to reach loopback/private host '{host}'; set FLEETY_ALLOW_PRIVATE_NET=1 to allow"
        )));
    }
    Ok(url)
}

/// A reqwest client that does not auto-follow redirects (so a 3xx can't bounce
/// to a private host past the guard).
fn no_redirect_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| CoreError::Message(format!("http client build failed: {e}")))
}

/// Whether a parsed IP is loopback / private / link-local / unspecified.
fn ip_blocked(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            // IPv4-mapped/compatible (e.g. ::ffff:127.0.0.1) -> check as IPv4.
            if let Some(v4) = v6.to_ipv4() {
                return v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified();
            }
            let seg0 = v6.segments()[0];
            (seg0 & 0xfe00) == 0xfc00 // fc00::/7 unique-local
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

/// Whether a host is loopback / private / link-local (blocked by default).
fn is_blocked_host(host: &str) -> bool {
    let h = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    // Robust check: if the host is a literal IP (incl. IPv4-mapped IPv6), classify it.
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        if ip_blocked(ip) {
            return true;
        }
    }
    false
}

fn allow_private() -> bool {
    std::env::var("FLEETY_ALLOW_PRIVATE_NET").as_deref() == Ok("1")
}

struct FetchUrl;

#[async_trait]
impl Tool for FetchUrl {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch_url".to_string(),
            description: "HTTP GET a public URL and return status + body (read-only; loopback/private-network hosts are blocked).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "max_bytes": { "type": "integer", "description": "cap on returned body bytes" }
                },
                "required": ["url"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let raw = args.get("url").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'url'".to_string())
        })?;
        let url = guard_url(raw)?;
        let max_bytes = args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_BYTES);

        let response = no_redirect_client()?
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Message(format!("request failed: {e}")))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let full = response
            .text()
            .await
            .map_err(|e| CoreError::Message(format!("reading body failed: {e}")))?;
        let truncated = full.chars().count() > max_bytes;
        let body: String = full.chars().take(max_bytes).collect();
        Ok(json!({
            "status": status,
            "content_type": content_type,
            "truncated": truncated,
            "body": body
        }))
    }
}

struct HttpRequest;

#[async_trait]
impl Tool for HttpRequest {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "http_request".to_string(),
            description: "Make an HTTP request (GET/POST/PUT/PATCH/DELETE/HEAD) to a public URL with optional headers and body; loopback/private hosts are blocked.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] },
                    "url": { "type": "string" },
                    "headers": { "type": "object" },
                    "body": { "type": "string" },
                    "max_bytes": { "type": "integer" }
                },
                "required": ["method", "url"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let method_str = args
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Message("missing required string argument 'method'".to_string())
            })?
            .to_ascii_uppercase();
        if !matches!(
            method_str.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD"
        ) {
            return Err(CoreError::Message(format!(
                "unsupported HTTP method '{method_str}'"
            )));
        }
        let raw = args.get("url").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'url'".to_string())
        })?;
        let url = guard_url(raw)?;
        let method = reqwest::Method::from_bytes(method_str.as_bytes())
            .map_err(|e| CoreError::Message(format!("bad method: {e}")))?;
        let max_bytes = args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_BYTES);

        let mut request = no_redirect_client()?.request(method, url);
        if let Some(headers) = args.get("headers").and_then(Value::as_object) {
            for (key, value) in headers {
                if let Some(v) = value.as_str() {
                    request = request.header(key, v);
                }
            }
        }
        if let Some(body) = args.get("body").and_then(Value::as_str) {
            request = request.body(body.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|e| CoreError::Message(format!("request failed: {e}")))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let full = response
            .text()
            .await
            .map_err(|e| CoreError::Message(format!("reading body failed: {e}")))?;
        let truncated = full.chars().count() > max_bytes;
        let body: String = full.chars().take(max_bytes).collect();
        Ok(
            json!({ "status": status, "content_type": content_type, "truncated": truncated, "body": body }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn blocks_private_and_loopback() {
        for h in [
            "localhost",
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "169.254.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.1.1",
            "0.0.0.0",
            "127.0.0.1.",
            "[::1]",
        ] {
            assert!(is_blocked_host(h), "{h} should be blocked");
        }
        for h in [
            "example.com",
            "8.8.8.8",
            "172.32.0.1",
            "1.1.1.1",
            "github.com",
        ] {
            assert!(!is_blocked_host(h), "{h} should be allowed");
        }
    }

    #[test]
    fn guard_allows_public_urls_and_env_can_allow_private_hosts() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        std::env::remove_var("FLEETY_ALLOW_PRIVATE_NET");

        let url = guard_url("https://example.com/path").expect("public url");
        assert_eq!(url.host_str(), Some("example.com"));
        assert!(guard_url("http://127.0.0.1:8787/").is_err());

        std::env::set_var("FLEETY_ALLOW_PRIVATE_NET", "1");
        let private = guard_url("http://127.0.0.1:8787/").expect("private allowed");
        assert_eq!(private.host_str(), Some("127.0.0.1"));
        std::env::remove_var("FLEETY_ALLOW_PRIVATE_NET");

        no_redirect_client().expect("client");
    }

    #[tokio::test]
    async fn rejects_bad_scheme_and_blocked_host() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        assert!(registry
            .call("fetch_url", json!({ "url": "file:///etc/passwd" }))
            .await
            .is_err());
        assert!(registry
            .call("fetch_url", json!({ "url": "ftp://example.com" }))
            .await
            .is_err());
        assert!(registry
            .call("fetch_url", json!({ "url": "http://localhost:8787/" }))
            .await
            .is_err());
        assert!(registry
            .call("fetch_url", json!({ "url": "not a url" }))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn http_request_validates_method_and_host() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        assert!(registry
            .call(
                "http_request",
                json!({ "method": "FOO", "url": "https://example.com" })
            )
            .await
            .is_err());
        assert!(registry
            .call(
                "http_request",
                json!({ "method": "GET", "url": "http://127.0.0.1/" })
            )
            .await
            .is_err());
        assert!(registry
            .call(
                "http_request",
                json!({ "method": "POST", "url": "file:///x" })
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn tools_require_url_and_method_arguments() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);

        assert!(registry.call("fetch_url", json!({})).await.is_err());
        assert!(registry
            .call("http_request", json!({ "url": "https://example.com" }))
            .await
            .is_err());
        assert!(registry
            .call("http_request", json!({ "method": "GET" }))
            .await
            .is_err());
    }
}
