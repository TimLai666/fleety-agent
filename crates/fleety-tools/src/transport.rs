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

/// The established secure channel, shared by the two halves.
///
/// Noise keeps one state for both directions, so the halves cannot own separate
/// copies. The lock is only ever held across the sealing itself — never across
/// an await — so the two directions do not block each other in practice.
type SharedSecure = std::sync::Arc<std::sync::Mutex<crate::secure::SecureSession>>;

fn with_secure<T>(
    secure: &SharedSecure,
    op: impl FnOnce(&mut crate::secure::SecureSession) -> Result<T>,
) -> Result<T> {
    let mut session = secure
        .lock()
        .map_err(|_| CoreError::Message("the secure channel state was lost".to_string()))?;
    op(&mut session)
}

/// Outbound half. Once a secure channel is established every frame leaves
/// sealed; there is no path that puts a control frame back in the clear.
pub struct Sender {
    inner: SenderKind,
    secure: Option<SharedSecure>,
}

enum SenderKind {
    Ws(futures::stream::SplitSink<Ws, WsMessage>),
    Sse {
        client: reqwest::Client,
        send_url: String,
        token: Option<String>,
    },
}

/// Inbound half.
pub struct Receiver {
    inner: ReceiverKind,
    secure: Option<SharedSecure>,
}

enum ReceiverKind {
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
    /// Send one outbound frame. On a secure channel the caller's frame is
    /// sealed first, so what reaches the wire carries no readable content and
    /// cannot be altered in flight.
    pub async fn send_text(&mut self, text: String) -> Result<()> {
        let text = match &self.secure {
            Some(secure) => {
                let payload = with_secure(secure, |session| session.seal(&text))?;
                serde_json::to_string(&fleety_protocol::ClientMsg::SecureFrame { payload })
                    .map_err(|e| {
                        CoreError::Message(format!("could not frame a sealed send: {e}"))
                    })?
            }
            None => text,
        };
        self.send_raw(text).await
    }

    /// Send one frame exactly as given, bypassing the secure channel. Only the
    /// handshake itself may use this: it runs before a channel exists.
    async fn send_raw(&mut self, text: String) -> Result<()> {
        match &mut self.inner {
            SenderKind::Ws(tx) => tx
                .send(WsMessage::Text(text))
                .await
                .map_err(|e| CoreError::Provider(format!("websocket send failed: {e}"))),
            SenderKind::Sse {
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
        if let SenderKind::Ws(tx) = &mut self.inner {
            let _ = tx.close().await;
        }
    }
}

impl Receiver {
    /// The next inbound frame text, or `None` when the link is closed/dead
    /// (including a WebSocket whose armed read deadline elapsed with no frames).
    ///
    /// On a secure channel the frame is opened first. A peer that sends a
    /// cleartext control frame after the channel is up is treated as a dead
    /// link, not as a peer to keep talking to — that is what stops an active
    /// relay from stripping the encryption back off.
    pub async fn recv_text(&mut self) -> Option<String> {
        let raw = self.recv_raw().await?;
        let Some(secure) = &self.secure else {
            return Some(raw);
        };
        let payload = match serde_json::from_str::<fleety_protocol::ServerMsg>(&raw) {
            Ok(fleety_protocol::ServerMsg::SecureFrame { payload }) => payload,
            _ => {
                tracing::warn!(
                    "the Server sent an unsealed control frame on a secure channel; dropping the link"
                );
                return None;
            }
        };
        match with_secure(secure, |session| session.open(&payload)) {
            Ok(frame) => Some(frame),
            Err(error) => {
                tracing::warn!(
                    report = ?error.report(),
                    "a sealed control frame failed verification; dropping the link"
                );
                None
            }
        }
    }

    /// The next frame exactly as it arrived, without opening the secure
    /// channel. Only the handshake may use this.
    async fn recv_raw(&mut self) -> Option<String> {
        match &mut self.inner {
            ReceiverKind::Ws(ws) => loop {
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
            ReceiverKind::Sse(rx) => rx.recv().await,
        }
    }
}

/// What a secure connect attempt found at the far end.
pub enum SecureChannel {
    /// The peer proved it holds this profile's device token. Everything from
    /// here on is sealed.
    Established(Connection),
    /// The peer never engaged with the handshake — an older Server, something
    /// that is not a Fleety Server at all, or a peer staying deliberately
    /// silent. No credential was revealed either way, so the caller is free to
    /// apply its own policy: a saved alternative endpoint must give up, while
    /// the endpoint the user themselves configured may still connect the old
    /// way if this profile has never seen a secure Server.
    Unsupported { detail: String },
}

/// Connect and establish the authenticated, encrypted channel a paired profile
/// uses, keyed by `token` and the `server_fingerprint` this profile is pinned to.
///
/// The token is deliberately *not* handed to [`connect`]: nothing authenticating
/// reaches the wire until the peer has proven it already holds the same token.
/// An `Err` means the peer engaged and failed to prove itself — never retry that
/// endpoint in the clear.
pub async fn connect_secure(
    agent_url: &str,
    token: &str,
    server_fingerprint: &str,
    wait: Duration,
) -> Result<SecureChannel> {
    let (initiator, opening) = crate::secure::Initiator::start(token, server_fingerprint)?;
    let key_id = crate::secure::key_id(token, server_fingerprint)?;
    // WebSocket only. The SSE+POST fallback authenticates its downstream with
    // the very bearer this handshake exists to keep off the wire, so a sealed
    // session cannot be opened over it.
    if mode_from_env() == Mode::ForceSse {
        return Ok(SecureChannel::Unsupported {
            detail: "FLEETY_FORCE_SSE is set, and the SSE fallback cannot carry an encrypted \
                     control channel"
                .to_string(),
        });
    }
    let mut conn = match connect_ws(agent_url).await {
        Ok(conn) => conn,
        Err(error) => {
            if mode_from_env() == Mode::WsOnly {
                return Err(error);
            }
            return Ok(SecureChannel::Unsupported {
                detail: format!(
                    "{} — the SSE fallback cannot carry an encrypted control channel",
                    error.report().message
                ),
            });
        }
    };

    let offer = serde_json::to_string(&fleety_protocol::ClientMsg::SecureHandshake {
        version: crate::secure::SECURE_CHANNEL_VERSION,
        key_id,
        msg: opening,
    })
    .map_err(|e| CoreError::Message(format!("could not frame the secure handshake: {e}")))?;
    if let Err(error) = conn.sender.send_raw(offer).await {
        return Ok(SecureChannel::Unsupported {
            detail: error.report().message,
        });
    }

    let reply = match tokio::time::timeout(wait, conn.receiver.recv_raw()).await {
        Ok(Some(reply)) => reply,
        // A closed link is exactly what an older Server does with a frame it
        // cannot parse, so this is the ordinary "too old" path, not a failure.
        Ok(None) => {
            conn.close().await;
            return Ok(SecureChannel::Unsupported {
                detail: "the Server closed the connection without answering".to_string(),
            });
        }
        Err(_) => {
            conn.close().await;
            return Ok(SecureChannel::Unsupported {
                detail: format!("the Server did not answer within {} ms", wait.as_millis()),
            });
        }
    };

    let (version, msg) = match serde_json::from_str::<fleety_protocol::ServerMsg>(&reply) {
        Ok(fleety_protocol::ServerMsg::SecureAccept { version, msg }) => (version, msg),
        // A Server new enough to understand the frame but unable to answer it
        // has no credential for this device — which is a statement that it is
        // not the Server this profile is paired with. That is a refusal, not an
        // old Server, so it must not buy a cleartext fallback.
        Ok(fleety_protocol::ServerMsg::Error { error }) => {
            conn.close().await;
            return Err(CoreError::Message(format!(
                "this endpoint has no credential for this device ({}) — re-pair with \
                 `fleety init <ws-url> --name <profile> --pairing-code <code>`",
                error.message
            )));
        }
        _ => {
            conn.close().await;
            return Ok(SecureChannel::Unsupported {
                detail: "the Server did not answer the secure handshake".to_string(),
            });
        }
    };

    // From here the peer has committed to the handshake, so a failure is an
    // impostor rather than an old Server: fail closed and never downgrade.
    let session = initiator.finish(version, &msg).inspect_err(|_| {
        tracing::warn!(
            "a saved endpoint answered the secure handshake but could not prove this profile's credential"
        );
    })?;
    let shared: SharedSecure = std::sync::Arc::new(std::sync::Mutex::new(session));
    conn.sender.secure = Some(shared.clone());
    conn.receiver.secure = Some(shared);
    Ok(SecureChannel::Established(conn))
}

/// Like [`connect`], but for a speculative probe: nobody asked for this
/// connection, so a failure is not news. Keeps the fallback, drops the warning.
pub async fn connect_quietly(agent_url: &str, token: Option<&str>) -> Result<Connection> {
    match mode_from_env() {
        Mode::WsOnly => connect_ws(agent_url).await,
        Mode::ForceSse => connect_sse(agent_url, token).await,
        Mode::Auto => match connect_ws(agent_url).await {
            Ok(connection) => Ok(connection),
            Err(_) => connect_sse(agent_url, token).await,
        },
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
            Err(ws_error) => {
                let endpoint = redact_endpoint(agent_url);
                tracing::debug!(%endpoint, "websocket connect failed; trying SSE+POST fallback");
                // Report the WebSocket failure, which is the transport the user
                // configured. The fallback is an implementation detail, and
                // naming only its error sends people to look at the wrong thing.
                connect_sse(agent_url, token).await.map_err(|_| ws_error)
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
    // Idempotent: these messages are wrapped and re-wrapped as they travel, and
    // running the pass twice produced `<redacted><redacted>`.
    if value.contains("<redacted>") {
        return value.to_string();
    }
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
        sender: Sender {
            inner: SenderKind::Ws(tx),
            secure: None,
        },
        receiver: Receiver {
            inner: ReceiverKind::Ws(WsReceiver {
                rx,
                deadline: ws_timeout(),
                armed: false,
            }),
            secure: None,
        },
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
        sender: Sender {
            inner: SenderKind::Sse {
                client,
                send_url,
                token: token.map(str::to_string),
            },
            secure: None,
        },
        receiver: Receiver {
            inner: ReceiverKind::Sse(rx),
            secure: None,
        },
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

    const TEST_TOKEN: &str = "device-token-must-never-appear";
    const TEST_FINGERPRINT: &str = "3f1c9a52-0d44-4a51-9b6e-1f2d3c4b5a60";

    /// How a fake Server should behave when the client offers a handshake.
    #[derive(Clone, Copy)]
    enum HandshakePeer {
        /// Holds the same device token and answers honestly.
        Honest,
        /// Understands nothing and drops the link, like an older Server.
        CloseImmediately,
        /// Understands the frame and says it holds no credential for us.
        RefuseExplicitly,
        /// Completes the handshake and then sends a control frame in the clear,
        /// which is what a relay stripping the encryption would produce.
        SealsThenSpeaksPlainly,
        /// Accepts the socket and then says nothing at all.
        Silent,
        /// Answers with a well-formed frame it cannot actually back up.
        ForgedAccept,
    }

    /// A fake Server for the handshake tests. Returns its address plus a handle
    /// to everything the client put on the wire, so a test can assert on what
    /// was actually transmitted rather than on what we believe was transmitted.
    async fn handshake_ws_server(
        peer: HandshakePeer,
    ) -> (
        std::net::SocketAddr,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let recorded = seen.clone();
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                return;
            };
            let Some(Ok(WsMessage::Text(opening))) = ws.next().await else {
                return;
            };
            if let Ok(mut log) = recorded.lock() {
                log.push(opening.to_string());
            }
            let Ok(fleety_protocol::ClientMsg::SecureHandshake { version, msg, .. }) =
                serde_json::from_str(&opening)
            else {
                return;
            };
            match peer {
                HandshakePeer::CloseImmediately => {
                    let _ = ws.close(None).await;
                }
                HandshakePeer::RefuseExplicitly => {
                    let refusal = serde_json::to_string(&fleety_protocol::ServerMsg::Error {
                        error: fleety_protocol::WireError {
                            kind: "secure_unavailable".into(),
                            message: "no credential for that device".into(),
                            remediation: None,
                        },
                    })
                    .expect("frame");
                    let _ = ws.send(WsMessage::Text(refusal)).await;
                    let _ = ws.close(None).await;
                }
                HandshakePeer::Silent => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                HandshakePeer::ForgedAccept => {
                    let forged = serde_json::to_string(&fleety_protocol::ServerMsg::SecureAccept {
                        version,
                        msg: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                            .to_string(),
                    })
                    .expect("frame");
                    let _ = ws.send(WsMessage::Text(forged)).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                HandshakePeer::SealsThenSpeaksPlainly => {
                    let Ok((responder, reply)) = crate::secure::Responder::accept(
                        version,
                        TEST_TOKEN,
                        TEST_FINGERPRINT,
                        &msg,
                    ) else {
                        return;
                    };
                    let accept = serde_json::to_string(&fleety_protocol::ServerMsg::SecureAccept {
                        version,
                        msg: reply,
                    })
                    .expect("frame");
                    let _ = ws.send(WsMessage::Text(accept)).await;
                    let _ = responder.finish();
                    let plain = serde_json::to_string(&fleety_protocol::ServerMsg::Error {
                        error: fleety_protocol::WireError {
                            kind: "stripped".into(),
                            message: "this frame was never sealed".into(),
                            remediation: None,
                        },
                    })
                    .expect("frame");
                    let _ = ws.send(WsMessage::Text(plain)).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                HandshakePeer::Honest => {
                    let Ok((responder, reply)) = crate::secure::Responder::accept(
                        version,
                        TEST_TOKEN,
                        TEST_FINGERPRINT,
                        &msg,
                    ) else {
                        return;
                    };
                    let accept = serde_json::to_string(&fleety_protocol::ServerMsg::SecureAccept {
                        version,
                        msg: reply,
                    })
                    .expect("frame");
                    let _ = ws.send(WsMessage::Text(accept)).await;
                    let Ok(mut session) = responder.finish() else {
                        return;
                    };
                    // Echo each sealed client frame back, sealed.
                    while let Some(Ok(WsMessage::Text(text))) = ws.next().await {
                        if let Ok(mut log) = recorded.lock() {
                            log.push(text.to_string());
                        }
                        let Ok(fleety_protocol::ClientMsg::SecureFrame { payload }) =
                            serde_json::from_str(&text)
                        else {
                            return;
                        };
                        let Ok(opened) = session.open(&payload) else {
                            return;
                        };
                        let Ok(payload) = session.seal(&opened) else {
                            return;
                        };
                        let sealed =
                            serde_json::to_string(&fleety_protocol::ServerMsg::SecureFrame {
                                payload,
                            })
                            .expect("frame");
                        if ws.send(WsMessage::Text(sealed)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        (addr, seen)
    }

    /// The whole point of the handshake: the credential stays off the wire, and
    /// control frames are unreadable to anything relaying them.
    #[tokio::test]
    async fn the_device_token_and_control_frames_never_appear_on_the_wire() {
        let (addr, seen) = handshake_ws_server(HandshakePeer::Honest).await;

        let SecureChannel::Established(mut conn) = connect_secure(
            &format!("ws://{addr}"),
            TEST_TOKEN,
            TEST_FINGERPRINT,
            Duration::from_secs(5),
        )
        .await
        .expect("the handshake completes against a Server holding this token") else {
            panic!("an honest Server must establish the channel");
        };

        let frame = format!(r#"{{"type":"hello","token":"{TEST_TOKEN}"}}"#);
        conn.send_text(frame.clone()).await.expect("send");
        assert_eq!(
            conn.recv_text().await.expect("the echoed frame comes back"),
            frame,
            "the caller still sees its own plaintext frames"
        );

        let wire = seen.lock().expect("recorded wire traffic").join("\n");
        assert!(!wire.is_empty(), "the fake Server recorded nothing");
        assert!(
            !wire.contains(TEST_TOKEN),
            "the device token must never be transmitted; wire was: {wire}"
        );
        assert!(
            !wire.contains("\"type\":\"hello\""),
            "control frames must not be readable on the wire; wire was: {wire}"
        );
    }

    /// A peer that understands the frame and refuses it has stated it holds no
    /// credential for this device — which is a statement that it is not the
    /// Server this profile is paired with. That must not buy a cleartext
    /// fallback the way an unparseable frame does.
    #[tokio::test]
    async fn an_explicit_refusal_is_fatal_rather_than_a_reason_to_downgrade() {
        let (addr, _) = handshake_ws_server(HandshakePeer::RefuseExplicitly).await;

        assert!(
            connect_secure(
                &format!("ws://{addr}"),
                TEST_TOKEN,
                TEST_FINGERPRINT,
                Duration::from_secs(5),
            )
            .await
            .is_err(),
            "a peer that says it has no credential must not be handed one"
        );
    }

    /// Once the channel is up, a cleartext control frame is what an active relay
    /// sends after stripping the encryption. Accepting one would undo the whole
    /// handshake, so the link is treated as dead instead.
    #[tokio::test]
    async fn a_cleartext_frame_on_a_sealed_channel_is_treated_as_a_dead_link() {
        let (addr, _) = handshake_ws_server(HandshakePeer::SealsThenSpeaksPlainly).await;

        let SecureChannel::Established(mut conn) = connect_secure(
            &format!("ws://{addr}"),
            TEST_TOKEN,
            TEST_FINGERPRINT,
            Duration::from_secs(5),
        )
        .await
        .expect("the handshake completes") else {
            panic!("the peer holds this token, so the channel must establish");
        };

        assert!(
            conn.recv_text().await.is_none(),
            "an unsealed frame must end the link, not be handed to the caller"
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_speak_the_handshake_is_reported_unsupported() {
        let (addr, _) = handshake_ws_server(HandshakePeer::CloseImmediately).await;

        let outcome = connect_secure(
            &format!("ws://{addr}"),
            TEST_TOKEN,
            TEST_FINGERPRINT,
            Duration::from_secs(5),
        )
        .await
        .expect("an old Server is not an error, it is a policy decision");

        assert!(
            matches!(outcome, SecureChannel::Unsupported { .. }),
            "closing the link is what an older Server does; the caller decides what that means"
        );
    }

    #[tokio::test]
    async fn a_silent_server_is_reported_unsupported_once_the_deadline_passes() {
        let (addr, _) = handshake_ws_server(HandshakePeer::Silent).await;

        let outcome = connect_secure(
            &format!("ws://{addr}"),
            TEST_TOKEN,
            TEST_FINGERPRINT,
            Duration::from_millis(300),
        )
        .await
        .expect("a silent peer resolves within the deadline");

        assert!(
            matches!(outcome, SecureChannel::Unsupported { .. }),
            "a peer that never answers must not hold the connect open"
        );
    }

    /// A peer that engages with the handshake and fails is an impostor, not an
    /// old Server: this must be a hard error so no caller can read it as a
    /// reason to try the endpoint in the clear.
    #[tokio::test]
    async fn a_forged_acceptance_is_a_hard_failure_not_a_downgrade() {
        let (addr, _) = handshake_ws_server(HandshakePeer::ForgedAccept).await;

        assert!(
            connect_secure(
                &format!("ws://{addr}"),
                TEST_TOKEN,
                TEST_FINGERPRINT,
                Duration::from_secs(5),
            )
            .await
            .is_err(),
            "a forged acceptance must fail closed"
        );
    }

    /// Wrap a connected client socket in a `Receiver` with a short deadline
    /// (bypassing env so parallel tests don't race on process globals).
    async fn ws_receiver(addr: std::net::SocketAddr, deadline: Duration) -> Receiver {
        let (ws, _) = connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let (_tx, rx) = ws.split();
        Receiver {
            inner: ReceiverKind::Ws(WsReceiver {
                rx,
                deadline,
                armed: false,
            }),
            secure: None,
        }
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
