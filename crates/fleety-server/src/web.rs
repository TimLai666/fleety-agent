//! HTTP / WebSocket / SSE tools for talking to the outside world.
//!
//! Four tools:
//!
//! * `fetch_url` — read-only GET, simple path
//! * `http_request` — full request/response with per-call knobs: method,
//!   headers, body (string or multipart), timeout, redirect policy, TLS
//!   verification, retry, persistent cookie jar, optional stream-to-file for
//!   big bodies
//! * `ws_call` — one-shot WebSocket: connect, send N frames, receive up to N
//!   frames (with deadline), close (see `websocket.rs`)
//! * `sse_stream` — subscribe to an SSE endpoint, return up to N events or
//!   stream them to a workspace file (see `sse.rs`)
//!
//! SSRF guarded across all of them — only `http`/`https` (or `ws`/`wss`), and
//! loopback / private-network hosts refused unless `FLEETY_ALLOW_PRIVATE_NET=1`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const DEFAULT_MAX_BYTES: usize = 100_000;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_REDIRECTS: usize = 10;

/// Register `fetch_url`, `http_request`, and the streaming siblings. Cookie
/// jars persist under `cookies_dir`; `workspace` is the root that
/// `stream_to_file: <path>` writes into.
pub fn register(registry: &mut ToolRegistry, cookies_dir: &Path, workspace: &Path) {
    registry.register(Box::new(FetchUrl));
    registry.register(Box::new(HttpRequest {
        cookies_dir: cookies_dir.to_path_buf(),
        workspace: workspace.to_path_buf(),
    }));
    registry.register(Box::new(crate::websocket::WsCall {
        cookies_dir: cookies_dir.to_path_buf(),
        workspace: workspace.to_path_buf(),
    }));
    registry.register(Box::new(crate::sse::SseStream {
        cookies_dir: cookies_dir.to_path_buf(),
        workspace: workspace.to_path_buf(),
    }));
}

/// Validate scheme + host (SSRF guard) for an outbound URL. Accepts the
/// extra schemes the WebSocket tool needs via `extra_schemes`.
pub(crate) fn guard_url_ex(raw: &str, extra_schemes: &[&str]) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| CoreError::Message(format!("invalid url '{raw}': {e}")))?;
    let scheme = url.scheme();
    let ok = matches!(scheme, "http" | "https") || extra_schemes.contains(&scheme);
    if !ok {
        return Err(CoreError::Message(format!(
            "unsupported url scheme '{scheme}'; allowed: http/https{}",
            if extra_schemes.is_empty() {
                String::new()
            } else {
                format!("/{}", extra_schemes.join("/"))
            }
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

/// http/https-only version (the common case).
fn guard_url(raw: &str) -> Result<reqwest::Url> {
    guard_url_ex(raw, &[])
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
            if let Some(v4) = v6.to_ipv4() {
                return v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified();
            }
            let seg0 = v6.segments()[0];
            (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
        }
    }
}

/// Whether a host is loopback / private / link-local (blocked by default).
pub(crate) fn is_blocked_host(host: &str) -> bool {
    let h = host
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        if ip_blocked(ip) {
            return true;
        }
    }
    false
}

pub(crate) fn allow_private() -> bool {
    std::env::var("FLEETY_ALLOW_PRIVATE_NET").as_deref() == Ok("1")
}

/// Resolve a workspace-relative path for `stream_to_file`. Refuses absolute /
/// escape paths so a remote server can't trick the agent into writing outside
/// the workspace.
pub(crate) fn resolve_in_workspace(workspace: &Path, rel: &str) -> Result<PathBuf> {
    // Cross-platform: a leading `/` or `\` (or a real OS-absolute path) all
    // count as escape attempts. `Path::is_absolute` on its own misses `/x`
    // when running on Windows.
    let first = rel.chars().next();
    if rel.is_empty()
        || first == Some('/')
        || first == Some('\\')
        || Path::new(rel).is_absolute()
        || rel.split(['/', '\\']).any(|c| c == "..")
    {
        return Err(CoreError::Message(format!(
            "stream_to_file path '{rel}' must be a relative path inside the workspace"
        )));
    }
    Ok(workspace.join(rel))
}

/// Build a reqwest client honouring per-call timeout / redirect / TLS knobs
/// and (optionally) sharing a persistent cookie jar across calls.
pub(crate) fn build_client(
    timeout_secs: u64,
    follow_redirects: usize,
    verify_tls: bool,
    cookie_jar: Option<Arc<reqwest::cookie::Jar>>,
) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .danger_accept_invalid_certs(!verify_tls);
    builder = if follow_redirects == 0 {
        builder.redirect(reqwest::redirect::Policy::none())
    } else {
        builder.redirect(reqwest::redirect::Policy::limited(follow_redirects))
    };
    if let Some(jar) = cookie_jar {
        builder = builder.cookie_provider(jar);
    }
    builder
        .build()
        .map_err(|e| CoreError::Message(format!("http client build failed: {e}")))
}

/// Persistent cookie jar API used by `http_request` / `ws_call` / `sse_stream`.
/// Loads existing cookies from `cookies_dir/<name>.json` (best-effort), hands
/// the agent a live `reqwest::cookie::Jar` to use during the call, and on
/// completion writes the updated jar back to disk.
pub(crate) struct CookieJarHandle {
    pub jar: Arc<reqwest::cookie::Jar>,
    pub url: reqwest::Url,
    pub cookies_dir: PathBuf,
    pub name: String,
}

impl CookieJarHandle {
    pub fn load(cookies_dir: &Path, name: &str, url: &reqwest::Url) -> Result<Self> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(CoreError::Message(format!(
                "invalid cookie_jar name '{name}'"
            )));
        }
        let jar = Arc::new(reqwest::cookie::Jar::default());
        let path = cookies_dir.join(format!("{name}.json"));
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(records) = serde_json::from_str::<Vec<String>>(&text) {
                use reqwest::cookie::CookieStore;
                for cookie in records {
                    let header = cookie
                        .parse::<reqwest::header::HeaderValue>()
                        .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static(""));
                    jar.set_cookies(&mut std::iter::once(&header), url);
                }
            }
        }
        Ok(Self {
            jar,
            url: url.clone(),
            cookies_dir: cookies_dir.to_path_buf(),
            name: name.to_string(),
        })
    }

    pub fn persist(&self) -> Result<()> {
        use reqwest::cookie::CookieStore;
        std::fs::create_dir_all(&self.cookies_dir)
            .map_err(|e| CoreError::Message(format!("cannot create cookies dir: {e}")))?;
        let mut all: Vec<String> = Vec::new();
        if let Some(header) = self.jar.cookies(&self.url) {
            if let Ok(s) = header.to_str() {
                // The combined Cookie header is `k=v; k2=v2; …`; split back into
                // per-pair records so the next load reapplies them individually.
                for pair in s.split(';') {
                    let trimmed = pair.trim();
                    if !trimmed.is_empty() {
                        all.push(trimmed.to_string());
                    }
                }
            }
        }
        let path = self.cookies_dir.join(format!("{}.json", self.name));
        let body = serde_json::to_string_pretty(&all)
            .map_err(|e| CoreError::Message(format!("serialize cookies: {e}")))?;
        std::fs::write(&path, body)
            .map_err(|e| CoreError::Message(format!("write cookies: {e}")))?;
        Ok(())
    }
}

struct FetchUrl;

#[async_trait]
impl Tool for FetchUrl {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch_url".to_string(),
            description: "HTTP GET a public URL and return status + body (read-only; loopback/private-network hosts are blocked). For richer options use `http_request`.".to_string(),
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
        let client = build_client(DEFAULT_TIMEOUT_SECS, 0, true, None)?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Message(format!("request failed: {e}")))?;
        let status = response.status().as_u16();
        let content_type = header_str(&response, reqwest::header::CONTENT_TYPE);
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

struct HttpRequest {
    cookies_dir: PathBuf,
    workspace: PathBuf,
}

#[async_trait]
impl Tool for HttpRequest {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "http_request".to_string(),
            description: "Make an HTTP request (GET/POST/PUT/PATCH/DELETE/HEAD) to a public URL. Supports per-call timeout, redirect policy, TLS verification, retry, multipart upload, a persistent cookie jar shared across calls, and streaming the response body to a file in the workspace instead of returning it inline.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] },
                    "url": { "type": "string" },
                    "headers": { "type": "object" },
                    "body": { "type": "string", "description": "raw request body (string). For binary upload use multipart or base64 inside JSON." },
                    "multipart": {
                        "type": "object",
                        "description": "Assemble a multipart/form-data body. Mutually exclusive with `body`.",
                        "properties": {
                            "fields": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "value": { "type": "string" } }, "required": ["name", "value"] } },
                            "files": { "type": "array", "items": { "type": "object", "properties": { "name": { "type": "string" }, "path": { "type": "string", "description": "workspace-relative file path" }, "mime": { "type": "string" }, "filename": { "type": "string" } }, "required": ["name", "path"] } }
                        }
                    },
                    "max_bytes": { "type": "integer", "description": "cap on returned body bytes (ignored when stream_to_file is set)" },
                    "timeout_secs": { "type": "integer", "description": "per-call timeout (default 30)" },
                    "follow_redirects": { "type": "integer", "description": "max redirects to follow; 0 = none (default 0)" },
                    "verify_tls": { "type": "boolean", "description": "verify the server's TLS certificate (default true)" },
                    "retry": {
                        "type": "object",
                        "description": "retry transient failures (network errors and the listed statuses) with exponential backoff",
                        "properties": {
                            "max": { "type": "integer", "description": "max attempts including the first (default 1)" },
                            "backoff_ms": { "type": "integer", "description": "base backoff between retries (default 200)" },
                            "on_status": { "type": "array", "items": { "type": "integer" }, "description": "HTTP statuses that count as retry-worthy (default [408, 429, 500, 502, 503, 504])" }
                        }
                    },
                    "cookie_jar": { "type": "string", "description": "name of a persistent cookie jar to load before the call and save after. Use the same name across calls to keep a session." },
                    "stream_to_file": { "type": "string", "description": "workspace-relative path; if set, the body is streamed to that file and the response returns size+sha256 instead of the body." }
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

        // Per-call knobs.
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        let follow_redirects = args
            .get("follow_redirects")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(0)
            .min(DEFAULT_REDIRECTS);
        let verify_tls = args
            .get("verify_tls")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let max_bytes = args
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_BYTES);

        // Retry config.
        let retry = args.get("retry").and_then(Value::as_object);
        let max_attempts = retry
            .and_then(|r| r.get("max"))
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let backoff_ms = retry
            .and_then(|r| r.get("backoff_ms"))
            .and_then(Value::as_u64)
            .unwrap_or(200);
        let retry_statuses: Vec<u16> = retry
            .and_then(|r| r.get("on_status"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as u16))
                    .collect()
            })
            .unwrap_or_else(|| vec![408, 429, 500, 502, 503, 504]);

        // Cookie jar.
        let cookie_handle = match args.get("cookie_jar").and_then(Value::as_str) {
            Some(name) => Some(CookieJarHandle::load(&self.cookies_dir, name, &url)?),
            None => None,
        };
        let jar_for_client = cookie_handle.as_ref().map(|h| Arc::clone(&h.jar));
        let client = build_client(timeout_secs, follow_redirects, verify_tls, jar_for_client)?;

        let stream_to_file = match args.get("stream_to_file").and_then(Value::as_str) {
            Some(rel) => Some(resolve_in_workspace(&self.workspace, rel)?),
            None => None,
        };

        // Body: at most one of `body` / `multipart`.
        if args.get("body").is_some() && args.get("multipart").is_some() {
            return Err(CoreError::Message(
                "pass at most one of `body` or `multipart`, not both".to_string(),
            ));
        }
        let multipart_spec = args.get("multipart").cloned();
        let body_str = args.get("body").and_then(Value::as_str).map(String::from);
        let headers = args.get("headers").and_then(Value::as_object).cloned();

        // Issue the request, with retries on transient failures. `Form` isn't
        // `Clone`, so on a retry we rebuild it from `multipart_spec`.
        let mut attempt = 0;
        #[allow(unused_assignments)]
        let mut last_err: Option<CoreError> = None;
        loop {
            attempt += 1;
            let mut request = client.request(method.clone(), url.clone());
            if let Some(hs) = &headers {
                for (key, value) in hs {
                    if let Some(v) = value.as_str() {
                        request = request.header(key, v);
                    }
                }
            }
            if let Some(spec) = &multipart_spec {
                let form = build_multipart(spec, &self.workspace).await?;
                request = request.multipart(form);
            } else if let Some(b) = &body_str {
                request = request.body(b.clone());
            }
            match request.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    if attempt < max_attempts && retry_statuses.contains(&status) {
                        tokio::time::sleep(Duration::from_millis(
                            backoff_ms.saturating_mul(1 << (attempt.saturating_sub(1))),
                        ))
                        .await;
                        continue;
                    }
                    let result = match &stream_to_file {
                        Some(path) => write_to_file_response(response, path).await?,
                        None => inline_response(response, max_bytes).await?,
                    };
                    if let Some(handle) = &cookie_handle {
                        handle.persist()?;
                    }
                    return Ok(result);
                }
                Err(e) => {
                    last_err = Some(CoreError::Message(format!("request failed: {e}")));
                    if attempt < max_attempts {
                        tokio::time::sleep(Duration::from_millis(
                            backoff_ms.saturating_mul(1 << (attempt.saturating_sub(1))),
                        ))
                        .await;
                        continue;
                    }
                    break;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| CoreError::Message("request failed".to_string())))
    }
}

async fn build_multipart(spec: &Value, workspace: &Path) -> Result<reqwest::multipart::Form> {
    let mut form = reqwest::multipart::Form::new();
    if let Some(fields) = spec.get("fields").and_then(Value::as_array) {
        for field in fields {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Message("multipart field missing 'name'".into()))?;
            let value = field
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Message("multipart field missing 'value'".into()))?;
            form = form.text(name.to_string(), value.to_string());
        }
    }
    if let Some(files) = spec.get("files").and_then(Value::as_array) {
        for file in files {
            let name = file
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Message("multipart file missing 'name'".into()))?;
            let rel = file
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Message("multipart file missing 'path'".into()))?;
            let resolved = resolve_in_workspace(workspace, rel)?;
            let bytes = tokio::fs::read(&resolved).await.map_err(|e| {
                CoreError::Message(format!("cannot read multipart file '{rel}': {e}"))
            })?;
            let filename = file
                .get("filename")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| {
                    resolved
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(String::from)
                        .unwrap_or_else(|| "upload".to_string())
                });
            let mime = file
                .get("mime")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();
            let part = reqwest::multipart::Part::bytes(bytes)
                .file_name(filename)
                .mime_str(&mime)
                .map_err(|e| CoreError::Message(format!("bad multipart mime: {e}")))?;
            form = form.part(name.to_string(), part);
        }
    }
    Ok(form)
}

async fn inline_response(response: reqwest::Response, max_bytes: usize) -> Result<Value> {
    let status = response.status().as_u16();
    let content_type = header_str(&response, reqwest::header::CONTENT_TYPE);
    let headers_map = collect_headers(&response);
    let full = response
        .text()
        .await
        .map_err(|e| CoreError::Message(format!("reading body failed: {e}")))?;
    let truncated = full.chars().count() > max_bytes;
    let body: String = full.chars().take(max_bytes).collect();
    Ok(json!({
        "status": status,
        "content_type": content_type,
        "headers": headers_map,
        "truncated": truncated,
        "body": body
    }))
}

async fn write_to_file_response(response: reqwest::Response, dest: &Path) -> Result<Value> {
    use futures::StreamExt;
    use sha2::{Digest, Sha256};
    let status = response.status().as_u16();
    let content_type = header_str(&response, reqwest::header::CONTENT_TYPE);
    let headers_map = collect_headers(&response);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| CoreError::Message(format!("create parent failed: {e}")))?;
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| CoreError::Message(format!("create file failed: {e}")))?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let started = Instant::now();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| CoreError::Message(format!("stream chunk failed: {e}")))?;
        hasher.update(&bytes);
        total += bytes.len() as u64;
        file.write_all(&bytes)
            .await
            .map_err(|e| CoreError::Message(format!("write chunk failed: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| CoreError::Message(format!("flush failed: {e}")))?;
    let sha256: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(json!({
        "status": status,
        "content_type": content_type,
        "headers": headers_map,
        "bytes_written": total,
        "sha256": sha256,
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "path": dest.to_string_lossy()
    }))
}

pub(crate) fn header_str(
    response: &reqwest::Response,
    name: reqwest::header::HeaderName,
) -> String {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

pub(crate) fn collect_headers(response: &reqwest::Response) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in response.headers() {
        if let Ok(s) = v.to_str() {
            map.insert(k.as_str().to_string(), Value::String(s.to_string()));
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("fleety-web-{}", uuid::Uuid::new_v4()));
        let cookies = base.join("cookies");
        let workspace = base.join("workspace");
        std::fs::create_dir_all(&cookies).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        (cookies, workspace)
    }

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
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "0.0.0.0",
            "127.0.0.1.",
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
        let (cookies, ws) = dirs();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &cookies, &ws);
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
    async fn http_request_validates_method_and_host_and_body_xor_multipart() {
        let (cookies, ws) = dirs();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &cookies, &ws);
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
        // body + multipart -> rejected
        assert!(registry
            .call(
                "http_request",
                json!({
                    "method": "POST",
                    "url": "https://example.com",
                    "body": "x",
                    "multipart": { "fields": [] }
                })
            )
            .await
            .is_err());
    }

    #[test]
    fn cookie_jar_handle_persists_and_reloads() {
        let (cookies_dir, _) = dirs();
        let url = reqwest::Url::parse("https://example.com").unwrap();
        let handle = CookieJarHandle::load(&cookies_dir, "session1", &url).expect("load empty");
        use reqwest::cookie::CookieStore;
        let v = reqwest::header::HeaderValue::from_static("sid=abc; Path=/");
        handle.jar.set_cookies(&mut std::iter::once(&v), &url);
        handle.persist().expect("persist");
        // Reload from disk and assert the cookie survives.
        let reloaded = CookieJarHandle::load(&cookies_dir, "session1", &url).expect("reload");
        let dumped = reloaded.jar.cookies(&url).expect("has cookies");
        assert!(dumped.to_str().unwrap().contains("sid=abc"));
    }

    #[test]
    fn resolve_in_workspace_rejects_escape() {
        let (_cookies, ws) = dirs();
        assert!(resolve_in_workspace(&ws, "good.txt").is_ok());
        assert!(resolve_in_workspace(&ws, "").is_err());
        assert!(resolve_in_workspace(&ws, "../escape").is_err());
        assert!(resolve_in_workspace(&ws, "/etc/passwd").is_err());
    }
}
