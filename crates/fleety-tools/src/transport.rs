//! Client transport with WebSocket→SSE fallback, shared by `fleety` and
//! `fleetyd`.
//!
//! Both clients speak the same JSON frames (`ClientMsg`/`ServerMsg`); this module
//! moves the *text* of those frames over whichever transport is reachable, so the
//! callers keep doing their own (de)serialization and message handling. WebSocket
//! is preferred; when it can't connect (e.g. a proxy blocks the upgrade) the
//! client falls back to SSE (downstream) + HTTP POST (upstream), correlated by a
//! session id. The pure URL/mode/parse helpers are unit-tested; the live connect
//! is exercised end-to-end against a real server by the binaries.

use agent_core::{CoreError, Result};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Which transport(s) the client will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// WebSocket first, fall back to SSE+POST (default).
    Auto,
    /// Always use SSE+POST (skip WebSocket).
    ForceSse,
    /// WebSocket only; never fall back.
    WsOnly,
}

/// Resolve the transport mode from env: `FLEETY_FORCE_SSE=1` forces SSE,
/// `FLEETY_DISABLE_SSE=1` disables the fallback; otherwise Auto.
pub fn mode_from_env() -> Mode {
    if std::env::var("FLEETY_FORCE_SSE").as_deref() == Ok("1") {
        Mode::ForceSse
    } else if std::env::var("FLEETY_DISABLE_SSE").as_deref() == Ok("1") {
        Mode::WsOnly
    } else {
        Mode::Auto
    }
}

/// The HTTP(S) base for the SSE+POST endpoints, derived from the agent URL:
/// `ws://` → `http://`, `wss://` → `https://`, an `http(s)://` URL is used as-is.
/// Any trailing slash is trimmed.
pub fn http_base(agent_url: &str) -> String {
    let u = agent_url.trim_end_matches('/');
    if let Some(rest) = u.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = u.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        u.to_string()
    }
}

/// SSE downstream half-open timeout: if no event *or* keep-alive comment arrives
/// within this window the stream is treated as dead and the client reconnects.
fn sse_timeout() -> Duration {
    let secs = std::env::var("FLEETY_SSE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(45);
    Duration::from_secs(secs)
}

/// Extract the payloads of `data:` lines in one SSE event block. Keep-alive
/// comment lines (starting `:`) and other fields are ignored.
fn data_frames(block: &str) -> Vec<String> {
    block
        .lines()
        .filter_map(|l| l.strip_prefix("data:").map(|d| d.trim().to_string()))
        .filter(|d| !d.is_empty())
        .collect()
}

/// A connected client transport, carrying frame *text* (the caller does the
/// JSON). It is a paired [`Sender`] + [`Receiver`]: hold it whole (daemon's
/// single loop) or `split()` it for concurrent send/recv (the CLI's TUI).
pub struct Connection {
    sender: Sender,
    receiver: Receiver,
}

/// Outbound half.
pub enum Sender {
    Ws(futures::stream::SplitSink<Ws, WsMessage>),
    Sse {
        client: reqwest::Client,
        send_url: String,
        token: Option<String>,
    },
}

/// Inbound half.
pub enum Receiver {
    Ws(futures::stream::SplitStream<Ws>),
    Sse(mpsc::UnboundedReceiver<String>),
}

impl Connection {
    /// The next inbound frame text, or `None` when the link is closed/dead.
    pub async fn recv_text(&mut self) -> Option<String> {
        self.receiver.recv_text().await
    }

    /// Send one outbound frame text. An error means the link is gone.
    pub async fn send_text(&mut self, text: String) -> Result<()> {
        self.sender.send_text(text).await
    }

    /// Close the link (best-effort).
    pub async fn close(&mut self) {
        self.sender.close().await;
    }

    /// Split into independent halves for concurrent send + recv.
    pub fn split(self) -> (Sender, Receiver) {
        (self.sender, self.receiver)
    }
}

impl Sender {
    pub async fn send_text(&mut self, text: String) -> Result<()> {
        match self {
            Sender::Ws(tx) => tx
                .send(WsMessage::Text(text))
                .await
                .map_err(|e| CoreError::Provider(format!("websocket send failed: {e}"))),
            Sender::Sse {
                client,
                send_url,
                token,
            } => {
                let mut req = client.post(&*send_url).body(text);
                if let Some(t) = token {
                    req = req.bearer_auth(t);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| CoreError::Provider(format!("sse POST failed: {e}")))?;
                if resp.status().is_success() {
                    Ok(())
                } else {
                    Err(CoreError::Provider(format!(
                        "sse POST rejected: {}",
                        resp.status()
                    )))
                }
            }
        }
    }

    /// Best-effort close. SSE has no close frame; dropping it ends the session.
    pub async fn close(&mut self) {
        if let Sender::Ws(tx) = self {
            let _ = tx.close().await;
        }
    }
}

impl Receiver {
    /// The next inbound frame text, or `None` when the link is closed/dead.
    pub async fn recv_text(&mut self) -> Option<String> {
        match self {
            Receiver::Ws(rx) => loop {
                match rx.next().await {
                    Some(Ok(WsMessage::Text(t))) => return Some(t.to_string()),
                    Some(Ok(WsMessage::Close(_))) | None => return None,
                    Some(Err(_)) => return None,
                    Some(Ok(_)) => continue, // ping/pong/binary
                }
            },
            Receiver::Sse(rx) => rx.recv().await,
        }
    }
}

/// Connect to the agent, honoring the env transport mode: WebSocket first with an
/// SSE+POST fallback (Auto), or one transport only. `token`, when present, is sent
/// as the `Authorization: Bearer` header on the SSE/POST requests.
pub async fn connect(agent_url: &str, token: Option<&str>) -> Result<Connection> {
    match mode_from_env() {
        Mode::WsOnly => connect_ws(agent_url).await,
        Mode::ForceSse => connect_sse(agent_url, token).await,
        Mode::Auto => match connect_ws(agent_url).await {
            Ok(c) => Ok(c),
            Err(e) => {
                tracing::warn!(%e, "websocket connect failed; trying SSE+POST fallback");
                connect_sse(agent_url, token).await
            }
        },
    }
}

async fn connect_ws(agent_url: &str) -> Result<Connection> {
    let (ws, _) = connect_async(agent_url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect (ws) to {agent_url}: {e}")))?;
    let (tx, rx) = ws.split();
    Ok(Connection {
        sender: Sender::Ws(tx),
        receiver: Receiver::Ws(rx),
    })
}

async fn connect_sse(agent_url: &str, token: Option<&str>) -> Result<Connection> {
    let base = http_base(agent_url);
    let session = uuid::Uuid::new_v4().to_string();
    let sse_url = format!("{base}/sse?session={session}");
    let send_url = format!("{base}/send?session={session}");
    let client = reqwest::Client::new();
    let mut req = client.get(&sse_url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("cannot open SSE at {sse_url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(CoreError::Provider(format!(
            "SSE open rejected: {}",
            resp.status()
        )));
    }
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(read_sse(resp, tx));
    tracing::info!(%base, "connected via SSE+POST fallback");
    Ok(Connection {
        sender: Sender::Sse {
            client,
            send_url,
            token: token.map(str::to_string),
        },
        receiver: Receiver::Sse(rx),
    })
}

/// Read the SSE response: split events on blank lines, forward each `data:`
/// payload to `tx`. A read timeout (no event or keep-alive within the window)
/// ends the task, which drops `tx` so `recv_text` returns `None` (reconnect).
async fn read_sse(mut resp: reqwest::Response, tx: mpsc::UnboundedSender<String>) {
    let timeout = sse_timeout();
    let mut buf = String::new();
    loop {
        let chunk = match tokio::time::timeout(timeout, resp.chunk()).await {
            Ok(Ok(Some(bytes))) => bytes,
            _ => break, // timeout (half-open), stream error, or end
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(idx) = buf.find("\n\n") {
            let block: String = buf.drain(..idx + 2).collect();
            for frame in data_frames(&block) {
                if tx.send(frame).is_err() {
                    return; // receiver gone
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_base_derives_from_ws_scheme() {
        assert_eq!(http_base("ws://host:8787"), "http://host:8787");
        assert_eq!(http_base("wss://host:8787"), "https://host:8787");
        assert_eq!(http_base("ws://host:8787/"), "http://host:8787");
        // Already HTTP — used as-is.
        assert_eq!(http_base("http://host:8787"), "http://host:8787");
    }

    #[test]
    fn data_frames_extracts_payloads_and_skips_comments() {
        let block = ": keep-alive\ndata: {\"type\":\"done\"}\n\n";
        assert_eq!(data_frames(block), vec!["{\"type\":\"done\"}".to_string()]);
        // A comment-only block yields nothing.
        assert!(data_frames(": ping\n\n").is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn mode_reflects_env() {
        std::env::remove_var("FLEETY_FORCE_SSE");
        std::env::remove_var("FLEETY_DISABLE_SSE");
        assert_eq!(mode_from_env(), Mode::Auto);
        std::env::set_var("FLEETY_FORCE_SSE", "1");
        assert_eq!(mode_from_env(), Mode::ForceSse);
        std::env::remove_var("FLEETY_FORCE_SSE");
        std::env::set_var("FLEETY_DISABLE_SSE", "1");
        assert_eq!(mode_from_env(), Mode::WsOnly);
        std::env::remove_var("FLEETY_DISABLE_SSE");
    }
}
