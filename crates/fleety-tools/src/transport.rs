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

/// WebSocket read deadline (ws-liveness), armed only once the server has
/// pinged: no frame of any kind within this window means the link is dead.
/// Keep it at least twice the server's `FLEETY_WS_PING_SECS`.
fn ws_timeout() -> Duration {
    let secs = std::env::var("FLEETY_WS_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(60);
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
    Ws(WsReceiver),
    Sse(mpsc::UnboundedReceiver<String>),
}

/// The WebSocket inbound half with its ping-adaptive read deadline
/// (ws-liveness): the deadline arms only after the first Ping frame from the
/// server, so a connection to an older server that never pings behaves exactly
/// as before. Once armed, any frame resets the window; a window with no frames
/// at all reports the link as dead (`recv_text` → `None`), which sends fleetyd
/// through its normal backoff reconnect and the CLI through its link-closed
/// handling.
pub struct WsReceiver {
    rx: futures::stream::SplitStream<Ws>,
    deadline: Duration,
    armed: bool,
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
    /// The next inbound frame text, or `None` when the link is closed/dead
    /// (including a WebSocket whose armed read deadline elapsed with no frames).
    pub async fn recv_text(&mut self) -> Option<String> {
        match self {
            Receiver::Ws(ws) => loop {
                // Armed: every wait is bounded; any frame (the server's pings
                // included) starts a fresh window. Unarmed: wait indefinitely,
                // exactly the pre-liveness behavior.
                let next = if ws.armed {
                    match tokio::time::timeout(ws.deadline, ws.rx.next()).await {
                        Ok(next) => next,
                        Err(_) => {
                            tracing::warn!(
                                deadline_secs = ws.deadline.as_secs(),
                                "websocket read deadline elapsed with no frames; treating link as dead"
                            );
                            return None;
                        }
                    }
                } else {
                    ws.rx.next().await
                };
                match next {
                    Some(Ok(WsMessage::Text(t))) => return Some(t.to_string()),
                    Some(Ok(WsMessage::Close(_))) | None => return None,
                    Some(Err(_)) => return None,
                    Some(Ok(WsMessage::Ping(_))) => {
                        // The server pings: it supports liveness, so arm the
                        // read deadline (tungstenite answers the ping itself).
                        ws.armed = true;
                        continue;
                    }
                    Some(Ok(_)) => continue, // pong/binary
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
                let error = redact_urls_in_text(&e.to_string());
                let endpoint = redact_endpoint(agent_url);
                tracing::warn!(%error, %endpoint, "websocket connect failed; trying SSE+POST fallback");
                connect_sse(agent_url, token).await
            }
        },
    }
}

/// Render an endpoint without credentials, fragment, or query values. This is
/// the only form safe for human output, diagnostics, and tracing.
pub fn redact_endpoint(value: &str) -> String {
    let Ok(url) = reqwest::Url::parse(value) else {
        return "<invalid endpoint>".to_string();
    };
    if !matches!(url.scheme(), "ws" | "wss" | "http" | "https") || url.host_str().is_none() {
        return "<invalid endpoint>".to_string();
    }
    let Some((scheme, tail)) = value.split_once("://") else {
        return "<invalid endpoint>".to_string();
    };
    let authority_end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
    let authority = &tail[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let suffix = &tail[authority_end..];
    let without_fragment = suffix.split('#').next().unwrap_or_default();
    let (path, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, None), |(path, query)| {
            (path, Some(query))
        });
    let mut redacted = format!("{scheme}://{host}{path}");
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        let query = query
            .split('&')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let key = part.split_once('=').map(|(key, _)| key).unwrap_or(part);
                format!("{key}=<redacted>")
            })
            .collect::<Vec<_>>()
            .join("&");
        if !query.is_empty() {
            redacted.push('?');
            redacted.push_str(&query);
        }
    }
    redacted
}

/// Redact every URL embedded in a transport or HTTP error string.
///
/// ```
/// use fleety_tools::transport::redact_urls_in_text;
///
/// assert_eq!(
///     redact_urls_in_text(
///         "OAuth failed (HtTpS://user:pass@example.test/Case?AccessToken=secret#fragment)."
///     ),
///     "OAuth failed (HtTpS://example.test/Case?AccessToken=<redacted>)."
/// );
/// ```
pub fn redact_urls_in_text(value: &str) -> String {
    const SCHEMES: [&str; 4] = ["ws://", "wss://", "http://", "https://"];
    let mut rendered = String::with_capacity(value.len());
    let mut offset = 0;
    while offset < value.len() {
        let next = value[offset..].char_indices().find_map(|(relative, _)| {
            let start = offset + relative;
            let has_scheme = SCHEMES.iter().any(|scheme| {
                value
                    .get(start..start + scheme.len())
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(scheme))
            });
            (has_scheme && is_url_scheme_boundary(value, start)).then_some(start)
        });
        let Some(start) = next else {
            rendered.push_str(&value[offset..]);
            break;
        };
        rendered.push_str(&value[offset..start]);
        let token_end = value[start..]
            .char_indices()
            .find_map(|(index, ch)| {
                (index > 0
                    && (ch.is_whitespace()
                        || ch.is_control()
                        || matches!(ch, '\'' | '"' | '<' | '>')))
                .then_some(start + index)
            })
            .unwrap_or(value.len());
        let end = trim_sentence_punctuation(value, start, token_end);
        let candidate = &value[start..end];
        if reqwest::Url::parse(candidate).is_ok() {
            rendered.push_str(&redact_endpoint(candidate));
        } else {
            rendered.push_str("<invalid endpoint>");
        }
        offset = end;
    }
    rendered
}

fn is_url_scheme_boundary(value: &str, start: usize) -> bool {
    start == 0
        || value[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_alphanumeric() && !matches!(ch, '+' | '-' | '.'))
}

fn trim_sentence_punctuation(value: &str, start: usize, mut end: usize) -> usize {
    loop {
        let candidate = &value[start..end];
        let Some((index, last)) = candidate.char_indices().next_back() else {
            return end;
        };
        let is_unmatched_closer = match last {
            ')' => candidate.matches(')').count() > candidate.matches('(').count(),
            ']' => candidate.matches(']').count() > candidate.matches('[').count(),
            '}' => candidate.matches('}').count() > candidate.matches('{').count(),
            _ => false,
        };
        if is_unmatched_closer || matches!(last, ',' | '.' | ';' | ':' | '!' | '?') {
            end = start + index;
        } else {
            return end;
        }
    }
}

/// Presentation boundary for direct binary output. URLs are redacted first,
/// then terminal controls are made visible while semantic line breaks remain.
/// Machine consumers should use the redacted rendered value instead of this
/// human-only form.
pub fn terminal_safe_multiline(value: &str) -> String {
    let redacted = redact_urls_in_text(value);
    let mut safe = String::with_capacity(redacted.len());
    for ch in redacted.chars() {
        match ch {
            '\n' => safe.push('\n'),
            '\r' => safe.push_str("\\r"),
            '\t' => safe.push_str("\\t"),
            control if control.is_control() => {
                safe.push_str(&format!("\\u{{{:x}}}", control as u32));
            }
            printable => safe.push(printable),
        }
    }
    safe
}

async fn connect_ws(agent_url: &str) -> Result<Connection> {
    // Deliberately NOT CoreError::Provider: that variant's remediation says to
    // check the model endpoint/key, which is misleading for a connection-layer
    // failure (the most common first-run error is simply "server not running").
    let endpoint = redact_endpoint(agent_url);
    let (ws, _) = connect_async(agent_url).await.map_err(|e| {
        let error = redact_urls_in_text(&e.to_string());
        CoreError::Message(format!(
            "cannot connect (ws) to {endpoint}: {error} — is the Fleety server running at this \
             address? (CLI: save the right URL with `fleety init <ws-url>`)"
        ))
    })?;
    let (tx, rx) = ws.split();
    Ok(Connection {
        sender: Sender::Ws(tx),
        receiver: Receiver::Ws(WsReceiver {
            rx,
            deadline: ws_timeout(),
            armed: false,
        }),
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
    let display_sse_url = redact_endpoint(&sse_url);
    let resp = req.send().await.map_err(|e| {
        let error = redact_urls_in_text(&e.to_string());
        CoreError::Message(format!(
            "cannot open SSE at {display_sse_url}: {error} — is the Fleety server running at this \
             address? (CLI: save the right URL with `fleety init <ws-url>`)"
        ))
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let hint = if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            " — the server requires auth; enroll this device with `fleety pair <code>`"
        } else {
            ""
        };
        return Err(CoreError::Message(format!(
            "SSE open rejected: {status}{hint}"
        )));
    }
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(read_sse(resp, tx));
    let endpoint = redact_endpoint(&base);
    tracing::info!(%endpoint, "connected via SSE+POST fallback");
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

    /// A fake WebSocket server for the read-deadline tests: accepts one client,
    /// optionally sends a single Ping, then holds the socket open in silence.
    async fn silent_ws_server(ping_first: bool) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                    if ping_first {
                        let _ = ws.send(WsMessage::Ping(Vec::new())).await;
                    }
                    // Stay silent, holding the socket open well past any
                    // deadline the test uses (the task dies with the test).
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(ws);
                }
            }
        });
        addr
    }

    /// Wrap a connected client socket in a `Receiver` with a short deadline
    /// (bypassing env so parallel tests don't race on process globals).
    async fn ws_receiver(addr: std::net::SocketAddr, deadline: Duration) -> Receiver {
        let (ws, _) = connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (_tx, rx) = ws.split();
        Receiver::Ws(WsReceiver {
            rx,
            deadline,
            armed: false,
        })
    }

    /// ws-liveness (spec: armed deadline detects a dead link): after the
    /// server's first Ping, a window with no frames at all makes `recv_text`
    /// report the link as ended (`None`) instead of waiting forever.
    #[tokio::test]
    async fn ws_read_deadline_arms_after_first_ping() {
        let addr = silent_ws_server(true).await;
        let mut receiver = ws_receiver(addr, Duration::from_millis(200)).await;
        let got = tokio::time::timeout(Duration::from_secs(3), receiver.recv_text())
            .await
            .expect("armed deadline must fire well within 3s");
        assert_eq!(got, None, "silence after a ping is a dead link");
    }

    /// ws-liveness (spec: never-pinged connection is unaffected): against a
    /// server that never pings (an older release), no deadline arms — recv
    /// keeps waiting far past the deadline window with no false positive.
    #[tokio::test]
    async fn ws_read_deadline_stays_unarmed_without_ping() {
        let addr = silent_ws_server(false).await;
        let mut receiver = ws_receiver(addr, Duration::from_millis(200)).await;
        let waited = tokio::time::timeout(Duration::from_millis(800), receiver.recv_text()).await;
        assert!(
            waited.is_err(),
            "no ping was ever seen, so recv must still be waiting (got {waited:?})"
        );
    }

    #[test]
    fn endpoint_redaction_removes_credentials_query_values_and_fragments() {
        let redacted = redact_endpoint(
            "wss://alice:USERINFOSECRET@example.test/socket?token=QUERYSECRET&mode=fast#FRAGMENTSECRET",
        );

        assert_eq!(
            redacted,
            "wss://example.test/socket?token=<redacted>&mode=<redacted>"
        );
        for secret in ["alice", "USERINFOSECRET", "QUERYSECRET", "FRAGMENTSECRET"] {
            assert!(
                !redacted.contains(secret),
                "redacted endpoint leaked {secret}"
            );
        }
    }

    #[test]
    fn text_redaction_sanitizes_every_embedded_endpoint() {
        let redacted = redact_urls_in_text(
            "failed ws://user:PASS@one.test/a?key=ONE, fallback https://two.test/b?q=TWO#THREE",
        );

        for secret in ["user", "PASS", "ONE", "TWO", "THREE"] {
            assert!(!redacted.contains(secret), "redacted text leaked {secret}");
        }
        assert!(redacted.contains("ws://one.test/a?key=<redacted>"));
        assert!(redacted.contains("https://two.test/b?q=<redacted>"));
    }

    #[test]
    fn text_redaction_matches_schemes_case_insensitively_without_rewriting_url_text() {
        let redacted = redact_urls_in_text(
            "OAuth failed (HTTPS://alice:PASS@example.test/CasePath?AccessToken=SECRET#FRAGMENT).",
        );

        assert_eq!(
            redacted,
            "OAuth failed (HTTPS://example.test/CasePath?AccessToken=<redacted>)."
        );
        for secret in ["alice", "PASS", "SECRET", "FRAGMENT"] {
            assert!(!redacted.contains(secret), "redacted text leaked {secret}");
        }
    }

    #[test]
    fn text_redaction_preserves_brackets_and_text_between_multiple_urls() {
        let redacted = redact_urls_in_text(
            "backend [WsS://alice:PASSWORD@one.test/A_(B)?FirstKey=ONE], then {hTtP://two.test/C?SecondKey=TWO}!",
        );

        assert_eq!(
            redacted,
            "backend [WsS://one.test/A_(B)?FirstKey=<redacted>], then {hTtP://two.test/C?SecondKey=<redacted>}!"
        );
        for secret in ["alice", "PASSWORD", "ONE", "TWO"] {
            assert!(!redacted.contains(secret), "redacted text leaked {secret}");
        }
    }

    #[test]
    fn text_redaction_respects_controls_and_scheme_boundaries() {
        let input = "prefixhttps://untouched.test/Case?Key=VALUE\u{1b} HTTPS://u:P@safe.test/X?Token=SECRET\u{7} tail";
        let redacted = redact_urls_in_text(input);

        assert_eq!(
            redacted,
            "prefixhttps://untouched.test/Case?Key=VALUE\u{1b} HTTPS://safe.test/X?Token=<redacted>\u{7} tail"
        );
        assert!(!redacted.contains("u:P"));
        assert!(!redacted.contains("SECRET"));
    }

    #[test]
    fn direct_output_boundary_redacts_urls_and_escapes_terminal_controls() {
        let rendered = terminal_safe_multiline(
            "name\u{1b}]52;c;owned\u{7}\r\nhttps://user:PASS@example.test/v1?token=SECRET#FRAG",
        );
        assert!(rendered.contains("name\\u{1b}]52;c;owned\\u{7}\\r\n"));
        assert!(rendered.contains("https://example.test/v1?token=<redacted>"));
        assert!(!rendered.contains("PASS"));
        assert!(!rendered.contains("SECRET"));
        assert!(!rendered.contains("FRAG"));
        assert!(!rendered.chars().any(|ch| ch.is_control() && ch != '\n'));
    }

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
