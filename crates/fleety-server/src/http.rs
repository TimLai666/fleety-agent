//! axum front for the server's single listen port.
//!
//! All client transports enter here. The WebSocket route (`GET /` with an
//! Upgrade) is the primary path; the SSE+POST fallback routes are added by the
//! sse-transport-fallback change. Every route ultimately drives the same
//! transport-agnostic [`run_connection`] core in [`crate::conn`] — only the
//! inbound ([`ClientInbound`]) and outbound ([`FrameWriter`]) adapters differ.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{CoreError, ModelProvider, Policy, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use fleety_protocol::ClientMsg;
use futures::stream;
use futures::StreamExt;
use tokio::sync::{mpsc, Mutex};

use crate::auth::AuthStore;
use crate::bridge::{DeviceTools, Handles, Hub, Pending};
use crate::conn::{run_connection, ClientInbound, FrameWriter};

/// How many opening frames are inspected for the `Hello` device id used in
/// connection logs. A sealed connection never exposes one, so this bound is what
/// keeps the peek from re-parsing the whole session.
const HELLO_PEEK_FRAMES: u32 = 3;
use crate::storage::Storage;

/// One live SSE+POST session: where to inject upstream messages, plus the
/// `Authorization` header presented when the stream was opened. `POST /send` must
/// present the same header, so only the caller that opened a session can inject
/// into it (the Hello frame still does the real identity auth in run_connection).
pub struct SseSession {
    in_tx: mpsc::UnboundedSender<ClientMsg>,
    auth_header: Option<String>,
}

/// Live SSE+POST sessions: session id → session. The `GET /sse` handler registers
/// one and spawns the connection; `POST /send` looks it up to inject upstream
/// messages. (WebSocket needs no such map — one socket carries both directions.)
pub type SseSessions = Arc<Mutex<HashMap<String, SseSession>>>;

/// A fresh, empty SSE session registry.
pub fn new_sse_sessions() -> SseSessions {
    Arc::new(Mutex::new(HashMap::new()))
}

/// WebSocket liveness timings (ws-liveness): how often the server pings each
/// connection, and how long a connection may stay silent before it is treated
/// as half-open and reclaimed. Read from env once at startup and carried in
/// [`AppState`], so tests can shorten the timings without racing on
/// process-global env.
#[derive(Clone, Copy)]
pub struct WsLiveness {
    pub ping_interval: std::time::Duration,
    pub deadline: std::time::Duration,
}

impl WsLiveness {
    /// `FLEETY_WS_PING_SECS` / `FLEETY_WS_TIMEOUT_SECS`; a non-numeric or
    /// non-positive value falls back to the default (20s / 60s). Keep the
    /// deadline at least twice the ping interval.
    pub fn from_env() -> Self {
        Self {
            ping_interval: std::time::Duration::from_secs(env_secs("FLEETY_WS_PING_SECS", 20)),
            deadline: std::time::Duration::from_secs(env_secs("FLEETY_WS_TIMEOUT_SECS", 60)),
        }
    }
}

/// Parse `var` as a positive number of seconds, falling back to `default`.
fn env_secs(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// Shared dependencies handed to every connection. Cheap to clone (Arcs + Copy).
#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<Storage>,
    pub provider: Arc<dyn ModelProvider>,
    pub workspace: Arc<PathBuf>,
    pub policy: Policy,
    pub hub: Hub,
    pub pending: Pending,
    pub handles: Handles,
    pub auth: Arc<AuthStore>,
    pub device_tools: DeviceTools,
    pub sse_sessions: SseSessions,
    pub ws_liveness: WsLiveness,
}

/// Build the router. WebSocket upgrades at `GET /` (clients connect to
/// `ws://host`, path `/`); the SSE+POST fallback is `GET /sse` (downstream
/// stream) + `POST /send` (upstream messages), correlated by `?session=<id>`.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(ws_handler))
        .route("/sse", get(sse_handler))
        .route("/send", post(send_handler))
        .with_state(state)
}

/// Upgrade a `GET /` request to WebSocket and drive the connection. The peer
/// socket address (from the accept layer, not a header) decides same-host
/// loopback trust.
async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    let loopback = crate::conn::peer_is_loopback(Some(peer));
    ws.on_upgrade(move |socket| serve_ws(socket, state, loopback))
}

async fn serve_ws(socket: WebSocket, state: AppState, peer_is_loopback: bool) {
    // The socket-owner task is the only holder of the WebSocket; the connection
    // logic talks to it through channels — the same shape as the SSE adapters.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
    let owner = tokio::spawn(own_socket(socket, out_rx, in_tx, state.ws_liveness));
    let inbound: Box<dyn ClientInbound> = Box::new(WsChannelInbound { rx: in_rx });
    let writer: Box<dyn FrameWriter> = Box::new(WsChannelWriter { tx: out_tx });
    if let Err(e) = run_connection(
        inbound,
        writer,
        state.storage,
        state.provider,
        state.workspace,
        state.policy,
        state.hub,
        state.pending,
        state.handles,
        state.auth,
        state.device_tools,
        peer_is_loopback,
    )
    .await
    {
        tracing::warn!(report = ?e.report(), "websocket connection error");
    }
    // run_connection has dropped its writer: the owner flushes whatever is still
    // queued (bounded by the write timeout) and exits, closing the socket.
    let _ = owner.await;
}

/// How long a single outbound write may stall before the peer is declared dead.
/// Keeps a vanished-but-unclosed client from parking the socket owner forever.
const WS_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Own the WebSocket: forward outbound frames from the connection logic,
/// forward inbound text frames to it, and keep the connection honest
/// (ws-liveness). A Ping goes out every `liveness.ping_interval`; any inbound
/// frame — text, pong, whatever — counts as proof of life; a connection silent
/// past `liveness.deadline` is closed, so `run_connection` unwinds through its
/// normal cleanup (hub and device_tools removal) and routing to the device
/// fails fast. Also exits when the socket ends (close/error), when a write
/// fails or stalls, or when the connection logic is done and its outbound
/// queue is drained.
///
/// A daemon blocked in a long-running tool does not read its socket, so it
/// stops answering pings and gets reclaimed here too. That is deliberate: the
/// call has already failed `device_exec`'s per-call timeout, the tool's side
/// effects still complete on the device, and the daemon reconnects as soon as
/// it returns to its loop.
async fn own_socket(
    mut socket: WebSocket,
    mut out_rx: mpsc::UnboundedReceiver<String>,
    in_tx: mpsc::UnboundedSender<String>,
    liveness: WsLiveness,
) {
    let mut ping = tokio::time::interval(liveness.ping_interval);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_seen = std::time::Instant::now();
    // The wire device id from the Hello frame, for the liveness log only (the
    // resolved identity lives in run_connection, above this task).
    let mut hello_device: Option<String> = None;
    let mut frames_seen: u32 = 0;
    loop {
        tokio::select! {
            out = out_rx.recv() => match out {
                Some(text) => {
                    let sent =
                        tokio::time::timeout(WS_WRITE_TIMEOUT, socket.send(Message::Text(text.into()))).await;
                    if !matches!(sent, Ok(Ok(()))) {
                        break; // write failed or stalled: the peer is gone
                    }
                }
                None => break, // connection logic finished and its queue is drained
            },
            frame = socket.next() => {
                last_seen = std::time::Instant::now();
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        frames_seen = frames_seen.saturating_add(1);
                        // Only the opening frames: on a sealed connection the
                        // Hello travels inside a `SecureFrame` and this can never
                        // match, so without a bound every frame would be parsed a
                        // second time just to fill a log field.
                        if hello_device.is_none() && frames_seen <= HELLO_PEEK_FRAMES {
                            if let Ok(ClientMsg::Hello { device_id, .. }) =
                                serde_json::from_str::<ClientMsg>(text.as_str())
                            {
                                hello_device = Some(device_id);
                            }
                        }
                        // A dropped receiver means run_connection ended; keep
                        // looping so the outbound queue still drains.
                        let _ = in_tx.send(text.to_string());
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    // Ping/pong/binary carry no message — they only feed liveness.
                    Some(Ok(_)) => {}
                }
            },
            _ = ping.tick() => {
                if last_seen.elapsed() >= liveness.deadline {
                    tracing::warn!(
                        device = hello_device.as_deref().unwrap_or("(pre-hello)"),
                        silent_secs = last_seen.elapsed().as_secs(),
                        "websocket liveness timeout; reclaiming half-open connection"
                    );
                    break;
                }
                let sent =
                    tokio::time::timeout(WS_WRITE_TIMEOUT, socket.send(Message::Ping(Vec::new().into()))).await;
                if !matches!(sent, Ok(Ok(()))) {
                    break; // cannot even ping: the connection is dead
                }
            },
        }
    }
}

/// Inbound adapter over the socket owner's channel: parses each text frame into
/// a `ClientMsg` (a malformed frame is a connection error, as before).
struct WsChannelInbound {
    rx: mpsc::UnboundedReceiver<String>,
}

#[async_trait::async_trait]
impl ClientInbound for WsChannelInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        match self.rx.recv().await {
            Some(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
                CoreError::Provider(format!(
                    "malformed client frame: {e}; expected a ClientMsg JSON object"
                ))
            }),
            None => Ok(None),
        }
    }
}

/// Outbound adapter: hands serialized frames to the socket-owner task.
struct WsChannelWriter {
    tx: mpsc::UnboundedSender<String>,
}

#[async_trait::async_trait]
impl FrameWriter for WsChannelWriter {
    async fn send_text(&mut self, text: String) -> bool {
        self.tx.send(text).is_ok()
    }
}

// ---- SSE (downstream) + POST (upstream) fallback transport ----

/// `GET /sse?session=<id>`: register the session, spawn its connection, and
/// stream `ServerMsg` frames as SSE events. A missing `session` is a 400.
async fn sse_handler(
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    let peer_is_loopback = crate::conn::peer_is_loopback(Some(peer));
    let Some(session) = params.get("session").filter(|s| !s.is_empty()).cloned() else {
        return (StatusCode::BAD_REQUEST, "missing ?session=<id>").into_response();
    };
    let auth_header = bearer_header(&headers);
    // When the server requires auth, a stream needs a valid token up front (an
    // already-paired device reconnecting). Initial pairing uses WebSocket.
    if state.auth.required() {
        let ok = auth_header
            .as_deref()
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .is_some_and(|t| state.auth.verify(t).is_some());
        if !ok {
            return (StatusCode::UNAUTHORIZED, "valid Bearer token required").into_response();
        }
    }
    // Inbound: POST /send pushes ClientMsg here. Outbound: run_connection pushes
    // serialized ServerMsg frames here, which this handler streams out as SSE.
    let (in_tx, in_rx) = mpsc::unbounded_channel::<ClientMsg>();
    let (out_tx, out_rx) = mpsc::unbounded_channel::<String>();
    state
        .sse_sessions
        .lock()
        .await
        .insert(session.clone(), SseSession { in_tx, auth_header });

    let conn_state = state.clone();
    let conn_session = session.clone();
    tokio::spawn(async move {
        let inbound: Box<dyn ClientInbound> = Box::new(SseInbound { rx: in_rx });
        let writer: Box<dyn FrameWriter> = Box::new(SseFrameWriter { tx: out_tx });
        if let Err(e) = run_connection(
            inbound,
            writer,
            conn_state.storage,
            conn_state.provider,
            conn_state.workspace,
            conn_state.policy,
            conn_state.hub,
            conn_state.pending,
            conn_state.handles,
            conn_state.auth,
            conn_state.device_tools,
            peer_is_loopback,
        )
        .await
        {
            tracing::warn!(report = ?e.report(), "sse connection error");
        }
        // The connection ended (client closed POST side, or error) — drop the
        // session so a stale id can't be reused.
        conn_state.sse_sessions.lock().await.remove(&conn_session);
    });

    // When this SSE stream ends or is dropped (client gone), reclaim the session
    // so a vanished client doesn't leak it. axum's keep-alive writes a periodic
    // comment, so a dead client surfaces (the write fails, the stream is dropped)
    // within the keep-alive interval even when the connection is otherwise idle.
    let guard = SessionGuard {
        sessions: state.sse_sessions.clone(),
        session: session.clone(),
    };
    // Stream outbound frames as SSE `data:` events. Tag each event that carries a
    // conversation `seq` with that seq as the SSE `id`, so a reconnecting client
    // can resume from `Last-Event-ID` (or send a `Resume` with the same value) and
    // the server replays only later events.
    let events = stream::unfold((out_rx, guard), |(mut rx, guard)| async move {
        rx.recv().await.map(|text| {
            let mut event = Event::default().data(&text);
            if let Some(seq) = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("seq").and_then(serde_json::Value::as_u64))
            {
                event = event.id(seq.to_string());
            }
            (Ok::<Event, std::convert::Infallible>(event), (rx, guard))
        })
    });
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// `POST /send?session=<id>` with a single `ClientMsg` JSON body: inject it into
/// the session's inbound channel. Unknown session → 404, bad JSON → 400.
async fn send_handler(
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    State(state): State<AppState>,
    body: String,
) -> StatusCode {
    let Some(session) = params.get("session") else {
        return StatusCode::BAD_REQUEST;
    };
    let msg: ClientMsg = match serde_json::from_str(&body) {
        Ok(m) => m,
        Err(_) => return StatusCode::BAD_REQUEST,
    };
    let auth_header = bearer_header(&headers);
    let sessions = state.sse_sessions.lock().await;
    match sessions.get(session) {
        // Only the caller that opened the stream (same Authorization) may inject.
        Some(s) if s.auth_header != auth_header => StatusCode::FORBIDDEN,
        Some(s) if s.in_tx.send(msg).is_ok() => StatusCode::ACCEPTED,
        // Session gone, or its connection task already dropped the receiver.
        _ => StatusCode::NOT_FOUND,
    }
}

/// The raw `Authorization` header value, if present and UTF-8.
fn bearer_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Drops a session from the registry when its SSE stream ends or is dropped
/// (client disconnected). Removing the entry drops the inbound sender, which ends
/// the connection's `serve` loop, so a vanished client leaks neither the session
/// nor a parked connection task.
struct SessionGuard {
    sessions: SseSessions,
    session: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let sessions = Arc::clone(&self.sessions);
        let session = std::mem::take(&mut self.session);
        // Removal needs the async lock; do it on the runtime.
        tokio::spawn(async move {
            sessions.lock().await.remove(&session);
        });
    }
}

/// Inbound adapter: yields `ClientMsg`s posted to `/send` for this session.
struct SseInbound {
    rx: mpsc::UnboundedReceiver<ClientMsg>,
}

#[async_trait::async_trait]
impl ClientInbound for SseInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        Ok(self.rx.recv().await)
    }
}

/// Outbound adapter: hands serialized frames to the SSE response stream.
struct SseFrameWriter {
    tx: mpsc::UnboundedSender<String>,
}

#[async_trait::async_trait]
impl FrameWriter for SseFrameWriter {
    async fn send_text(&mut self, text: String) -> bool {
        self.tx.send(text).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthStore;
    use crate::bridge;
    use crate::echo::EchoProvider;
    use crate::storage::Storage;
    use fleety_protocol::{ClientMsg, ServerMsg, PROTOCOL_VERSION};
    use futures::SinkExt;
    use std::time::Duration;
    use tokio::time::timeout;

    fn state_with(home: &std::path::Path, auth: Arc<AuthStore>) -> AppState {
        AppState {
            storage: Arc::new(Storage::new(home.to_path_buf())),
            provider: Arc::new(EchoProvider),
            workspace: Arc::new(home.to_path_buf()),
            policy: Policy::FullAccess,
            hub: bridge::new_hub(),
            pending: bridge::new_pending(),
            handles: bridge::new_handles(),
            auth,
            device_tools: bridge::new_device_tools(),
            sse_sessions: new_sse_sessions(),
            ws_liveness: WsLiveness::from_env(),
        }
    }

    fn test_state(home: &std::path::Path) -> AppState {
        let auth = Arc::new(AuthStore::load(home.join("auth.json"), None, false));
        state_with(home, auth)
    }

    /// Serve `state` on a fresh port; returns the `http://addr` base.
    async fn serve(state: AppState) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let app = router(state);
        tokio::spawn(async move {
            // ConnectInfo is required now that the handlers read the peer address
            // for loopback trust (the production serve uses the same).
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await;
        });
        format!("http://{addr}")
    }

    /// Read SSE `data:` events from the response until one parses as a `ServerMsg`
    /// for which `want` returns true; returns that message. Bounded by a timeout.
    async fn next_matching(
        resp: &mut reqwest::Response,
        buf: &mut String,
        want: impl Fn(&ServerMsg) -> bool,
    ) -> ServerMsg {
        loop {
            // Drain any complete events already buffered.
            while let Some(idx) = buf.find("\n\n") {
                let block: String = buf.drain(..idx + 2).collect();
                for line in block.lines() {
                    if let Some(data) = line.strip_prefix("data:") {
                        if let Ok(msg) = serde_json::from_str::<ServerMsg>(data.trim()) {
                            if want(&msg) {
                                return msg;
                            }
                        }
                    }
                }
            }
            let chunk = timeout(Duration::from_secs(5), resp.chunk())
                .await
                .expect("sse read timed out")
                .expect("sse stream error");
            match chunk {
                Some(bytes) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                None => panic!("sse stream ended before the expected frame"),
            }
        }
    }

    type WsClient = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;

    /// Connect a raw tokio-tungstenite client — the same WebSocket layer an
    /// old (pre-liveness) fleetyd uses — say Hello, and read until Welcome.
    /// Returns the socket and the Welcome's conversation id.
    async fn ws_hello(base: &str, device: &str) -> (WsClient, String) {
        let url = base.replace("http://", "ws://");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("ws connect");
        let hello = serde_json::to_string(&ClientMsg::Hello {
            device_id: device.into(),
            protocol: PROTOCOL_VERSION,
            token: None,
            pairing_code: None,
            local_tools_json: Some(
                serde_json::to_string(&vec![agent_core::ToolSpec {
                    name: "read_file".to_string(),
                    description: "test daemon tool".to_string(),
                    parameters: serde_json::json!({}),
                    risk: agent_core::RiskLevel::Read,
                }])
                .expect("tool specs"),
            ),
            hostname: None,
        })
        .expect("hello json");
        ws.send(tokio_tungstenite::tungstenite::Message::Text(hello))
            .await
            .expect("send hello");
        // Read until Welcome so the connection is fully registered in the hub.
        let conversation = timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                        if let Ok(ServerMsg::Welcome {
                            conversation_id, ..
                        }) = serde_json::from_str::<ServerMsg>(t.as_ref())
                        {
                            break conversation_id;
                        }
                    }
                    Some(Ok(_)) => {}
                    other => panic!("connection ended before Welcome: {other:?}"),
                }
            }
        })
        .await
        .expect("welcome");
        (ws, conversation)
    }

    /// ws-liveness (spec: server reclaims half-open connections): a WebSocket
    /// client that completes Hello and then stops reading and writing — the
    /// shape of a silent drop, or of a daemon blocked in a long tool — is
    /// removed from the hub within the liveness deadline plus a ping interval,
    /// and a subsequent device_exec fails fast with not-connected instead of
    /// burning the 30s per-call timeout.
    #[tokio::test]
    async fn ws_liveness_reclaims_half_open_connection() {
        let home = std::env::temp_dir().join(format!("fleety-wslive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let mut state = test_state(&home);
        state.ws_liveness = WsLiveness {
            ping_interval: Duration::from_millis(100),
            deadline: Duration::from_millis(300),
        };
        let hub = Arc::clone(&state.hub);
        let pending = Arc::clone(&state.pending);
        let handles = Arc::clone(&state.handles);
        let device_tools = Arc::clone(&state.device_tools);
        let base = serve(state).await;

        let (ws, _conversation) = ws_hello(&base, "half-open-dev").await;
        assert!(
            hub.lock().await.contains_key("half-open-dev"),
            "registered after Welcome"
        );

        // Go silent WITHOUT closing: hold the socket open but never poll it
        // again, so the client's WebSocket layer cannot flush pong replies.
        let reclaimed = async {
            while hub.lock().await.contains_key("half-open-dev") {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        };
        timeout(Duration::from_secs(3), reclaimed)
            .await
            .expect("half-open connection should be reclaimed within deadline + ping interval");
        drop(ws);

        // Routing to the reclaimed device fails fast, not via the call timeout.
        let mut registry = agent_core::ToolRegistry::new();
        crate::bridge::register(&mut registry, hub, pending, handles, device_tools);
        let err = timeout(
            Duration::from_secs(2),
            registry.call(
                "device_exec",
                serde_json::json!({ "device": "half-open-dev", "tool": "read_file", "args": {} }),
            ),
        )
        .await
        .expect("not-connected must fail fast")
        .expect_err("device is gone");
        assert!(
            err.report().message.contains("not connected"),
            "unexpected error: {}",
            err.report().message
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// ws-liveness (spec: idle healthy connection survives indefinitely; old
    /// daemon is protected without an upgrade): a bare tokio-tungstenite
    /// client — exactly the WebSocket layer a pre-liveness fleetyd uses —
    /// keeps polling its socket, so tungstenite answers the server's pings
    /// automatically. Across a full deadline window of message silence it
    /// stays registered and can still run a full message round-trip, with no
    /// client-side or protocol change.
    ///
    /// Timings are deliberately generous (1s deadline, 100ms pings) so the test
    /// is robust under the scheduler jitter of a loaded `cargo test --workspace`
    /// run: the connection is only reclaimed if a pong is delayed past the whole
    /// deadline, which a >1s task starvation would take — far beyond normal jitter.
    #[tokio::test]
    async fn ws_liveness_keeps_healthy_idle_connection() {
        let home = std::env::temp_dir().join(format!("fleety-wsidle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let mut state = test_state(&home);
        state.ws_liveness = WsLiveness {
            ping_interval: Duration::from_millis(100),
            deadline: Duration::from_millis(1000),
        };
        let hub = Arc::clone(&state.hub);
        let base = serve(state).await;

        let (mut ws, conversation) = ws_hello(&base, "idle-dev").await;

        // Idle (no messages) for longer than the deadline while polling the
        // socket — the frames read here are the server's pings, and reading them
        // is what lets the WebSocket layer flush its automatic pong replies. The
        // short poll timeout flushes pongs promptly even when the runtime is busy.
        let idle_until = tokio::time::Instant::now() + Duration::from_millis(1200);
        while tokio::time::Instant::now() < idle_until {
            match timeout(Duration::from_millis(25), ws.next()).await {
                Ok(Some(Ok(_))) => {}
                Ok(other) => panic!("connection ended while idle: {other:?}"),
                Err(_) => {} // no frame this tick
            }
        }
        assert!(
            hub.lock().await.contains_key("idle-dev"),
            "healthy idle client must stay registered"
        );

        // Still fully routable after idling: a message round-trip completes.
        let msg = serde_json::to_string(&ClientMsg::UserMessage {
            conversation_id: Some(conversation),
            text: "still there?".into(),
            origin: Default::default(),
            attachments: Vec::new(),
            voice: false,
            acting_user: None,
        })
        .expect("msg json");
        ws.send(tokio_tungstenite::tungstenite::Message::Text(msg))
            .await
            .expect("send after idle");
        let reply = timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                        if let Ok(ServerMsg::Assistant { text, .. }) =
                            serde_json::from_str::<ServerMsg>(t.as_ref())
                        {
                            break text;
                        }
                    }
                    Some(Ok(_)) => {}
                    other => panic!("connection ended before the assistant reply: {other:?}"),
                }
            }
        })
        .await
        .expect("assistant reply after idle");
        assert!(
            reply.contains("echo: still there?"),
            "unexpected reply: {reply}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Dropping a session's stream guard reclaims it from the registry (so a
    /// vanished SSE client doesn't leak the session).
    #[tokio::test]
    async fn session_guard_reclaims_on_drop() {
        let sessions = new_sse_sessions();
        let (in_tx, _in_rx) = mpsc::unbounded_channel::<ClientMsg>();
        sessions.lock().await.insert(
            "s".into(),
            SseSession {
                in_tx,
                auth_header: None,
            },
        );
        {
            let _guard = SessionGuard {
                sessions: Arc::clone(&sessions),
                session: "s".into(),
            };
            assert!(sessions.lock().await.contains_key("s"));
        } // guard dropped here → spawns removal
        for _ in 0..50 {
            tokio::task::yield_now().await;
            if !sessions.lock().await.contains_key("s") {
                break;
            }
        }
        assert!(
            !sessions.lock().await.contains_key("s"),
            "guard should reclaim the session on drop"
        );
    }

    /// Read the next complete SSE event block (terminated by a blank line).
    async fn next_block(resp: &mut reqwest::Response, buf: &mut String) -> String {
        loop {
            if let Some(idx) = buf.find("\n\n") {
                return buf.drain(..idx + 2).collect();
            }
            let chunk = timeout(Duration::from_secs(5), resp.chunk())
                .await
                .expect("sse read timed out")
                .expect("sse stream error")
                .expect("sse stream ended");
            buf.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    /// SSE events carry the conversation `seq` as their event `id`, and a `Resume`
    /// posted over SSE+POST replays the prior turn — the building blocks of
    /// gap-free reconnect.
    #[tokio::test]
    async fn sse_tags_seq_and_resumes() {
        let home = std::env::temp_dir().join(format!("fleety-sseseq-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let base = serve(test_state(&home)).await;
        let client = reqwest::Client::new();

        // Session A: run one turn.
        let mut a = client
            .get(format!("{base}/sse?session=A"))
            .send()
            .await
            .expect("open A");
        let mut buf = String::new();
        let post = |sess: &str, body: String| {
            client
                .post(format!("{base}/send?session={sess}"))
                .body(body)
        };
        post(
            "A",
            serde_json::to_string(&ClientMsg::Hello {
                device_id: "seq-dev".into(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: None,
                local_tools_json: None,
                hostname: None,
            })
            .unwrap(),
        )
        .send()
        .await
        .expect("hello A");
        let welcome =
            next_matching(&mut a, &mut buf, |m| matches!(m, ServerMsg::Welcome { .. })).await;
        let conv = match welcome {
            ServerMsg::Welcome {
                conversation_id, ..
            } => conversation_id,
            _ => unreachable!(),
        };
        post(
            "A",
            serde_json::to_string(&ClientMsg::UserMessage {
                conversation_id: Some(conv.clone()),
                text: "remember this".into(),
                origin: Default::default(),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            })
            .unwrap(),
        )
        .send()
        .await
        .expect("msg A");

        // Find the Assistant event block and confirm it carries `id: <seq>`.
        let mut assistant_seq = None;
        for _ in 0..200 {
            let block = next_block(&mut a, &mut buf).await;
            let data = block
                .lines()
                .find_map(|l| l.strip_prefix("data:"))
                .map(str::trim);
            if let Some(d) = data {
                if let Ok(ServerMsg::Assistant { seq, .. }) = serde_json::from_str::<ServerMsg>(d) {
                    assert!(
                        block.contains(&format!("id:{seq}"))
                            || block.contains(&format!("id: {seq}")),
                        "assistant event must carry its seq as the SSE id; block was: {block:?}"
                    );
                    assistant_seq = Some(seq);
                    break;
                }
            }
        }
        assert!(assistant_seq.is_some(), "never saw an Assistant event");

        // Session B: resume the conversation from the start; expect a Replay.
        let mut b = client
            .get(format!("{base}/sse?session=B"))
            .send()
            .await
            .expect("open B");
        let mut buf_b = String::new();
        post(
            "B",
            serde_json::to_string(&ClientMsg::Hello {
                device_id: "seq-dev".into(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: None,
                local_tools_json: None,
                hostname: None,
            })
            .unwrap(),
        )
        .send()
        .await
        .expect("hello B");
        next_matching(&mut b, &mut buf_b, |m| {
            matches!(m, ServerMsg::Welcome { .. })
        })
        .await;
        post(
            "B",
            serde_json::to_string(&ClientMsg::Resume {
                conversation_id: conv,
                after_seq: 0,
            })
            .unwrap(),
        )
        .send()
        .await
        .expect("resume B");
        let replay = next_matching(&mut b, &mut buf_b, |m| {
            matches!(m, ServerMsg::Replay { .. })
        })
        .await;
        if let ServerMsg::Replay { seq, .. } = replay {
            assert!(seq > 0, "replayed events come after after_seq=0");
        }

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A full turn over SSE+POST: open the stream, POST Hello, read Welcome, POST a
    /// user message, read the echoed assistant reply — proving the SSE+POST
    /// transport drives the same connection loop as WebSocket.
    #[tokio::test]
    async fn sse_post_full_turn_and_session_guard() {
        let home = std::env::temp_dir().join(format!("fleety-sse-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let base = serve(test_state(&home)).await;
        let client = reqwest::Client::new();
        let session = "sess-1";

        // POST to an unknown session is rejected (no stream registered yet).
        let early = client
            .post(format!("{base}/send?session=ghost"))
            .body(
                serde_json::to_string(&ClientMsg::Hello {
                    device_id: "d".into(),
                    protocol: PROTOCOL_VERSION,
                    token: None,
                    pairing_code: None,
                    local_tools_json: None,
                    hostname: None,
                })
                .unwrap(),
            )
            .send()
            .await
            .expect("post ghost");
        assert_eq!(early.status(), reqwest::StatusCode::NOT_FOUND);

        // Open the downstream SSE stream for our session.
        let mut resp = client
            .get(format!("{base}/sse?session={session}"))
            .send()
            .await
            .expect("open sse");
        assert!(resp.status().is_success());
        let mut buf = String::new();

        // POST Hello upstream, read Welcome downstream.
        let post = |body: String| {
            client
                .post(format!("{base}/send?session={session}"))
                .body(body)
        };
        post(
            serde_json::to_string(&ClientMsg::Hello {
                device_id: "sse-device".into(),
                protocol: PROTOCOL_VERSION,
                token: None,
                pairing_code: None,
                local_tools_json: None,
                hostname: None,
            })
            .unwrap(),
        )
        .send()
        .await
        .expect("post hello");
        let welcome = next_matching(&mut resp, &mut buf, |m| {
            matches!(m, ServerMsg::Welcome { .. })
        })
        .await;
        let conversation_id = match welcome {
            ServerMsg::Welcome {
                conversation_id, ..
            } => conversation_id,
            _ => unreachable!(),
        };

        // POST a user message, read the echoed assistant reply.
        post(
            serde_json::to_string(&ClientMsg::UserMessage {
                conversation_id: Some(conversation_id),
                text: "hi there".into(),
                origin: Default::default(),
                attachments: Vec::new(),
                voice: false,
                acting_user: None,
            })
            .unwrap(),
        )
        .send()
        .await
        .expect("post user msg");
        let reply = next_matching(
            &mut resp,
            &mut buf,
            |m| matches!(m, ServerMsg::Assistant { text, .. } if text.contains("echo: hi there")),
        )
        .await;
        assert!(matches!(reply, ServerMsg::Assistant { .. }));

        let _ = std::fs::remove_dir_all(&home);
    }

    /// With auth required: an unauthenticated SSE open is rejected; once opened
    /// with a valid token, a `POST /send` from a different (wrong) token is
    /// refused — only the opener can inject into the session.
    #[tokio::test]
    async fn sse_auth_required_and_session_binding() {
        let home = std::env::temp_dir().join(format!("fleety-sseauth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("mk home");
        let auth = Arc::new(AuthStore::load(
            home.join("auth.json"),
            Some("secret".to_string()),
            true,
        ));
        let base = serve(state_with(&home, auth)).await;
        let client = reqwest::Client::new();
        let session = "sess-auth";

        // Unauthenticated SSE open → 401.
        let no_auth = client
            .get(format!("{base}/sse?session={session}"))
            .send()
            .await
            .expect("open sse no auth");
        assert_eq!(no_auth.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Open with the valid token.
        let resp = client
            .get(format!("{base}/sse?session={session}"))
            .bearer_auth("secret")
            .send()
            .await
            .expect("open sse");
        assert!(resp.status().is_success());

        // A POST with a different Authorization is refused (foreign injection).
        let hello = serde_json::to_string(&ClientMsg::Hello {
            device_id: "d".into(),
            protocol: PROTOCOL_VERSION,
            token: Some("secret".into()),
            pairing_code: None,
            local_tools_json: None,
            hostname: None,
        })
        .unwrap();
        let foreign = client
            .post(format!("{base}/send?session={session}"))
            .bearer_auth("not-secret")
            .body(hello.clone())
            .send()
            .await
            .expect("post foreign");
        assert_eq!(foreign.status(), reqwest::StatusCode::FORBIDDEN);

        // The matching token is accepted.
        let ok = client
            .post(format!("{base}/send?session={session}"))
            .bearer_auth("secret")
            .body(hello)
            .send()
            .await
            .expect("post ok");
        assert_eq!(ok.status(), reqwest::StatusCode::ACCEPTED);

        let _ = std::fs::remove_dir_all(&home);
    }
}
