//! fleety — the Fleety CLI.
//!
//! M2: `fleety ask "<message>"` connects to the Agent over WebSocket, does one
//! conversation round-trip, and prints the reply. Interactive TUI comes later.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

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
        Some("init") => {
            let url = args.get(2).cloned().unwrap_or_default();
            if url.is_empty() {
                eprintln!("usage: fleety init <agent-url>   (e.g. ws://host:8787)");
                return;
            }
            if let Err(e) = init(url).await {
                let report = e.report();
                eprintln!("error: {}", report.message);
                if let Some(hint) = report.remediation {
                    eprintln!("hint: {hint}");
                }
            }
        }
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
        Some("resume") => {
            let conversation_id = args.get(2).cloned().unwrap_or_default();
            let after_seq = args.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            if conversation_id.is_empty() {
                eprintln!("usage: fleety resume <conversation_id> [after_seq]");
                return;
            }
            if let Err(e) = resume(conversation_id, after_seq).await {
                eprintln!("error: {}", e.report().message);
            }
        }
        _ => {
            println!("fleety {} — try: fleety ask \"hello\"", agent_core::VERSION);
        }
    }
}

fn fleety_dir() -> Option<PathBuf> {
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(base).join(".fleety"))
}

/// Resolve the agent URL: `FLEETY_AGENT_URL`, else saved config, else default.
fn agent_url() -> String {
    if let Ok(url) = std::env::var("FLEETY_AGENT_URL") {
        return url;
    }
    if let Some(dir) = fleety_dir() {
        if let Ok(text) = std::fs::read_to_string(dir.join("config.json")) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(url) = value.get("agent_url").and_then(|v| v.as_str()) {
                    return url.to_string();
                }
            }
        }
    }
    "ws://127.0.0.1:8787".to_string()
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

/// `fleety init <agent-url>`: connect, register this device, and save config.
async fn init(url: String) -> Result<()> {
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
        Some(ServerMsg::Welcome { session_id, .. }) => {
            if let Some(dir) = fleety_dir() {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| CoreError::Message(format!("cannot create ~/.fleety: {e}")))?;
                let config = serde_json::json!({ "agent_url": url, "device_id": device_id() });
                std::fs::write(
                    dir.join("config.json"),
                    serde_json::to_string_pretty(&config).unwrap_or_default(),
                )
                .map_err(|e| CoreError::Message(format!("cannot write config: {e}")))?;
            }
            println!("✓ connected to {url}");
            println!(
                "✓ registered device '{}' (session {session_id})",
                device_id()
            );
        }
        other => {
            return Err(CoreError::Provider(format!(
                "unexpected reply during init: {other:?}"
            )))
        }
    }
    let _ = tx.close().await;
    Ok(())
}

async fn ask(text: String) -> Result<()> {
    let url = agent_url();
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
            Some(ServerMsg::ApprovalRequested {
                approval_id,
                tool,
                risk,
                summary,
            }) => {
                eprintln!("Approve tool '{tool}' (risk: {risk})? {summary}");
                eprint!("[y/N] ");
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                let decision = if line.trim().eq_ignore_ascii_case("y") {
                    ClientMsg::Approve { approval_id }
                } else {
                    ClientMsg::Deny { approval_id }
                };
                send(&mut tx, &decision).await?;
            }
            Some(ServerMsg::Welcome { .. }) | Some(ServerMsg::Replay { .. }) => {}
        }
    }
    // Close the connection gracefully so the server sees a clean disconnect.
    let _ = tx.close().await;
    Ok(())
}

/// Reconnect to a conversation and print events replayed after `after_seq`.
async fn resume(conversation_id: String, after_seq: u64) -> Result<()> {
    let url = agent_url();
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
        &ClientMsg::Resume {
            conversation_id,
            after_seq,
        },
    )
    .await?;
    loop {
        match recv(&mut rx).await? {
            Some(ServerMsg::Replay {
                seq, role, content, ..
            }) => println!("[{seq}] {role}: {content}"),
            Some(ServerMsg::Done { .. }) | None => break,
            Some(ServerMsg::Error { error }) => {
                eprintln!("agent error: {}", error.message);
                break;
            }
            _ => {}
        }
    }
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
