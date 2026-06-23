//! Browser automation over the Chrome DevTools Protocol (CDP).
//!
//! Connects to a Chrome started with `--remote-debugging-port` (discovered via
//! `GET {base}/json`) and drives it over a WebSocket — no extra dependency
//! (reqwest + tokio-tungstenite are already used). The page is intrusive UI, so
//! prefer screenshots and use sparingly. The CDP framing / page discovery is
//! unit-tested; the live connection follows the logic-tested/live-manual posture.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const CDP_TIMEOUT: Duration = Duration::from_secs(30);

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A persistent CDP session: an open Chrome DevTools websocket + its id counter.
struct Session {
    ws: AsyncMutex<WsStream>,
    next_id: AtomicU64,
}

/// Process-wide registry of open browser sessions (`browser_open`/`browser_close`).
fn sessions() -> &'static Mutex<HashMap<String, Arc<Session>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<Session>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register the browser tools.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(Box::new(BrowserOpen));
    registry.register(Box::new(BrowserClose));
    registry.register(Box::new(BrowserNavigate));
    registry.register(Box::new(BrowserEval));
    registry.register(Box::new(BrowserScreenshot));
}

fn chrome_base(args: &Value) -> String {
    args.get("chrome")
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches('/').to_string())
        .or_else(|| std::env::var("FLEETY_CHROME_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:9222".to_string())
}

/// Pick a debuggable page's WebSocket URL from Chrome's `/json` listing.
fn parse_ws_url(body: &str) -> Result<String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Message(format!("unexpected /json from Chrome: {e}")))?;
    let targets = value
        .as_array()
        .ok_or_else(|| CoreError::Message("Chrome /json was not a list".to_string()))?;
    let pick = |want_page: bool| {
        targets.iter().find_map(|t| {
            let is_page = t.get("type").and_then(Value::as_str) == Some("page");
            if want_page && !is_page {
                return None;
            }
            t.get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .map(String::from)
        })
    };
    pick(true).or_else(|| pick(false)).ok_or_else(|| {
        CoreError::Message(
            "no debuggable page found; start Chrome with --remote-debugging-port".to_string(),
        )
    })
}

fn cdp_command(id: u64, method: &str, params: &Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

/// Parse a CDP frame: `Some(result/err)` if it matches `id`, else `None`.
fn parse_cdp_result(text: &str, id: u64) -> Option<Result<Value>> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(err) = value.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("CDP error");
        return Some(Err(CoreError::Provider(format!("CDP error: {message}"))));
    }
    Some(Ok(value.get("result").cloned().unwrap_or(Value::Null)))
}

async fn with_timeout<F: Future<Output = Result<Value>>>(method: &str, fut: F) -> Result<Value> {
    match tokio::time::timeout(CDP_TIMEOUT, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(CoreError::Provider(format!(
            "CDP '{method}' timed out after {}s",
            CDP_TIMEOUT.as_secs()
        ))),
    }
}

/// Discover a debuggable page's websocket URL from `{http_base}/json`.
async fn discover_ws(http_base: &str) -> Result<String> {
    let list = reqwest::get(format!("{http_base}/json"))
        .await
        .map_err(|e| CoreError::Provider(format!("cannot reach Chrome at {http_base}: {e}")))?
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("reading Chrome /json failed: {e}")))?;
    parse_ws_url(&list)
}

/// Run one CDP command on a fresh connection (no persistent session).
async fn cdp(http_base: &str, method: &str, params: Value) -> Result<Value> {
    let ws_url = discover_ws(http_base).await?;
    let exchange = async {
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| CoreError::Provider(format!("CDP websocket connect failed: {e}")))?;
        ws.send(WsMessage::Text(cdp_command(1, method, &params)))
            .await
            .map_err(|e| CoreError::Provider(format!("CDP send failed: {e}")))?;
        while let Some(frame) = ws.next().await {
            let frame = frame.map_err(|e| CoreError::Provider(format!("CDP read failed: {e}")))?;
            if let Ok(text) = frame.to_text() {
                if let Some(result) = parse_cdp_result(text, 1) {
                    return result;
                }
            }
        }
        Err(CoreError::Provider(
            "CDP connection closed before responding".to_string(),
        ))
    };
    with_timeout(method, exchange).await
}

/// Run one CDP command over an existing persistent session.
async fn cdp_session(session_id: &str, method: &str, params: Value) -> Result<Value> {
    let sess = sessions()
        .lock()
        .map_err(|_| CoreError::Message("browser sessions lock poisoned".to_string()))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| CoreError::Message(format!("no such browser session '{session_id}'")))?;
    let id = sess.next_id.fetch_add(1, Ordering::Relaxed);
    let exchange = async {
        let mut ws = sess.ws.lock().await;
        ws.send(WsMessage::Text(cdp_command(id, method, &params)))
            .await
            .map_err(|e| CoreError::Provider(format!("CDP send failed: {e}")))?;
        while let Some(frame) = ws.next().await {
            let frame = frame.map_err(|e| CoreError::Provider(format!("CDP read failed: {e}")))?;
            if let Ok(text) = frame.to_text() {
                if let Some(result) = parse_cdp_result(text, id) {
                    return result;
                }
            }
        }
        Err(CoreError::Provider(
            "CDP session closed before responding".to_string(),
        ))
    };
    with_timeout(method, exchange).await
}

/// Route a CDP command to a persistent `session` if given, else a fresh connection.
async fn dispatch(args: &Value, method: &str, params: Value) -> Result<Value> {
    if let Some(session) = args.get("session").and_then(Value::as_str) {
        cdp_session(session, method, params).await
    } else {
        cdp(&chrome_base(args), method, params).await
    }
}

struct BrowserNavigate;

#[async_trait]
impl Tool for BrowserNavigate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_navigate".to_string(),
            description:
                "Navigate the connected Chrome to a URL (CDP). Intrusive UI — use sparingly."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "session": { "type": "string", "description": "reuse a browser_open session" },
                    "chrome": { "type": "string", "description": "Chrome devtools http base, default http://127.0.0.1:9222" }
                },
                "required": ["url"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let url = args.get("url").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'url'".to_string())
        })?;
        dispatch(&args, "Page.navigate", json!({ "url": url })).await
    }
}

struct BrowserEval;

#[async_trait]
impl Tool for BrowserEval {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_eval".to_string(),
            description: "Evaluate JavaScript in the connected Chrome page and return the value (CDP Runtime.evaluate).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "expression": { "type": "string" },
                    "session": { "type": "string", "description": "reuse a browser_open session" },
                    "chrome": { "type": "string" }
                },
                "required": ["expression"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let expression = args
            .get("expression")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Message("missing required string argument 'expression'".to_string())
            })?;
        dispatch(
            &args,
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
        )
        .await
    }
}

struct BrowserScreenshot;

#[async_trait]
impl Tool for BrowserScreenshot {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_screenshot".to_string(),
            description: "Capture a screenshot of the connected Chrome page (base64 PNG via CDP). The low-impact way to observe a device's screen.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "reuse a browser_open session" },
                    "chrome": { "type": "string" }
                }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        dispatch(&args, "Page.captureScreenshot", json!({})).await
    }
}

struct BrowserOpen;

#[async_trait]
impl Tool for BrowserOpen {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_open".to_string(),
            description: "Open a persistent browser (CDP) session and return its handle; pass `session` to browser_navigate/eval/screenshot to reuse the connection.".to_string(),
            parameters: json!({ "type": "object", "properties": { "chrome": { "type": "string" } } }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let ws_url = discover_ws(&chrome_base(&args)).await?;
        let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| CoreError::Provider(format!("CDP websocket connect failed: {e}")))?;
        let id = format!("br-{}", uuid::Uuid::new_v4().simple());
        sessions()
            .lock()
            .map_err(|_| CoreError::Message("browser sessions lock poisoned".to_string()))?
            .insert(
                id.clone(),
                Arc::new(Session {
                    ws: AsyncMutex::new(ws),
                    next_id: AtomicU64::new(1),
                }),
            );
        Ok(json!({ "session": id }))
    }
}

struct BrowserClose;

#[async_trait]
impl Tool for BrowserClose {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser_close".to_string(),
            description: "Close a persistent browser session opened with browser_open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "session": { "type": "string" } },
                "required": ["session"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let session = args.get("session").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'session'".to_string())
        })?;
        let closed = sessions()
            .lock()
            .map_err(|_| CoreError::Message("browser sessions lock poisoned".to_string()))?
            .remove(session)
            .is_some();
        Ok(json!({ "session": session, "closed": closed }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    #[test]
    fn parse_ws_url_prefers_a_page() {
        let body = r#"[
            {"type":"background_page","webSocketDebuggerUrl":"ws://bg"},
            {"type":"page","webSocketDebuggerUrl":"ws://page1"},
            {"type":"page","webSocketDebuggerUrl":"ws://page2"}
        ]"#;
        assert_eq!(parse_ws_url(body).expect("url"), "ws://page1");

        let only_other = r#"[{"type":"other","webSocketDebuggerUrl":"ws://x"}]"#;
        assert_eq!(parse_ws_url(only_other).expect("url"), "ws://x");

        assert!(parse_ws_url("[]").is_err());
        assert!(parse_ws_url("not json").is_err());
    }

    #[test]
    fn chrome_base_prefers_trimmed_arg_then_env_then_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        std::env::remove_var("FLEETY_CHROME_URL");

        assert_eq!(chrome_base(&json!({})), "http://127.0.0.1:9222");
        std::env::set_var("FLEETY_CHROME_URL", "http://chrome-from-env:9222");
        assert_eq!(chrome_base(&json!({})), "http://chrome-from-env:9222");
        assert_eq!(
            chrome_base(&json!({ "chrome": "http://chrome-from-arg:9222/" })),
            "http://chrome-from-arg:9222"
        );

        std::env::remove_var("FLEETY_CHROME_URL");
    }

    #[tokio::test]
    async fn unknown_session_errors_and_close_is_idempotent() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        // Using a bogus session handle is rejected (no network involved).
        assert!(registry
            .call(
                "browser_navigate",
                json!({ "url": "https://example.com", "session": "nope" })
            )
            .await
            .is_err());
        // Closing an unknown session is a no-op, not an error.
        let r = registry
            .call("browser_close", json!({ "session": "nope" }))
            .await
            .expect("close");
        assert_eq!(r["closed"], json!(false));
    }

    #[tokio::test]
    async fn browser_tools_validate_required_arguments_without_cdp() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);

        assert!(registry
            .call("browser_navigate", json!({ "session": "s1" }))
            .await
            .is_err());
        assert!(registry
            .call("browser_eval", json!({ "session": "s1" }))
            .await
            .is_err());
        assert!(registry.call("browser_close", json!({})).await.is_err());
        assert!(registry
            .call("browser_screenshot", json!({ "session": "missing" }))
            .await
            .is_err());
    }

    #[test]
    fn cdp_framing() {
        let cmd = cdp_command(1, "Page.navigate", &json!({ "url": "https://x" }));
        let v: Value = serde_json::from_str(&cmd).expect("json");
        assert_eq!(v["id"], json!(1));
        assert_eq!(v["method"], json!("Page.navigate"));

        let ok = r#"{"id":1,"result":{"frameId":"F"}}"#;
        assert_eq!(
            parse_cdp_result(ok, 1).expect("some").expect("ok")["frameId"],
            json!("F")
        );
        let err = r#"{"id":1,"error":{"message":"boom"}}"#;
        assert!(parse_cdp_result(err, 1).expect("some").is_err());
        assert!(parse_cdp_result(r#"{"id":2,"result":{}}"#, 1).is_none());
    }
}
