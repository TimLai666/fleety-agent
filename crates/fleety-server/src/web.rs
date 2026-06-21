//! Read-only HTTP fetch tool: connect to platforms/APIs over HTTP.
//!
//! SSRF-guarded — only `http`/`https`, and loopback/private-network hosts are
//! refused unless `FLEETY_ALLOW_PRIVATE_NET=1`. Body is size-capped.

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const DEFAULT_MAX_BYTES: usize = 100_000;

/// Register the web fetch tool.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(Box::new(FetchUrl));
}

/// Whether a host is loopback / private / link-local (blocked by default).
fn is_blocked_host(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") || h == "::1" || h == "0.0.0.0" {
        return true;
    }
    if h.starts_with("127.")
        || h.starts_with("10.")
        || h.starts_with("192.168.")
        || h.starts_with("169.254.")
    {
        return true;
    }
    // fc00::/7 unique-local IPv6
    if h.starts_with("fc") || h.starts_with("fd") {
        return true;
    }
    // 172.16.0.0/12
    if let Some(second) = h.strip_prefix("172.").and_then(|r| r.split('.').next()) {
        if let Ok(octet) = second.parse::<u8>() {
            if (16..=31).contains(&octet) {
                return true;
            }
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
                "refusing to fetch loopback/private host '{host}'; set FLEETY_ALLOW_PRIVATE_NET=1 to allow"
            )));
        }
        let max_bytes = args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_BYTES);

        let response = reqwest::Client::new()
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
