//! fleety — the Fleety CLI.
//!
//! M2: `fleety ask "<message>"` connects to the Agent over WebSocket, does one
//! conversation round-trip, and prints the reply. Interactive TUI comes later.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod clipboard;
mod tui;
mod voice;

use std::path::{Path, PathBuf};

use agent_core::{obs, CoreError, Result};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use fleety_protocol::{ClientMsg, OriginContext, ServerMsg, WireAttachment, PROTOCOL_VERSION};

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
            // Parse: fleety ask [--image P]* [--audio P]* [--video P]* [--file P]* "<text>"
            let mut text = String::new();
            let mut attachment_paths: Vec<(PathBuf, &'static str)> = Vec::new();
            let mut iter = args.iter().skip(2);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--image" | "-i" => {
                        if let Some(p) = iter.next() {
                            attachment_paths.push((PathBuf::from(p), "image"));
                        }
                    }
                    "--audio" => {
                        if let Some(p) = iter.next() {
                            attachment_paths.push((PathBuf::from(p), "audio"));
                        }
                    }
                    "--video" => {
                        if let Some(p) = iter.next() {
                            attachment_paths.push((PathBuf::from(p), "video"));
                        }
                    }
                    "--file" => {
                        if let Some(p) = iter.next() {
                            attachment_paths.push((PathBuf::from(p), "file"));
                        }
                    }
                    _ => {
                        if text.is_empty() {
                            text = arg.clone();
                        }
                    }
                }
            }
            if text.is_empty() && attachment_paths.is_empty() {
                eprintln!(
                    "usage: fleety ask [--image PATH]... [--audio PATH]... [--video PATH]... [--file PATH]... \"<message>\""
                );
                return;
            }
            let attachments = match load_attachments(&attachment_paths) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("error: {}", e.report().message);
                    return;
                }
            };
            if let Err(e) = ask(text, attachments).await {
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
        Some("tui") => {
            if let Err(e) = run_tui().await {
                eprintln!("error: {}", e.report().message);
            }
        }
        Some("audit") => {
            let sub = args.get(2).cloned().unwrap_or_default();
            match sub.as_str() {
                "list" => {
                    let limit = args.get(3).and_then(|s| s.parse::<u32>().ok());
                    if let Err(e) = audit_list(limit).await {
                        eprintln!("error: {}", e.report().message);
                    }
                }
                "show" => {
                    let index = args.get(3).and_then(|s| s.parse::<u64>().ok());
                    match index {
                        Some(i) => {
                            if let Err(e) = audit_show(i).await {
                                eprintln!("error: {}", e.report().message);
                            }
                        }
                        None => eprintln!("usage: fleety audit show <index>"),
                    }
                }
                _ => eprintln!("usage: fleety audit list [<limit>]  |  fleety audit show <index>"),
            }
        }
        Some("rollback") => {
            let sub = args.get(2).cloned().unwrap_or_default();
            match sub.as_str() {
                "list" => {
                    if let Err(e) = rollback_list().await {
                        eprintln!("error: {}", e.report().message);
                    }
                }
                "apply" => {
                    let id = args.get(3).cloned().unwrap_or_default();
                    if id.is_empty() {
                        eprintln!("usage: fleety rollback apply <backup_id>");
                    } else if let Err(e) = rollback_apply(id).await {
                        eprintln!("error: {}", e.report().message);
                    }
                }
                _ => eprintln!("usage: fleety rollback list  |  fleety rollback apply <backup_id>"),
            }
        }
        Some("status") => {
            if let Err(e) = status().await {
                eprintln!("error: {}", e.report().message);
            }
        }
        Some("voice") => {
            if let Err(e) = voice_chat().await {
                let report = e.report();
                eprintln!("error: {}", report.message);
                if let Some(hint) = report.remediation {
                    eprintln!("hint: {hint}");
                }
            }
        }
        Some("pair") => {
            let code = args.get(2).cloned().unwrap_or_default();
            if code.is_empty() {
                eprintln!(
                    "usage: fleety pair <pairing-code>   (from `pair_create` on a paired device)"
                );
                return;
            }
            if let Err(e) = pair(code).await {
                eprintln!("error: {}", e.report().message);
            }
        }
        _ => {
            println!(
                "fleety {} — try: fleety ask \"hello\"  |  fleety voice  |  fleety tui  |  fleety pair <code>",
                agent_core::VERSION
            );
        }
    }
}

/// Interactive TUI: connect, then loop over key events and server frames.
async fn run_tui() -> Result<()> {
    use ratatui::crossterm::event::{Event, KeyEventKind};

    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();
    send(&mut tx, &hello(None)).await?;

    // Blocking key reads happen on a thread and arrive over a channel.
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || loop {
        match ratatui::crossterm::event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                if key_tx.send(k).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    let mut terminal = ratatui::init();
    let mut app = tui::App::new(format!("connected to {url}"));
    let result = loop {
        if let Err(e) = terminal.draw(|f| tui::render(f, &app)) {
            break Err(CoreError::Message(format!("draw failed: {e}")));
        }
        if app.should_quit {
            break Ok(());
        }
        tokio::select! {
            key = key_rx.recv() => match key {
                Some(k) => match tui::on_key(&mut app, k) {
                    tui::Action::Send { text, attachments } => {
                        app.status = "sent; waiting…".to_string();
                        if let Err(e) = send(&mut tx, &ClientMsg::UserMessage {
                            conversation_id: None,
                            text,
                            origin: OriginContext::default(),
                            attachments,
                            voice: false,
                            acting_user: None,
                        }).await {
                            app.status = format!("send failed: {}", e.report().message);
                        }
                    }
                    tui::Action::PasteFromClipboard => {
                        // Clipboard I/O on a thread so the TUI event loop never
                        // blocks on slow / large reads.
                        let result = tokio::task::spawn_blocking(clipboard::read)
                            .await
                            .unwrap_or(clipboard::ClipboardPaste::Empty);
                        match result {
                            clipboard::ClipboardPaste::Image(att)
                            | clipboard::ClipboardPaste::File(att) => {
                                app.attach(att);
                            }
                            clipboard::ClipboardPaste::Text(text) => {
                                app.input.push_str(&text);
                                app.status = "pasted text".to_string();
                            }
                            clipboard::ClipboardPaste::Empty => {
                                app.status = "clipboard empty / unavailable".to_string();
                            }
                        }
                    }
                    tui::Action::Quit => app.should_quit = true,
                    tui::Action::None => {}
                },
                None => app.should_quit = true,
            },
            frame = rx.next() => match frame {
                Some(Ok(f)) if f.is_text() => {
                    if let Ok(text) = f.to_text() {
                        match serde_json::from_str::<ServerMsg>(text) {
                            Ok(ServerMsg::AssistantDelta { chunk, .. }) => {
                                app.push_delta(&chunk);
                                app.status = "streaming…".to_string();
                            }
                            Ok(ServerMsg::Assistant { text, .. }) => {
                                app.finish_assistant(text);
                                app.status = "ready".to_string();
                            }
                            Ok(ServerMsg::Error { error }) => {
                                app.status = format!("agent error: {}", error.message);
                            }
                            _ => {}
                        }
                    }
                }
                Some(Ok(_)) => {}
                _ => {
                    app.status = "disconnected".to_string();
                    app.should_quit = true;
                }
            },
        }
    };
    ratatui::restore();
    let _ = tx.close().await;
    result
}

fn fleety_dir() -> Option<PathBuf> {
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(base).join(".fleety"))
}

/// Resolve the agent URL: `FLEETY_AGENT_URL`, else saved config, else mDNS
/// discovery on the LAN, else the local default. mDNS probe is short (2 s) so
/// an offline laptop doesn't pause noticeably before falling through.
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
    if let Some(url) = discover_via_mdns(std::time::Duration::from_secs(2)) {
        return url;
    }
    "ws://127.0.0.1:8787".to_string()
}

/// Browse the LAN for a `_fleety._tcp.local.` service and return the first
/// `ws://host:port` URL. None on timeout, error, or when the user disables
/// mDNS via FLEETY_MDNS_DISABLED.
fn discover_via_mdns(timeout: std::time::Duration) -> Option<String> {
    if std::env::var("FLEETY_MDNS_DISABLED").is_ok() {
        return None;
    }
    let daemon = mdns_sd::ServiceDaemon::new().ok()?;
    let receiver = daemon.browse("_fleety._tcp.local.").ok()?;
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let recv_timeout = remaining.min(std::time::Duration::from_millis(500));
        match receiver.recv_timeout(recv_timeout) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let addrs = info.get_addresses_v4();
                if let Some(ip) = addrs.iter().next() {
                    let url = format!("ws://{}:{}", ip, info.get_port());
                    let _ = daemon.shutdown();
                    return Some(url);
                }
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    None
}

fn device_id() -> String {
    std::env::var("FLEETY_DEVICE_ID")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "cli-device".to_string())
}

/// The auth token saved in config (or `FLEETY_TOKEN`), for authenticated connects.
fn saved_token() -> Option<String> {
    if let Ok(tok) = std::env::var("FLEETY_TOKEN") {
        if !tok.is_empty() {
            return Some(tok);
        }
    }
    let dir = fleety_dir()?;
    let text = std::fs::read_to_string(dir.join("config.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("token")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Build a Hello carrying our saved token (and an optional pairing code).
fn hello(pairing_code: Option<String>) -> ClientMsg {
    ClientMsg::Hello {
        device_id: device_id(),
        protocol: PROTOCOL_VERSION,
        token: saved_token(),
        pairing_code,
        // CLI sessions have no on-device tool registry to advertise — only
        // fleetyd does (it runs tools locally).
        local_tools_json: None,
    }
}

/// Persist config, preserving fields not being changed.
fn write_config(agent_url: Option<&str>, token: Option<&str>) -> Result<()> {
    let dir =
        fleety_dir().ok_or_else(|| CoreError::Message("no home dir for config".to_string()))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoreError::Message(format!("cannot create ~/.fleety: {e}")))?;
    let path = dir.join("config.json");
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let obj = value
        .as_object_mut()
        .ok_or_else(|| CoreError::Message("corrupt config.json".to_string()))?;
    obj.insert("device_id".to_string(), serde_json::json!(device_id()));
    if let Some(url) = agent_url {
        obj.insert("agent_url".to_string(), serde_json::json!(url));
    }
    if let Some(tok) = token {
        obj.insert("token".to_string(), serde_json::json!(tok));
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )
    .map_err(|e| CoreError::Message(format!("cannot write config: {e}")))?;
    Ok(())
}

/// `fleety pair <code>`: enroll this device with a pairing code; saves the token.
async fn pair(code: String) -> Result<()> {
    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();
    send(&mut tx, &hello(Some(code))).await?;
    let result = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            token: Some(tok), ..
        }) => {
            write_config(Some(&url), Some(&tok))?;
            println!("✓ paired with {url}; token saved");
            Ok(())
        }
        Some(ServerMsg::Welcome { token: None, .. }) => Err(CoreError::Message(
            "server returned no token (is it running with FLEETY_REQUIRE_AUTH=1?)".to_string(),
        )),
        Some(ServerMsg::Error { error }) => Err(CoreError::Provider(format!(
            "pairing failed: {}",
            error.message
        ))),
        other => Err(CoreError::Provider(format!(
            "unexpected reply during pair: {other:?}"
        ))),
    };
    let _ = tx.close().await;
    result
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

    send(&mut tx, &hello(None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome { session_id, .. }) => {
            write_config(Some(&url), None)?;
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

/// Read each attachment path from disk and base64-encode it. The CLI knows the
/// rough kind (image/audio/video/file) the user named at the flag; we use the
/// extension to pick a more precise MIME so the provider routes correctly.
fn load_attachments(paths: &[(PathBuf, &'static str)]) -> Result<Vec<WireAttachment>> {
    let mut out = Vec::with_capacity(paths.len());
    for (path, kind) in paths {
        let bytes = std::fs::read(path).map_err(|e| {
            CoreError::Message(format!("cannot read attachment '{}': {e}", path.display()))
        })?;
        let mime = guess_mime(path, kind);
        let name = path.file_name().and_then(|n| n.to_str()).map(String::from);
        use base64::Engine;
        out.push(WireAttachment {
            mime,
            bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
            url: None,
            name,
        });
    }
    Ok(out)
}

/// Best-effort MIME from a file extension + the flag kind the user used.
/// Falls back to `<kind>/octet-stream` so the server still classifies it.
fn guess_mime(path: &Path, kind: &str) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let from_ext = match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "heic" => Some("image/heic"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "m4a" => Some("audio/mp4"),
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        "pdf" => Some("application/pdf"),
        "txt" => Some("text/plain"),
        "json" => Some("application/json"),
        _ => None,
    };
    if let Some(m) = from_ext {
        return m.to_string();
    }
    match kind {
        "image" => "image/octet-stream".to_string(),
        "audio" => "audio/octet-stream".to_string(),
        "video" => "video/octet-stream".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

async fn ask(text: String, attachments: Vec<WireAttachment>) -> Result<()> {
    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    send(&mut tx, &hello(None)).await?;
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
            attachments,
            voice: false,
            acting_user: None,
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
            Some(ServerMsg::Welcome { .. })
            | Some(ServerMsg::Replay { .. })
            | Some(ServerMsg::RunTool { .. })
            | Some(ServerMsg::AssistantDelta { .. })
            | Some(ServerMsg::AuditListResult { .. })
            | Some(ServerMsg::AuditShowResult { .. })
            | Some(ServerMsg::RollbackListResult { .. })
            | Some(ServerMsg::RollbackResult { .. })
            | Some(ServerMsg::ConversationRolled { .. })
            | Some(ServerMsg::ServerStatusResult { .. }) => {}
        }
    }
    // Close the connection gracefully so the server sees a clean disconnect.
    let _ = tx.close().await;
    Ok(())
}

/// Voice mode: a spoken conversation. Each turn captures input via OS dictation
/// (falling back to typing where the OS has no headless STT), sends it with the
/// `voice` flag on, prints the reply, and speaks the spoken channel aloud. The
/// agent only produces a spoken version on the terminal turn, so one summary is
/// read per completed request, not one per intermediate step.
async fn voice_chat() -> Result<()> {
    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    send(&mut tx, &hello(None)).await?;
    let conversation = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            conversation_id, ..
        }) => conversation_id,
        other => {
            return Err(CoreError::Provider(format!(
                "expected welcome, got {other:?}"
            )))
        }
    };

    println!("Voice mode — speak your message (say or type 'quit' to exit).");
    loop {
        // Capture input: OS dictation if available, else fall back to typing.
        let input = match voice::listen() {
            Some(spoken) => {
                println!("you: {spoken}");
                spoken
            }
            None => {
                print!("(dictation unavailable — type your message) > ");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line).is_err() {
                    break;
                }
                line.trim().to_string()
            }
        };
        if input.is_empty() || input.eq_ignore_ascii_case("quit") {
            break;
        }

        send(
            &mut tx,
            &ClientMsg::UserMessage {
                conversation_id: Some(conversation.clone()),
                text: input,
                origin: origin(),
                attachments: Vec::new(),
                voice: true,
                acting_user: None,
            },
        )
        .await?;

        loop {
            match recv(&mut rx).await? {
                Some(ServerMsg::Assistant {
                    text,
                    speech,
                    attention,
                    ..
                }) => {
                    println!("{text}");
                    // Read the spoken channel aloud; falls back to silence if no
                    // engine or no spoken version was produced.
                    if let Some(spoken) = speech {
                        voice::speak(&spoken);
                    }
                    // Device-deixis: point the user at the named device/target.
                    if let Some(a) = attention {
                        match a.url {
                            Some(url) => println!("→ look at {} on {}: {url}", a.look_at, a.device),
                            None => println!("→ look at {} on {}", a.look_at, a.device),
                        }
                    }
                }
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
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    let decision = if line.trim().eq_ignore_ascii_case("y") {
                        ClientMsg::Approve { approval_id }
                    } else {
                        ClientMsg::Deny { approval_id }
                    };
                    send(&mut tx, &decision).await?;
                }
                _ => {}
            }
        }
    }
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

    send(&mut tx, &hello(None)).await?;
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

/// Open a connection, send Hello, await Welcome, return the streams. Common
/// preamble for audit/rollback commands.
async fn connect_hello() -> Result<(Tx, Rx)> {
    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();
    send(&mut tx, &hello(None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome { .. }) => Ok((tx, rx)),
        other => Err(CoreError::Provider(format!(
            "expected welcome, got {other:?}"
        ))),
    }
}

/// Render a past unix timestamp as a short relative span (`5s ago`, `2h ago`,
/// `3d ago`). `0` means "no timestamp recorded" — typically a legacy audit
/// line written before this field existed; we surface it as `—` so the user
/// sees an honest hole rather than a fake "0s ago".
fn format_relative(now: u64, ts: u64) -> String {
    if ts == 0 || now < ts {
        return "—".to_string();
    }
    let diff = now - ts;
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86_400)
    }
}

async fn audit_list(limit: Option<u32>) -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(
        &mut tx,
        &ClientMsg::AuditList {
            device_id: device_id(),
            since: None,
            limit: Some(limit.unwrap_or(50)),
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::AuditListResult { entries_json, .. }) => {
            let entries: Vec<serde_json::Value> =
                serde_json::from_str(&entries_json).unwrap_or_default();
            if entries.is_empty() {
                println!("(no audit entries)");
            } else {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                for entry in &entries {
                    let idx = entry.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
                    let kind = entry.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
                    let tool = entry.get("tool").and_then(|v| v.as_str()).unwrap_or("");
                    let ts = entry.get("ts_secs").and_then(|v| v.as_u64()).unwrap_or(0);
                    let when = format_relative(now, ts);
                    if tool.is_empty() {
                        println!("[{idx:>5}] {when:>8}  {kind}");
                    } else {
                        println!("[{idx:>5}] {when:>8}  {kind:<12} {tool}");
                    }
                }
            }
        }
        Some(ServerMsg::Error { error }) => {
            eprintln!("agent error: {}", error.message);
        }
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

async fn audit_show(index: u64) -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(
        &mut tx,
        &ClientMsg::AuditShow {
            device_id: device_id(),
            index,
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::AuditShowResult { event_json, .. }) => {
            let value: serde_json::Value = serde_json::from_str(&event_json).unwrap_or_default();
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or(event_json)
            );
        }
        Some(ServerMsg::Error { error }) => {
            eprintln!("agent error: {}", error.message);
        }
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

async fn rollback_list() -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(
        &mut tx,
        &ClientMsg::RollbackList {
            device_id: device_id(),
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::RollbackListResult { backups_json, .. }) => {
            let backups: Vec<serde_json::Value> =
                serde_json::from_str(&backups_json).unwrap_or_default();
            if backups.is_empty() {
                println!("(no backups)");
            } else {
                for b in &backups {
                    let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let path = b
                        .get("original_rel_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ts = b.get("ts_secs").and_then(|v| v.as_u64()).unwrap_or(0);
                    println!("{id}  ({ts})  {path}");
                }
            }
        }
        Some(ServerMsg::Error { error }) => {
            eprintln!("agent error: {}", error.message);
        }
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

async fn status() -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(&mut tx, &ClientMsg::ServerStatus).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::ServerStatusResult {
            version,
            uptime_secs,
            connected_devices,
            device_ids_json,
            extra_json,
        }) => {
            let ids: Vec<String> = serde_json::from_str(&device_ids_json).unwrap_or_default();
            println!("fleety-server");
            println!("  version:        {version}");
            println!("  uptime:         {}", format_uptime(uptime_secs));
            println!("  connected:      {connected_devices} device(s)");
            if !ids.is_empty() {
                println!("  device ids:     {}", ids.join(", "));
            }
            if let Some(extra) = extra_json {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&extra) {
                    if let Some(sidecars) = value.get("sidecars").and_then(|s| s.as_object()) {
                        for (name, info) in sidecars {
                            let status = info.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                            let suffix = info
                                .get("path")
                                .and_then(|p| p.as_str())
                                .map(|p| format!(" ({p})"))
                                .unwrap_or_default();
                            println!("  {name:<14}  {status}{suffix}");
                        }
                    }
                }
            }
        }
        Some(ServerMsg::Error { error }) => eprintln!("agent error: {}", error.message),
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    let s = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {mins}m")
    } else if hours > 0 {
        format!("{hours}h {mins}m {s}s")
    } else if mins > 0 {
        format!("{mins}m {s}s")
    } else {
        format!("{s}s")
    }
}

async fn rollback_apply(backup_id: String) -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(
        &mut tx,
        &ClientMsg::RollbackApply {
            device_id: device_id(),
            backup_id: backup_id.clone(),
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::RollbackResult { ok, message, .. }) => {
            if ok {
                println!("✓ {message}");
            } else {
                eprintln!("✗ rollback failed: {message}");
            }
        }
        Some(ServerMsg::Error { error }) => {
            eprintln!("agent error: {}", error.message);
        }
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
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

#[cfg(test)]
mod tests {
    use super::{format_relative, format_uptime};

    #[test]
    fn uptime_picks_the_right_band() {
        assert_eq!(format_uptime(45), "45s");
        assert_eq!(format_uptime(125), "2m 5s");
        assert_eq!(format_uptime(3_725), "1h 2m 5s");
        assert_eq!(format_uptime(180_000), "2d 2h 0m");
    }

    #[test]
    fn relative_renders_each_band() {
        // Now = 1_000_000.
        assert_eq!(format_relative(1_000_000, 999_990), "10s ago");
        assert_eq!(format_relative(1_000_000, 999_700), "5m ago");
        assert_eq!(format_relative(1_000_000, 996_400), "1h ago");
        assert_eq!(format_relative(1_000_000, 913_600), "1d ago");
    }

    #[test]
    fn relative_handles_legacy_and_clock_skew() {
        // Zero means "no timestamp on this line" — show a dash, not "0s ago".
        assert_eq!(format_relative(1_000_000, 0), "—");
        // Clock skew (ts in the future) is also rendered as a dash so we don't
        // print nonsense like "-3s ago".
        assert_eq!(format_relative(1_000_000, 2_000_000), "—");
    }
}
