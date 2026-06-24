//! `sse_stream`: subscribe to a Server-Sent Events endpoint.
//!
//! Connects with `Accept: text/event-stream`, parses each `data:` event into a
//! string, and returns either:
//!
//! - the first `max_events` events inline (default), or
//! - streams each event as a JSON Lines record into a workspace file when
//!   `stream_to_file` is set (so long subscriptions don't blow the response).
//!
//! Deadlined by `timeout_secs`. SSRF-guarded same as http_request.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolSpec};

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_EVENTS: usize = 50;

pub struct SseStream {
    pub cookies_dir: PathBuf,
    pub workspace: PathBuf,
}

#[async_trait]
impl Tool for SseStream {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sse_stream".to_string(),
            description: "Subscribe to a Server-Sent Events (text/event-stream) endpoint and return up to `max_events` events, or stream them to a workspace file. Deadlined by `timeout_secs`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "max_events": { "type": "integer", "description": "max events to collect inline (default 50; ignored when stream_to_file is set)" },
                    "timeout_secs": { "type": "integer", "description": "overall deadline once connected (default 30)" },
                    "headers": { "type": "object", "description": "extra request headers" },
                    "cookie_jar": { "type": "string", "description": "persistent cookie jar to load" },
                    "verify_tls": { "type": "boolean", "description": "verify the server's TLS certificate (default true)" },
                    "stream_to_file": { "type": "string", "description": "workspace-relative file; one JSON event per line (JSONL) instead of inline" }
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
        let url = crate::web::guard_url_ex(raw, &[])?;
        let max_events = args
            .get("max_events")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_EVENTS);
        let deadline = Duration::from_secs(
            args.get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_TIMEOUT_SECS),
        );
        let verify_tls = args
            .get("verify_tls")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        // Cookie jar.
        let cookie_handle = match args.get("cookie_jar").and_then(Value::as_str) {
            Some(name) => Some(crate::web::CookieJarHandle::load(
                &self.cookies_dir,
                name,
                &url,
            )?),
            None => None,
        };
        let jar = cookie_handle.as_ref().map(|h| Arc::clone(&h.jar));
        // Use a generous timeout on the client itself; the deadline below
        // controls the receive loop independently.
        let client = crate::web::build_client(deadline.as_secs().max(60), 0, verify_tls, jar)?;

        let mut request = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(hs) = args.get("headers").and_then(Value::as_object) {
            for (k, v) in hs {
                if let Some(s) = v.as_str() {
                    request = request.header(k, s);
                }
            }
        }
        let response = request
            .send()
            .await
            .map_err(|e| CoreError::Message(format!("sse connect failed: {e}")))?;
        let status = response.status().as_u16();
        let content_type = crate::web::header_str(&response, reqwest::header::CONTENT_TYPE);
        if let Some(handle) = &cookie_handle {
            handle.persist()?;
        }
        if !response.status().is_success() {
            return Err(CoreError::Message(format!(
                "sse endpoint returned HTTP {status}; content-type={content_type}"
            )));
        }

        let stream_to_file = match args.get("stream_to_file").and_then(Value::as_str) {
            Some(rel) => Some(crate::web::resolve_in_workspace(&self.workspace, rel)?),
            None => None,
        };

        use futures::StreamExt;
        let mut byte_stream = response.bytes_stream();
        let started = Instant::now();
        let mut buf = String::new();
        let mut events: Vec<Value> = Vec::new();
        let mut events_written: u64 = 0;

        // If streaming to disk, open the file once and append per-event JSONL.
        let mut file = if let Some(path) = &stream_to_file {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| CoreError::Message(format!("mkdir failed: {e}")))?;
            }
            Some(
                tokio::fs::File::create(path)
                    .await
                    .map_err(|e| CoreError::Message(format!("create file failed: {e}")))?,
            )
        } else {
            None
        };

        loop {
            let remaining = deadline.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            let next = tokio::time::timeout(remaining, byte_stream.next()).await;
            let chunk = match next {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => {
                    return Err(CoreError::Message(format!("sse read failed: {e}")));
                }
                Ok(None) => break,
                Err(_) => break, // deadline
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // SSE frames are separated by a blank line; data: lines accumulate.
            while let Some(idx) = buf.find("\n\n").or_else(|| buf.find("\r\n\r\n")) {
                let raw: String = buf.drain(..idx).collect();
                // Drain the blank-line separator.
                if buf.starts_with("\r\n\r\n") {
                    buf.drain(..4);
                } else {
                    buf.drain(..2);
                }
                let event = parse_sse_event(&raw);
                if let Some(file) = file.as_mut() {
                    let line = event.to_string();
                    file.write_all(line.as_bytes())
                        .await
                        .map_err(|e| CoreError::Message(format!("write event: {e}")))?;
                    file.write_all(b"\n")
                        .await
                        .map_err(|e| CoreError::Message(format!("write nl: {e}")))?;
                    events_written += 1;
                } else {
                    events.push(event);
                    if events.len() >= max_events {
                        break;
                    }
                }
            }
            if file.is_none() && events.len() >= max_events {
                break;
            }
        }
        if let Some(file) = file.as_mut() {
            file.flush()
                .await
                .map_err(|e| CoreError::Message(format!("flush: {e}")))?;
        }

        if let Some(path) = &stream_to_file {
            Ok(json!({
                "status": status,
                "content_type": content_type,
                "events_written": events_written,
                "elapsed_ms": started.elapsed().as_millis() as u64,
                "path": path.to_string_lossy(),
            }))
        } else {
            Ok(json!({
                "status": status,
                "content_type": content_type,
                "events": events,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }))
        }
    }
}

/// Parse one SSE frame's lines into `{ event, id, data, retry }`. Lines that
/// don't match the spec are ignored; the `data` field joins multiple `data:`
/// lines with `\n` per the SSE spec.
fn parse_sse_event(raw: &str) -> Value {
    let mut event_type: Option<String> = None;
    let mut id: Option<String> = None;
    let mut retry_ms: Option<u64> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in raw.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some((field, value)) = line.split_once(':') {
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => event_type = Some(value.to_string()),
                "data" => data_lines.push(value),
                "id" => id = Some(value.to_string()),
                "retry" => retry_ms = value.parse::<u64>().ok(),
                _ => {}
            }
        }
    }
    let data = data_lines.join("\n");
    json!({
        "event": event_type.unwrap_or_else(|| "message".to_string()),
        "id": id,
        "data": data,
        "retry_ms": retry_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_event_handles_multiline_data_and_named_event() {
        let raw = "event: update\nid: 42\ndata: line one\ndata: line two\nretry: 2500";
        let ev = parse_sse_event(raw);
        assert_eq!(ev["event"], json!("update"));
        assert_eq!(ev["id"], json!("42"));
        assert_eq!(ev["data"], json!("line one\nline two"));
        assert_eq!(ev["retry_ms"], json!(2500));
    }

    #[test]
    fn parse_event_defaults_to_message() {
        let ev = parse_sse_event("data: hello");
        assert_eq!(ev["event"], json!("message"));
        assert_eq!(ev["data"], json!("hello"));
    }

    #[test]
    fn parse_event_ignores_comments_and_blank_lines() {
        let ev = parse_sse_event(": ignored\n\ndata: x\n: also ignored");
        assert_eq!(ev["data"], json!("x"));
    }
}
