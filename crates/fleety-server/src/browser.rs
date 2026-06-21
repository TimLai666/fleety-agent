//! Browser automation over the Chrome DevTools Protocol (CDP).
//!
//! Connects to a Chrome started with `--remote-debugging-port` (discovered via
//! `GET {base}/json`) and drives it over a WebSocket — no extra dependency
//! (reqwest + tokio-tungstenite are already used). The page is intrusive UI, so
//! prefer screenshots and use sparingly. The CDP framing / page discovery is
//! unit-tested; the live connection follows the logic-tested/live-manual posture.

use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const CDP_TIMEOUT: Duration = Duration::from_secs(30);

/// Register the browser tools.
pub fn register(registry: &mut ToolRegistry) {
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

/// Run one CDP command against the Chrome at `http_base`.
async fn cdp(http_base: &str, method: &str, params: Value) -> Result<Value> {
    let list = reqwest::get(format!("{http_base}/json"))
        .await
        .map_err(|e| CoreError::Provider(format!("cannot reach Chrome at {http_base}: {e}")))?
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("reading Chrome /json failed: {e}")))?;
    let ws_url = parse_ws_url(&list)?;

    let exchange = async {
        let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| CoreError::Provider(format!("CDP websocket connect failed: {e}")))?;
        let (mut tx, mut rx) = ws.split();
        tx.send(WsMessage::Text(cdp_command(1, method, &params)))
            .await
            .map_err(|e| CoreError::Provider(format!("CDP send failed: {e}")))?;
        while let Some(frame) = rx.next().await {
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

    match tokio::time::timeout(CDP_TIMEOUT, exchange).await {
        Ok(inner) => inner,
        Err(_) => Err(CoreError::Provider(format!(
            "CDP '{method}' timed out after {}s",
            CDP_TIMEOUT.as_secs()
        ))),
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
        cdp(&chrome_base(&args), "Page.navigate", json!({ "url": url })).await
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
        cdp(
            &chrome_base(&args),
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
                "properties": { "chrome": { "type": "string" } }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        cdp(&chrome_base(&args), "Page.captureScreenshot", json!({})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
