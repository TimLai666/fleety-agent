//! Browser automation over the Chrome DevTools Protocol (CDP).
//!
//! Shared so the same tools run on the server and on any device via fleetyd —
//! `device_exec(device="laptop", tool="browser_screenshot")` drives the laptop's
//! browser. The persistent-session map is process-wide, so a `browser_open` on a
//! device lives in that device's daemon and later `browser_*` calls routed to it
//! reuse the same connection.
//!
//! Connects to a Chrome started with `--remote-debugging-port` (discovered via
//! `GET {base}/json`). When the local endpoint is down, [`crate::chrome`]
//! auto-provisions one (detect → launch → install/download). The CDP framing /
//! page discovery is unit-tested; the live connection follows the
//! logic-tested/live-manual posture.

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

/// Register the browser (CDP) tools.
pub fn register_browser(registry: &mut ToolRegistry) {
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
    crate::chrome::ensure_chrome(http_base).await?;
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
                "Navigate the connected Chrome to a URL (CDP). Runs on the device you target \
                 (bare = server; via device_exec = that device). Intrusive UI — use sparingly."
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
            description: "Open a persistent browser (CDP) session and return its handle; pass `session` to browser_navigate/eval/screenshot to reuse the connection. Auto-provisions a local Chrome if none is running.".to_string(),
            parameters: json!({ "type": "object", "properties": { "chrome": { "type": "string" } } }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let base = chrome_base(&args);
        crate::chrome::ensure_chrome(&base).await?;
        let ws_url = discover_ws(&base).await?;
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use tokio_tungstenite::tungstenite::{accept, Message};

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn start_cdp_ws(methods: Vec<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind cdp ws");
        let addr = listener.local_addr().expect("cdp ws addr");
        thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept cdp ws");
            let mut ws = accept(stream).expect("accept websocket");
            for expected_method in methods {
                let frame = ws.read().expect("cdp frame");
                let text = frame.to_text().expect("cdp text");
                let req: Value = serde_json::from_str(text).expect("cdp json");
                assert_eq!(
                    req.get("method").and_then(Value::as_str),
                    Some(expected_method)
                );
                let id = req.get("id").and_then(Value::as_u64).expect("cdp id");
                let result = match expected_method {
                    "Runtime.evaluate" => {
                        json!({ "result": { "type": "number", "value": 42 } })
                    }
                    "Page.captureScreenshot" => json!({ "data": "png-base64" }),
                    "Page.navigate" => json!({ "frameId": "frame-1" }),
                    _ => json!({}),
                };
                ws.send(Message::Text(
                    json!({ "id": id, "result": result }).to_string(),
                ))
                .expect("cdp response");
            }
            let _ = ws.close(None);
        });
        format!("ws://{addr}")
    }

    fn start_chrome_json(ws_url: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind chrome http");
        let addr = listener.local_addr().expect("chrome http addr");
        thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept chrome json");
                let mut buf = [0_u8; 1024];
                let n = stream.read(&mut buf).expect("read chrome request");
                let request = String::from_utf8_lossy(&buf[..n]);
                let body = if request.contains(" /json/version ") {
                    json!({ "Browser": "Fake Chrome" }).to_string()
                } else {
                    json!([
                        {
                            "type": "page",
                            "webSocketDebuggerUrl": ws_url
                        }
                    ])
                    .to_string()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .and_then(|_| stream.write_all(body.as_bytes()))
                    .expect("chrome json response");
            }
        });
        format!("http://{addr}")
    }

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
        register_browser(&mut registry);
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
        register_browser(&mut registry);

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

    #[tokio::test]
    async fn browser_open_reuses_fake_cdp_session_for_eval_and_screenshot() {
        let cdp_ws = start_cdp_ws(vec!["Runtime.evaluate", "Page.captureScreenshot"]);
        let chrome = start_chrome_json(cdp_ws);
        let mut registry = ToolRegistry::new();
        register_browser(&mut registry);

        let opened = registry
            .call("browser_open", json!({ "chrome": chrome }))
            .await
            .expect("browser open");
        let session = opened["session"].as_str().expect("session");

        let eval = registry
            .call(
                "browser_eval",
                json!({ "session": session, "expression": "21 + 21" }),
            )
            .await
            .expect("eval");
        assert_eq!(eval["result"]["value"], json!(42));

        let screenshot = registry
            .call("browser_screenshot", json!({ "session": session }))
            .await
            .expect("screenshot");
        assert_eq!(screenshot["data"], json!("png-base64"));

        let closed = registry
            .call("browser_close", json!({ "session": session }))
            .await
            .expect("close");
        assert_eq!(closed["closed"], json!(true));
    }

    #[tokio::test]
    async fn browser_navigate_can_use_one_shot_fake_cdp_connection() {
        let cdp_ws = start_cdp_ws(vec!["Page.navigate"]);
        let chrome = start_chrome_json(cdp_ws);
        let mut registry = ToolRegistry::new();
        register_browser(&mut registry);

        let result = registry
            .call(
                "browser_navigate",
                json!({ "chrome": chrome, "url": "https://example.com" }),
            )
            .await
            .expect("navigate");
        assert_eq!(result["frameId"], json!("frame-1"));
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
