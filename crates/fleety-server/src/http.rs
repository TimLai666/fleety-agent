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
use axum::extract::{Query, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::stream::{self, SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use fleety_protocol::ClientMsg;
use tokio::sync::{mpsc, Mutex};

use crate::auth::AuthStore;
use crate::bridge::{DeviceTools, Handles, Hub, Pending};
use crate::conn::{run_connection, ClientInbound, FrameWriter};
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

/// Upgrade a `GET /` request to WebSocket and drive the connection.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| serve_ws(socket, state))
}

async fn serve_ws(socket: WebSocket, state: AppState) {
    let (sink, stream) = socket.split();
    let inbound: Box<dyn ClientInbound> = Box::new(AxumWsInbound { stream });
    let writer: Box<dyn FrameWriter> = Box::new(AxumWsFrameWriter { sink });
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
    )
    .await
    {
        tracing::warn!(report = ?e.report(), "websocket connection error");
    }
}

/// Inbound adapter over an axum WebSocket stream.
struct AxumWsInbound {
    stream: SplitStream<WebSocket>,
}

#[async_trait::async_trait]
impl ClientInbound for AxumWsInbound {
    async fn next_client(&mut self) -> Result<Option<ClientMsg>> {
        while let Some(frame) = self.stream.next().await {
            match frame {
                Ok(Message::Text(text)) => {
                    let msg = serde_json::from_str(text.as_str()).map_err(|e| {
                        CoreError::Provider(format!(
                            "malformed client frame: {e}; expected a ClientMsg JSON object"
                        ))
                    })?;
                    return Ok(Some(msg));
                }
                Ok(Message::Close(_)) => return Ok(None),
                // A read error (reset/abort/EOF) is a normal end of connection.
                Err(_) => return Ok(None),
                // Ignore ping/pong/binary frames.
                Ok(_) => continue,
            }
        }
        Ok(None)
    }
}

/// Outbound adapter over an axum WebSocket sink.
struct AxumWsFrameWriter {
    sink: SplitSink<WebSocket, Message>,
}

#[async_trait::async_trait]
impl FrameWriter for AxumWsFrameWriter {
    async fn send_text(&mut self, text: String) -> bool {
        self.sink.send(Message::Text(text.into())).await.is_ok()
    }
}

// ---- SSE (downstream) + POST (upstream) fallback transport ----

/// `GET /sse?session=<id>`: register the session, spawn its connection, and
/// stream `ServerMsg` frames as SSE events. A missing `session` is a 400.
async fn sse_handler(
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
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
    state.sse_sessions.lock().await.insert(
        session.clone(),
        SseSession {
            in_tx,
            auth_header,
        },
    );

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
        )
        .await
        {
            tracing::warn!(report = ?e.report(), "sse connection error");
        }
        // The connection ended (client closed POST side, or error) — drop the
        // session so a stale id can't be reused.
        conn_state.sse_sessions.lock().await.remove(&conn_session);
    });

    // Stream outbound frames as SSE `data:` events; axum's keep-alive sends a
    // periodic comment so proxies don't drop an idle stream (half-open guard).
    let events = stream::unfold(out_rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|text| (Ok::<Event, std::convert::Infallible>(Event::default().data(text)), rx))
    });
    Sse::new(events).keep_alive(KeepAlive::default()).into_response()
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
            let _ = axum::serve(listener, app.into_make_service()).await;
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
        let reply = next_matching(&mut resp, &mut buf, |m| {
            matches!(m, ServerMsg::Assistant { text, .. } if text.contains("echo: hi there"))
        })
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
