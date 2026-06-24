//! `ws_call`: one-shot WebSocket interaction.
//!
//! Connect to a `ws://` / `wss://` URL, send any number of text frames in
//! order, receive up to N frames (with an overall deadline), then close. The
//! SSRF guard accepts `ws`/`wss` schemes alongside the usual `http`/`https`.
//! Cookie jars work via an `Cookie:` header at handshake time — same persistent
//! file format as `http_request`'s `cookie_jar`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolSpec};

const DEFAULT_RECV_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_RECV_FRAMES: usize = 50;

pub struct WsCall {
    pub cookies_dir: PathBuf,
    /// Unused today (no `stream_to_file`), but kept here so the constructor in
    /// `web::register` stays uniform with `SseStream` / `HttpRequest`. Lets us
    /// add file-backed WS transcripts later without changing the call site.
    #[allow(dead_code)]
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for WsCall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ws_call".to_string(),
            description: "Open a WebSocket to a public ws/wss URL, send text frames, receive up to `max_frames` frames (or until the connection closes / the deadline lapses), then close. SSRF-guarded; private hosts blocked unless FLEETY_ALLOW_PRIVATE_NET=1.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "send": { "type": "array", "items": { "type": "string" }, "description": "text frames to send before reading" },
                    "max_frames": { "type": "integer", "description": "max frames to receive (default 50)" },
                    "timeout_secs": { "type": "integer", "description": "overall deadline once the handshake completes (default 30)" },
                    "headers": { "type": "object", "description": "extra handshake headers (e.g. Authorization)" },
                    "cookie_jar": { "type": "string", "description": "load cookies from this jar and send them in the handshake Cookie: header" }
                },
                "required": ["url"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let raw = args.get("url").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'url'".to_string())
        })?;
        let url = crate::web::guard_url_ex(raw, &["ws", "wss"])?;

        let max_frames = args
            .get("max_frames")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_RECV_FRAMES);
        let deadline = Duration::from_secs(
            args.get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_RECV_TIMEOUT_SECS),
        );

        // Build the handshake request so we can attach headers + Cookie.
        use tokio_tungstenite::tungstenite::handshake::client::generate_key;
        use tokio_tungstenite::tungstenite::http::Request;
        let mut builder = Request::builder()
            .uri(url.as_str())
            .header(
                "Host",
                url.host_str()
                    .ok_or_else(|| CoreError::Message("url has no host".into()))?,
            )
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key());
        if let Some(hs) = args.get("headers").and_then(Value::as_object) {
            for (k, v) in hs {
                if let Some(s) = v.as_str() {
                    builder = builder.header(k, s);
                }
            }
        }
        if let Some(jar_name) = args.get("cookie_jar").and_then(Value::as_str) {
            let http_url = http_equivalent(&url);
            let handle = crate::web::CookieJarHandle::load(&self.cookies_dir, jar_name, &http_url)?;
            use reqwest::cookie::CookieStore;
            if let Some(header) = handle.jar.cookies(&http_url) {
                if let Ok(s) = header.to_str() {
                    if !s.is_empty() {
                        builder = builder.header("Cookie", s);
                    }
                }
            }
        }
        let request = builder
            .body(())
            .map_err(|e| CoreError::Message(format!("build ws request: {e}")))?;

        let (mut socket, response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| CoreError::Message(format!("ws connect failed: {e}")))?;
        let handshake_status = response.status().as_u16();

        // Send phase.
        let sends: Vec<String> = args
            .get("send")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for frame in &sends {
            socket
                .send(WsMessage::Text(frame.clone()))
                .await
                .map_err(|e| CoreError::Message(format!("ws send failed: {e}")))?;
        }

        // Receive phase, bounded by both max_frames and the deadline.
        let started = Instant::now();
        let mut received: Vec<Value> = Vec::new();
        while received.len() < max_frames {
            let remaining = deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, socket.next()).await {
                Ok(Some(Ok(WsMessage::Text(t)))) => {
                    received.push(json!({ "type": "text", "data": t }))
                }
                Ok(Some(Ok(WsMessage::Binary(b)))) => {
                    use base64::Engine;
                    received.push(json!({
                        "type": "binary",
                        "base64": base64::engine::general_purpose::STANDARD.encode(&b),
                        "size": b.len()
                    }));
                }
                Ok(Some(Ok(WsMessage::Close(frame)))) => {
                    received.push(json!({
                        "type": "close",
                        "code": frame.as_ref().map(|f| u16::from(f.code)),
                        "reason": frame.as_ref().map(|f| f.reason.to_string()),
                    }));
                    break;
                }
                Ok(Some(Ok(_))) => continue, // ping/pong
                Ok(Some(Err(e))) => {
                    return Err(CoreError::Message(format!("ws recv failed: {e}")));
                }
                Ok(None) => break,
                Err(_) => break, // deadline
            }
        }

        let _ = socket.close(None).await;
        Ok(json!({
            "handshake_status": handshake_status,
            "frames": received,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "frames_received": received.len(),
        }))
    }
}

/// Map a `ws://` / `wss://` URL onto its `http://` / `https://` equivalent so a
/// cookie jar can be loaded under the same origin.
fn http_equivalent(url: &reqwest::Url) -> reqwest::Url {
    let scheme = match url.scheme() {
        "wss" => "https",
        "ws" => "http",
        other => other,
    };
    let mut rewritten = url.clone();
    let _ = rewritten.set_scheme(scheme);
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_equivalent_swaps_scheme() {
        let ws = reqwest::Url::parse("ws://example.com/feed").unwrap();
        let mapped = http_equivalent(&ws);
        assert_eq!(mapped.scheme(), "http");
        assert_eq!(mapped.host_str(), Some("example.com"));
        let wss = reqwest::Url::parse("wss://example.com/feed").unwrap();
        assert_eq!(http_equivalent(&wss).scheme(), "https");
    }

    #[tokio::test]
    async fn ws_call_rejects_unsupported_scheme() {
        use agent_core::ToolRegistry;
        let dir = std::env::temp_dir().join(format!("fleety-ws-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(WsCall {
            cookies_dir: dir.clone(),
            workspace: dir.clone(),
        }));
        assert!(registry
            .call("ws_call", json!({ "url": "ftp://example.com" }))
            .await
            .is_err());
        // ws:// to private host should also fail without the allow-private flag.
        std::env::remove_var("FLEETY_ALLOW_PRIVATE_NET");
        assert!(registry
            .call("ws_call", json!({ "url": "ws://127.0.0.1:9999" }))
            .await
            .is_err());
    }
}
