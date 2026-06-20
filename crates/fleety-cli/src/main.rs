//! fleety — the Fleety CLI.
//!
//! M2: `fleety ask "<message>"` connects to the Agent over WebSocket, does one
//! conversation round-trip, and prints the reply. Interactive TUI comes later.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use agent_core::{obs, CoreError, Result};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use fleety_protocol::{ClientMsg, OriginContext, ServerMsg, PROTOCOL_VERSION};

type Tx = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>;
type Rx = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

#[tokio::main]
async fn main() {
    obs::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("ask") => {
            let text = args.get(2).cloned().unwrap_or_default();
            if text.is_empty() {
                eprintln!("usage: fleety ask \"<message>\"");
                return;
            }
            if let Err(e) = ask(text).await {
                let report = e.report();
                eprintln!("error: {}", report.message);
                if let Some(hint) = report.remediation {
                    eprintln!("hint: {hint}");
                }
            }
        }
        _ => {
            println!("fleety {} — try: fleety ask \"hello\"", agent_core::VERSION);
        }
    }
}

fn device_id() -> String {
    std::env::var("FLEETY_DEVICE_ID")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "cli-device".to_string())
}

fn origin() -> OriginContext {
    OriginContext {
        hostname: std::env::var("COMPUTERNAME")
            .ok()
            .or_else(|| std::env::var("HOSTNAME").ok()),
        os: Some(std::env::consts::OS.to_string()),
        cwd: std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string()),
    }
}

async fn ask(text: String) -> Result<()> {
    let url =
        std::env::var("FLEETY_AGENT_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787".to_string());
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    send(
        &mut tx,
        &ClientMsg::Hello {
            device_id: device_id(),
            protocol: PROTOCOL_VERSION,
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome { .. }) => {}
        other => {
            return Err(CoreError::Provider(format!(
                "expected welcome, got {other:?}"
            )))
        }
    }

    send(
        &mut tx,
        &ClientMsg::UserMessage {
            conversation_id: None,
            text,
            origin: origin(),
        },
    )
    .await?;
    loop {
        match recv(&mut rx).await? {
            Some(ServerMsg::Assistant { text, .. }) => println!("{text}"),
            Some(ServerMsg::Done { .. }) | None => break,
            Some(ServerMsg::Error { error }) => {
                eprintln!("agent error: {}", error.message);
                break;
            }
            Some(ServerMsg::Welcome { .. }) => {}
        }
    }
    // Close the connection gracefully so the server sees a clean disconnect.
    let _ = tx.close().await;
    Ok(())
}

async fn send(tx: &mut Tx, msg: &ClientMsg) -> Result<()> {
    let json = serde_json::to_string(msg)
        .map_err(|e| CoreError::Message(format!("serialize client frame: {e}")))?;
    tx.send(WsMessage::Text(json))
        .await
        .map_err(|e| CoreError::Provider(format!("websocket send failed: {e}")))?;
    Ok(())
}

async fn recv(rx: &mut Rx) -> Result<Option<ServerMsg>> {
    while let Some(frame) = rx.next().await {
        let frame =
            frame.map_err(|e| CoreError::Provider(format!("websocket read failed: {e}")))?;
        if frame.is_text() {
            let text = frame
                .to_text()
                .map_err(|e| CoreError::Provider(format!("non-utf8 text frame: {e}")))?;
            let msg = serde_json::from_str(text)
                .map_err(|e| CoreError::Provider(format!("malformed server frame: {e}")))?;
            return Ok(Some(msg));
        } else if frame.is_close() {
            return Ok(None);
        }
    }
    Ok(None)
}
