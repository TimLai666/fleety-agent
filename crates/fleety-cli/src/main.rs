//! fleety — the Fleety CLI.
//!
//! M2: `fleety ask "<message>"` connects to the Agent over WebSocket, does one
//! conversation round-trip, and prints the reply. Interactive TUI comes later.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod acp;
mod auth;
mod clipboard;
mod config;
mod config_panel;
mod input;
mod markdown;
mod provider_tui;
mod server;
mod tui;
mod voice;

use std::path::{Path, PathBuf};

use agent_core::{obs, CoreError, Result};
use fleety_protocol::{
    ClientMsg, ConfigTarget, Effect, OriginContext, ServerMsg, WireAttachment, PROTOCOL_VERSION,
};
// The client transport (WebSocket with SSE+POST fallback) lives in fleety-tools;
// `Tx`/`Rx` are its split halves so the existing connect sites barely change.
use fleety_tools::connection::{self, Target};
use fleety_tools::transport::{self, Receiver as Rx, Sender as Tx};

/// Print an error report (message + hint when present); yields the failure code
/// so every command reports failure the same way — and scripts can rely on it.
fn fail(e: CoreError) -> std::process::ExitCode {
    let report = e.report();
    eprintln!("error: {}", report.message);
    if let Some(hint) = report.remediation {
        eprintln!("hint: {hint}");
    }
    std::process::ExitCode::FAILURE
}

/// Map a command result to the process exit code (0 ok, 1 failure).
fn done(res: Result<()>) -> std::process::ExitCode {
    match res {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

/// Usage error: print to stderr, exit 2 (distinct from runtime failures).
fn usage(msg: &str) -> std::process::ExitCode {
    eprintln!("{msg}");
    std::process::ExitCode::from(2)
}

/// The full command list (aligned with the README command reference).
fn print_help() {
    println!("fleety {} — the Fleety CLI", agent_core::VERSION);
    println!();
    println!("usage: fleety <command> [args]");
    println!();
    println!("  init <ws-url>                    add+use a server (sugar for `server add --use`)");
    println!("  server <add|use|list|show|current|rename|remove|set-url>");
    println!("                                   manage which server(s) this device connects to");
    println!("  ask \"<text>\" [--image|--audio|--video|--file PATH]...");
    println!("                                   one-shot prompt (with attachments)");
    println!("  resume <conversation_id> [after_seq]");
    println!("                                   continue an existing conversation");
    println!("  conversations [<limit>]          list recent conversations to resume");
    println!("  tui                              interactive terminal UI");
    println!("  voice                            voice conversation");
    println!("  status                           server health: version, uptime, devices");
    println!("  config <list|get|set|unset|edit> [--target server|local|<device-id>]");
    println!("  config provider|model <...>      manage providers + model roles (providers.toml)");
    println!("  auth <login|status|logout>       ChatGPT/Codex OAuth sign-in");
    println!("  audit list [<limit>]             this device's audit-log entries");
    println!("  audit show <index>               one audit entry in full");
    println!("  rollback list                    backups available to restore");
    println!("  rollback apply <backup_id>       restore a file from a backup");
    println!("  pair <code>                      enroll this device (auth-required servers)");
    println!("  daemon <verb>                    manage the local daemon (install/start/...)");
    println!("  update                           update every fleety component on this host");
    println!("  acp [install [zed]]              run as an ACP agent (editors launch this)");
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    obs::init();
    // Seed env from ~/.fleety/config.toml so client settings (e.g. transport mode)
    // set via `fleety config` apply; an explicit env var still wins.
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    // One-time, idempotent migration of the legacy config.json into
    // connections.toml (best-effort — a fresh device has nothing to migrate).
    let _ = connection::migrate_from_config_json();
    // Pull a leading per-invocation server override (`fleety -s <name> …` /
    // `fleety --url <ws> …`) out of the args, so it applies to this command only.
    let (args, target) = take_server_override(std::env::args().collect());
    let _ = OVERRIDE.set(target);
    match args.get(1).map(String::as_str) {
        Some("init") => {
            // `fleety init <ws-url> [--name <name>]` — positional url plus an
            // optional profile name (default `default`).
            let mut url = String::new();
            let mut name = "default".to_string();
            let mut it = args.iter().skip(2);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--name" => {
                        if let Some(n) = it.next() {
                            name = n.clone();
                        }
                    }
                    _ if url.is_empty() => url = a.clone(),
                    _ => {}
                }
            }
            if url.is_empty() {
                return usage(
                    "usage: fleety init <ws-url> [--name <name>]   (e.g. ws://host:8787)",
                );
            }
            // Catch the common scheme mistakes before any network work — the
            // raw connect error that would follow is much harder to act on.
            if !url.starts_with("ws://") && !url.starts_with("wss://") {
                if url.starts_with("http://") || url.starts_with("https://") {
                    eprintln!(
                        "error: '{url}' is an http(s) URL — the agent URL uses the WebSocket scheme"
                    );
                    eprintln!("hint: use ws:// (or wss:// behind TLS), e.g. ws://host:8787");
                } else {
                    eprintln!("error: '{url}' is not a ws:// or wss:// URL");
                    eprintln!("hint: e.g. fleety init ws://192.168.1.10:8787");
                }
                return std::process::ExitCode::from(2);
            }
            done(init(url, name).await)
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
                return usage(
                    "usage: fleety ask [--image PATH]... [--audio PATH]... [--video PATH]... [--file PATH]... \"<message>\"",
                );
            }
            let attachments = match load_attachments(&attachment_paths) {
                Ok(a) => a,
                Err(e) => return fail(e),
            };
            done(ask(text, attachments).await)
        }
        Some("resume") => {
            let conversation_id = args.get(2).cloned().unwrap_or_default();
            let after_seq = args.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            if conversation_id.is_empty() {
                return usage("usage: fleety resume <conversation_id> [after_seq]");
            }
            done(resume(conversation_id, after_seq).await)
        }
        Some("tui") => done(run_tui().await),
        Some("conversations") => {
            // `fleety conversations [<limit>]` — list recent conversations so the
            // user can find the id `fleety resume` needs.
            let limit = args.get(2).and_then(|s| s.parse::<u32>().ok());
            done(conversations(limit).await)
        }
        Some("audit") => {
            let sub = args.get(2).cloned().unwrap_or_default();
            match sub.as_str() {
                "list" => {
                    let limit = args.get(3).and_then(|s| s.parse::<u32>().ok());
                    done(audit_list(limit).await)
                }
                "show" => match args.get(3).and_then(|s| s.parse::<u64>().ok()) {
                    Some(i) => done(audit_show(i).await),
                    None => usage("usage: fleety audit show <index>"),
                },
                _ => usage("usage: fleety audit list [<limit>]  |  fleety audit show <index>"),
            }
        }
        Some("rollback") => {
            let sub = args.get(2).cloned().unwrap_or_default();
            match sub.as_str() {
                "list" => done(rollback_list().await),
                "apply" => {
                    let id = args.get(3).cloned().unwrap_or_default();
                    if id.is_empty() {
                        usage("usage: fleety rollback apply <backup_id>")
                    } else {
                        done(rollback_apply(id).await)
                    }
                }
                _ => usage("usage: fleety rollback list  |  fleety rollback apply <backup_id>"),
            }
        }
        Some("server") => done(server::run(&args[2..])),
        Some("status") => done(status().await),
        Some("voice") => done(voice_chat().await),
        Some("config") => {
            // `--target server` (default) manages the connected server's config
            // over the connection; `--target local` (and interactive `edit`) edit
            // this host's own files. `--target <device-id>` is sent to the server
            // (which reports it as a follow-up for now).
            let (target, rest) = config::split_target(&args[2..]);
            // Bare `fleety config` on a TTY → the three-region interactive panel
            // (connection / this device / server); no `--target` needed.
            let res = if rest.is_empty() && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                config_panel::run().await
            } else if config::is_interactive_edit(&rest) || matches!(target, ConfigTarget::Local) {
                config::run(&rest)
            } else {
                config_remote(target, &rest).await
            };
            done(res)
        }
        Some("auth") => {
            // `fleety auth <login|status|logout>` — Codex ChatGPT OAuth sign-in.
            done(auth::run(&args[2..]).await)
        }
        Some("daemon") => {
            // Drive the local daemon from the unified CLI: `fleety daemon <verb>`
            // forwards to the `fleetyd` binary (install/start/stop/status/update/…).
            let sub = &args[2..];
            if sub.is_empty() {
                usage(
                    "usage: fleety daemon <install|uninstall|start|stop|restart|enable|disable|status|up|down|update>",
                )
            } else {
                done(daemon_delegate(sub))
            }
        }
        Some("update") => {
            // Update every fleety component installed on this host (CLI + any
            // local server + daemon). One command, per the unified update model.
            done(update_all().await)
        }
        Some("acp") => {
            // `fleety acp install [--server <url>]` writes the Zed agent-server
            // config; plain `fleety acp` runs the adapter over stdio (stdout is
            // only JSON-RPC, logs go to stderr, so the editor's parser is safe).
            if args.get(2).map(String::as_str) == Some("install") {
                let server = args
                    .iter()
                    .position(|a| a == "--server")
                    .and_then(|i| args.get(i + 1))
                    .cloned();
                // `fleety acp install [<editor>]` — <editor> (e.g. `zed`) auto-
                // configures that editor; with none, print the generic setup that
                // works with any ACP-capable editor.
                let target = args.get(3).filter(|a| !a.starts_with("--")).cloned();
                done(acp::install(target, server))
            } else {
                done(acp::run(agent_url()).await)
            }
        }
        Some("pair") => {
            let code = args.get(2).cloned().unwrap_or_default();
            if code.is_empty() {
                return usage(
                    "usage: fleety pair <pairing-code>   (from `pair_create` on a paired device)",
                );
            }
            done(pair(code).await)
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            std::process::ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command '{other}' — run `fleety help` for the full list");
            std::process::ExitCode::from(2)
        }
        None => {
            print_help();
            std::process::ExitCode::SUCCESS
        }
    }
}

/// A local fleety binary `bin` installed as a sibling of the running `fleety`.
fn sibling_bin(bin: &str) -> Option<std::path::PathBuf> {
    fleety_tools::update::sibling_exe(bin)
}

/// Resolve the local `fleetyd` binary: prefer a sibling of `fleety`, else PATH.
fn daemon_binary() -> std::path::PathBuf {
    sibling_bin("fleetyd").unwrap_or_else(|| std::path::PathBuf::from("fleetyd"))
}

/// `fleety update`: update every fleety component installed on THIS host. Updates
/// the CLI itself, then the server binary (restarting its service), then delegates
/// the daemon to `fleetyd update` (which also refreshes the insyra sidecar).
/// Remote components are updated by running `fleety update` on their own host.
async fn update_all() -> Result<()> {
    println!("Updating fleety (CLI)…");
    fleety_tools::update::self_update().await?;

    // fleety-server: update the binary, then restart its service. Resolving its
    // artifact needs a `{bin}` manifest template (a plain URL is the CLI's own).
    if let Some(exe) = sibling_bin("fleety-server") {
        if fleety_tools::update::manifest_is_templated() {
            match fleety_tools::update::update_named("fleety-server", &exe).await {
                Ok(true) => {
                    // Bare `restart` (no --force) → the running server defers the
                    // restart until it is idle rather than interrupting a turn.
                    println!(
                        "fleety-server updated — requesting a restart. The running server \
                         restarts once it is idle (no in-flight turn), or after the deferral \
                         deadline; an interrupted turn is recovered from the journal, not lost."
                    );
                    let _ = std::process::Command::new(&exe).arg("restart").status();
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!(
                        "warning: fleety-server update failed: {}",
                        e.report().message
                    )
                }
            }
        } else {
            println!(
                "note: set FLEETY_UPDATE_MANIFEST to a URL containing {{bin}} to also update fleety-server."
            );
        }
    }

    // fleetyd: delegate to its own complete update (binary + insyra + restart).
    if let Some(exe) = sibling_bin("fleetyd") {
        let _ = std::process::Command::new(&exe).arg("update").status();
    }

    // Self-heal any already-installed ACP agent configs (e.g. Zed) so they point
    // at this binary — in case the path changed or the `acp` invocation evolved.
    acp::refresh_installed(None);
    Ok(())
}

/// Run `fleetyd <args...>` to manage the local daemon from the CLI. Inherits
/// stdio (so the daemon's output shows through) and forwards a non-zero exit.
fn daemon_delegate(args: &[String]) -> Result<()> {
    let program = daemon_binary();
    let status = std::process::Command::new(&program)
        .args(args)
        .status()
        .map_err(|e| {
            CoreError::Message(format!(
                "cannot run the daemon binary ({}): {e}. Is fleetyd installed and on PATH?",
                program.display()
            ))
        })?;
    if !status.success() {
        return Err(CoreError::Message(format!(
            "fleetyd {} exited unsuccessfully ({status})",
            args.first().map(String::as_str).unwrap_or("")
        )));
    }
    Ok(())
}

/// Interactive TUI: connect, then loop over key events and server frames.
async fn run_tui() -> Result<()> {
    use ratatui::crossterm::event::{Event, KeyEventKind};

    let (mut tx, mut rx, target) = open().await?;
    let url = target.url.clone();
    let token = target.token.clone();
    send(&mut tx, &hello(token.clone(), None)).await?;

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
    // Redraw only when something changed — a key/frame event, or a spinner tick
    // while waiting. Idle ticks must not force periodic repaints (the spinner is
    // static when no turn is in flight).
    let mut dirty = true;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(120));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = loop {
        if dirty {
            if let Err(e) = terminal.draw(|f| tui::render(f, &app)) {
                break Err(CoreError::Message(format!("draw failed: {e}")));
            }
            dirty = false;
        }
        if app.should_quit {
            break Ok(());
        }
        tokio::select! {
            key = key_rx.recv() => {
                dirty = true;
                match key {
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
                                app.input.insert_str(&text);
                                app.status = "pasted text".to_string();
                            }
                            clipboard::ClipboardPaste::Empty => {
                                app.status = "clipboard empty / unavailable".to_string();
                            }
                        }
                    }
                    tui::Action::Approve(approval_id) => {
                        if let Err(e) = send(&mut tx, &ClientMsg::Approve { approval_id }).await {
                            app.status = format!("approve failed: {}", e.report().message);
                        }
                    }
                    tui::Action::Deny(approval_id) => {
                        if let Err(e) = send(&mut tx, &ClientMsg::Deny { approval_id }).await {
                            app.status = format!("deny failed: {}", e.report().message);
                        }
                    }
                    tui::Action::CancelTurn => {
                        if let Err(e) = send(&mut tx, &ClientMsg::CancelTurn {
                            conversation_id: None,
                        }).await {
                            app.status = format!("cancel failed: {}", e.report().message);
                        }
                    }
                    tui::Action::Quit => app.should_quit = true,
                    tui::Action::None => {}
                },
                None => app.should_quit = true,
                }
            }
            frame = rx.recv_text() => {
                dirty = true;
                match frame {
                Some(text) => match serde_json::from_str::<ServerMsg>(&text) {
                    Ok(ServerMsg::AssistantDelta { chunk, conversation_id }) => {
                        // Track the id even mid-stream so a reconnect can Resume.
                        app.last_conversation_id = Some(conversation_id);
                        app.push_delta(&chunk);
                        app.status = "streaming…".to_string();
                    }
                    Ok(ServerMsg::Assistant { text, conversation_id, seq, .. }) => {
                        app.note_seq(&conversation_id, seq);
                        app.finish_assistant(text);
                        // Surface the id — it's what `fleety resume` needs later.
                        app.status = format!("ready — conversation {conversation_id}");
                    }
                    Ok(ServerMsg::Replay { conversation_id, seq, role, content }) => {
                        // Reconnect replay: apply only events we haven't shown.
                        if app.apply_replay(&conversation_id, seq, &role, &content) {
                            app.status = format!("restored — conversation {conversation_id}");
                        }
                    }
                    Ok(ServerMsg::ApprovalRequested { approval_id, tool, risk, summary }) => {
                        app.request_approval(approval_id, &tool, &risk, &summary);
                    }
                    Ok(ServerMsg::RunTool { call_id, tool, .. }) => {
                        // Viewer connection, no daemon: decline instead of letting
                        // the server wait out its 30 s dispatch timeout.
                        let error = fleety_protocol::WireError {
                            kind: "unsupported".to_string(),
                            message: format!(
                                "'{tool}' was dispatched to this device, but it is connected \
                                 via the TUI, which does not run on-device tools"
                            ),
                            remediation: Some(
                                "run fleetyd on this device, or target a device that runs \
                                 the daemon"
                                    .to_string(),
                            ),
                        };
                        if let Err(e) = send(&mut tx, &ClientMsg::ToolError { call_id, error }).await {
                            app.status = format!("send failed: {}", e.report().message);
                        } else {
                            app.status =
                                format!("declined on-device tool '{tool}' (no daemon here)");
                        }
                    }
                    Ok(ServerMsg::Error { error }) => {
                        // The turn ended (with an error): clear the in-flight
                        // state so Esc goes back to quitting.
                        app.turn_in_flight = false;
                        app.status = format!("agent error: {}", error.message);
                    }
                    _ => {}
                },
                None => {
                    // The link dropped: try to reconnect with capped backoff and
                    // resume the conversation, instead of exiting outright. On a
                    // give-up, reconnect() has already set the status + should_quit.
                    if let Some((new_tx, new_rx)) =
                        reconnect(&url, token.as_deref(), &mut app, &mut terminal, &mut key_rx)
                            .await
                    {
                        tx = new_tx;
                        rx = new_rx;
                    }
                }
                }
            }
            _ = tick.tick() => {
                // Only the waiting state animates; idle ticks cause no redraw.
                if app.turn_in_flight {
                    app.advance_spinner();
                    dirty = true;
                }
            }
        }
    };
    ratatui::restore();
    let _ = tx.close().await;
    result
}

/// Reconnect after a dropped link, using capped exponential backoff. On success
/// it re-sends `Hello`, `Resume`s the active conversation (so the server replays
/// what we missed — de-duplicated by seq in the loop), and returns the fresh
/// streams. The wait between attempts is abortable with Ctrl+C. Returns `None`
/// (and sets `should_quit`) when Ctrl+C aborts or the attempts are exhausted.
async fn reconnect(
    url: &str,
    token: Option<&str>,
    app: &mut tui::App,
    terminal: &mut ratatui::DefaultTerminal,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<ratatui::crossterm::event::KeyEvent>,
) -> Option<(Tx, Rx)> {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    // A dropped link ends any in-flight turn; the spinner marks the wait.
    app.turn_in_flight = false;

    const MAX_ATTEMPTS: u32 = 8;
    const MAX_DELAY_MS: u64 = 30_000;
    let mut delay_ms: u64 = 500;

    for attempt in 1..=MAX_ATTEMPTS {
        app.advance_spinner();
        app.status = format!(
            "{} reconnecting… (attempt {attempt}/{MAX_ATTEMPTS}) — Ctrl+C to quit",
            app.spinner_char()
        );
        let _ = terminal.draw(|f| tui::render(f, app));

        if let Ok(conn) = transport::connect(url, token).await {
            let (mut tx, rx) = conn.split();
            if send(&mut tx, &hello(token.map(String::from), None))
                .await
                .is_ok()
            {
                if let Some(cid) = app.last_conversation_id.clone() {
                    let _ = send(
                        &mut tx,
                        &ClientMsg::Resume {
                            conversation_id: cid,
                            after_seq: app.last_seq,
                        },
                    )
                    .await;
                }
                app.status = "reconnected".to_string();
                return Some((tx, rx));
            }
        }

        // Wait out the backoff, but let Ctrl+C abort the whole wait.
        let sleep = tokio::time::sleep(std::time::Duration::from_millis(delay_ms));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                key = key_rx.recv() => match key {
                    Some(k)
                        if k.modifiers.contains(KeyModifiers::CONTROL)
                            && matches!(k.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&'c')) =>
                    {
                        app.should_quit = true;
                        return None;
                    }
                    // Other keys keep waiting; a closed channel means the input
                    // thread died, so there's nothing left to drive the TUI.
                    Some(_) => continue,
                    None => {
                        app.should_quit = true;
                        return None;
                    }
                },
                _ = &mut sleep => break,
            }
        }
        delay_ms = delay_ms.saturating_mul(2).min(MAX_DELAY_MS);
    }

    app.status = "disconnected — reconnect attempts exhausted".to_string();
    app.should_quit = true;
    None
}

/// The per-invocation server override parsed from a leading `-s`/`--server`/
/// `--url` (set once in `main`; `Target::Current` when none).
static OVERRIDE: std::sync::OnceLock<Target> = std::sync::OnceLock::new();

/// Pull a leading per-invocation server override out of the argument list:
/// `fleety -s <name> …` / `fleety --server <name> …` (select a profile) or
/// `fleety --url <ws> …` (direct connection). Only leading flags (before the
/// subcommand) are consumed, so a later `-s` in a message stays untouched.
/// Returns the cleaned args and the resolved [`Target`].
fn take_server_override(mut args: Vec<String>) -> (Vec<String>, Target) {
    let mut target = Target::Current;
    // args[0] is the program name; overrides come right after it. `i` stays at 1
    // because each match drains the flag+value, shifting the next token into place.
    let i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-s" | "--server" if i + 1 < args.len() => {
                target = Target::Named(args[i + 1].clone());
                args.drain(i..=i + 1);
            }
            "--url" if i + 1 < args.len() => {
                target = Target::Url(args[i + 1].clone());
                args.drain(i..=i + 1);
            }
            _ => break,
        }
    }
    (args, target)
}

/// Resolve which server (url + token) this command connects to, via the shared
/// [`connection::resolve`] over `connections.toml`, honoring the per-invocation
/// override and the `FLEETY_AGENT_URL`/`FLEETY_TOKEN` env vars. Prints a one-line
/// hint when an env override is in effect, when it discovered a server on the
/// LAN, or when it fell through to the localhost default — so a fresh machine
/// gets a next step instead of a bare connection error.
fn resolve_target() -> Result<connection::Resolved> {
    let conns = connection::load()?;
    let over = OVERRIDE.get().cloned().unwrap_or(Target::Current);
    let env_url = std::env::var("FLEETY_AGENT_URL").ok();
    let env_token = std::env::var("FLEETY_TOKEN").ok();
    let r = connection::resolve(&conns, &over, env_url, env_token, || {
        discover_via_mdns(std::time::Duration::from_secs(2)).map(|url| connection::Discovered {
            url,
            // Plain mDNS advertises no server fingerprint yet, so a discovered
            // server never inherits a pinned profile's token (the guard stays
            // closed). Fingerprinted discovery is a later enhancement.
            fingerprint: None,
        })
    })?;
    match &r.source {
        connection::Source::Env => {
            eprintln!(
                "note: FLEETY_AGENT_URL overrides the current server ({})",
                r.url
            )
        }
        connection::Source::Mdns => eprintln!("discovered agent on the LAN: {}", r.url),
        connection::Source::Default => eprintln!(
            "no server configured and none found on the LAN — trying the local default \
             {} (point at one with `fleety init <ws-url>`)",
            r.url
        ),
        connection::Source::Override | connection::Source::Profile(_) => {}
    }
    Ok(r)
}

/// The resolved server URL (used by the ACP bridge, which manages its own
/// connection). Falls back to the localhost default if resolution errors.
fn agent_url() -> String {
    resolve_target()
        .map(|r| r.url)
        .unwrap_or_else(|_| connection::DEFAULT_URL.to_string())
}

/// Resolve + connect in one step: returns the split streams plus the resolved
/// target (so callers can read its url/token). Every non-`init` connect site
/// goes through here so they share one resolution (one mDNS probe, one token).
async fn open() -> Result<(Tx, Rx, connection::Resolved)> {
    let target = resolve_target()?;
    let (tx, rx) = transport::connect(&target.url, target.token.as_deref())
        .await?
        .split();
    Ok((tx, rx, target))
}

/// Add-or-update a named profile's url and make it current — the shared core of
/// `fleety init` (sugar for `server add <name> <url> --use`). Returns the
/// profile's existing token, if any (so a re-init of a paired server keeps
/// authenticating).
fn upsert_profile_and_use(name: &str, url: &str) -> Result<Option<String>> {
    let mut conns = connection::load()?;
    // Persist this device's id on first enrollment so it stays stable regardless
    // of later hostname/env changes.
    if conns.device_id.is_empty() {
        conns.device_id = fleety_tools::device::device_id();
    }
    let profile = conns.profiles.entry(name.to_string()).or_default();
    profile.url = url.to_string();
    let token = profile.token.clone();
    conns.current = Some(name.to_string());
    connection::save(&conns)?;
    Ok(token)
}

/// Write a freshly-minted pairing token onto the current profile (replacing the
/// legacy config.json write). Errors if there is no current server to attach it
/// to (the user should `fleety init <url>` first).
fn set_current_token(token: &str) -> Result<()> {
    let mut conns = connection::load()?;
    let name = conns.current.clone().ok_or_else(|| {
        CoreError::Message(
            "no current server to attach the token to — run `fleety init <ws-url>` first"
                .to_string(),
        )
    })?;
    if let Some(p) = conns.profiles.get_mut(&name) {
        p.token = Some(token.to_string());
    }
    connection::save(&conns)
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
    connection::load()
        .map(|c| c.effective_device_id())
        .unwrap_or_else(|_| fleety_tools::device::device_id())
}

/// Build a Hello carrying the resolved token (and an optional pairing code). The
/// token is passed in so it matches the one the transport connected with.
fn hello(token: Option<String>, pairing_code: Option<String>) -> ClientMsg {
    ClientMsg::Hello {
        device_id: device_id(),
        protocol: PROTOCOL_VERSION,
        token,
        pairing_code,
        // CLI sessions have no on-device tool registry to advertise — only
        // fleetyd does (it runs tools locally).
        local_tools_json: None,
        hostname: fleety_tools::device::hostname(),
    }
}

/// `fleety pair <code>`: enroll this device against the current server; the
/// minted token is written onto the current profile in connections.toml.
async fn pair(code: String) -> Result<()> {
    let (mut tx, mut rx, target) = open().await?;
    let url = target.url.clone();
    send(&mut tx, &hello(target.token.clone(), Some(code))).await?;
    let result = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            token: Some(tok), ..
        }) => {
            set_current_token(&tok)?;
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
        other => Err(CoreError::Provider(unexpected_pair_reply(other.as_ref()))),
    };
    let _ = tx.close().await;
    result
}

/// The wire tag of a server frame (`"assistant"`, `"done"`, …) for human-readable
/// messages — read from the serde `type` tag, never the Debug form (which would
/// dump the internal type's fields).
fn server_msg_kind(msg: &ServerMsg) -> String {
    serde_json::to_value(msg)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unrecognized".to_string())
}

/// A concise, human-readable message for a pairing reply that is neither a
/// successful `Welcome` nor a server `Error` — including the case where the
/// server closed the connection without replying. It names the frame kind but
/// never prints the Debug representation of the internal `ServerMsg` type.
fn unexpected_pair_reply(reply: Option<&ServerMsg>) -> String {
    match reply {
        None => "the server closed the connection without replying to the pairing request; \
                 check the agent URL and that the server is running, then retry"
            .to_string(),
        Some(msg) => format!(
            "the server answered pairing with an unexpected '{}' frame instead of a welcome; \
             check that the URL points at a Fleety agent running a compatible version, then retry",
            server_msg_kind(msg)
        ),
    }
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
        home: std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok()),
    }
}

/// `fleety init <ws-url> [--name <name>]`: sugar for `server add <name> <url>
/// --use` plus enrollment. Records/updates the named profile (default `default`),
/// makes it current, connects, and registers this device.
async fn init(url: String, name: String) -> Result<()> {
    // Persist the profile first (add-or-update + make current), so the connect
    // below and every later command resolve to it.
    let token = upsert_profile_and_use(&name, &url)?;
    let (mut tx, mut rx) = transport::connect(&url, token.as_deref()).await?.split();

    send(&mut tx, &hello(token, None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome { session_id, .. }) => {
            println!("✓ connected to {url}");
            println!(
                "✓ registered device '{}' as server '{name}' (session {session_id})",
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
    let (mut tx, mut rx, target) = open().await?;

    send(&mut tx, &hello(target.token.clone(), None)).await?;
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
            Some(ServerMsg::Done { conversation_id }) => {
                // stderr, so piping the reply stays clean — without this line
                // the id `fleety resume` needs is never shown anywhere.
                eprintln!("(conversation {conversation_id} — continue with: fleety resume {conversation_id})");
                break;
            }
            None => break,
            Some(ServerMsg::Error { error }) => {
                eprintln!("agent error: {}", error.message);
                break;
            }
            Some(ServerMsg::RunTool { call_id, tool, .. }) => {
                // This connection is a viewer (no daemon): decline instead of
                // letting the server wait out its 30 s dispatch timeout.
                let error = fleety_protocol::WireError {
                    kind: "unsupported".to_string(),
                    message: format!(
                        "'{tool}' was dispatched to this device, but it is connected via the \
                         CLI, which does not run on-device tools"
                    ),
                    remediation: Some(
                        "run fleetyd on this device (`fleetyd install` + `fleetyd start`), or \
                         target a device that runs the daemon"
                            .to_string(),
                    ),
                };
                send(&mut tx, &ClientMsg::ToolError { call_id, error }).await?;
            }
            // Credential replies belong to the `fleety auth` command's own
            // request/reply exchange; in the ask loop they are stray noise.
            Some(ServerMsg::CredentialResult { .. })
            | Some(ServerMsg::CredentialStatusResult { .. }) => {}
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
            | Some(ServerMsg::AssistantDelta { .. })
            | Some(ServerMsg::AuditListResult { .. })
            | Some(ServerMsg::AuditShowResult { .. })
            | Some(ServerMsg::ConversationListResult { .. })
            | Some(ServerMsg::RollbackListResult { .. })
            | Some(ServerMsg::RollbackResult { .. })
            | Some(ServerMsg::ConversationRolled { .. })
            | Some(ServerMsg::ServerStatusResult { .. })
            | Some(ServerMsg::ConfigResult { .. })
            | Some(ServerMsg::ConfigSnapshotResult { .. }) => {}
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
/// Capture one utterance as text: OS/Whisper dictation if available, else typed.
/// `None` means the input stream ended — stop the voice loop.
fn capture_voice_text() -> Option<String> {
    match voice::listen() {
        Some(spoken) => {
            println!("you: {spoken}");
            Some(spoken)
        }
        None => {
            print!("(dictation unavailable — type your message) > ");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                return None;
            }
            Some(line.trim().to_string())
        }
    }
}

async fn voice_chat() -> Result<()> {
    let (mut tx, mut rx, target) = open().await?;

    send(&mut tx, &hello(target.token.clone(), None)).await?;
    let (conversation, audio_input) = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            conversation_id,
            audio_input,
            ..
        }) => (conversation_id, audio_input),
        other => {
            return Err(CoreError::Provider(format!(
                "expected welcome, got {other:?}"
            )))
        }
    };
    // Decide once: send audio to an audio-capable model, else transcribe locally.
    let voice_mode = voice::voice_mode(audio_input, voice::voice_audio_setting());

    println!("Voice mode — speak your message (say or type 'quit' to exit).");
    loop {
        // SendAudio: capture compressed audio and let the model transcribe. If
        // capture fails / is oversized, fall back to local transcription. Quit is
        // a text affordance, so audio turns don't run the quit-string check.
        let (text, attachments): (String, Vec<WireAttachment>) = match voice_mode {
            voice::VoiceMode::SendAudio => match voice::capture_audio() {
                Some((bytes, mime)) => {
                    use base64::Engine;
                    println!("you: (sent {} bytes of speech audio)", bytes.len());
                    let att = WireAttachment {
                        mime: mime.to_string(),
                        bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
                        url: None,
                        name: Some("speech.wav".to_string()),
                    };
                    (String::new(), vec![att])
                }
                None => match capture_voice_text() {
                    Some(t) => (t, Vec::new()),
                    None => break,
                },
            },
            voice::VoiceMode::LocalStt => match capture_voice_text() {
                Some(t) => (t, Vec::new()),
                None => break,
            },
        };
        // For text turns, honor an empty/"quit" utterance as exit.
        if attachments.is_empty() && (text.is_empty() || text.eq_ignore_ascii_case("quit")) {
            break;
        }

        send(
            &mut tx,
            &ClientMsg::UserMessage {
                conversation_id: Some(conversation.clone()),
                text,
                origin: origin(),
                attachments,
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
                    // engine or no spoken version was produced. Honor barge-in:
                    // if the user talks over the reply, stop this turn early and
                    // return to the outer loop to capture their utterance.
                    if let Some(spoken) = speech {
                        if voice::speak_interruptible(&spoken) == voice::SpeakOutcome::Interrupted {
                            break;
                        }
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
    let (mut tx, mut rx, target) = open().await?;

    send(&mut tx, &hello(target.token.clone(), None)).await?;
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
    let (mut tx, mut rx, target) = open().await?;
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome { .. }) => Ok((tx, rx)),
        other => Err(CoreError::Provider(format!(
            "expected welcome, got {other:?}"
        ))),
    }
}

/// `connect_hello` for the `auth` command: also returns the server's advertised
/// config protocol (the credential-support gate) and the resolved target, so
/// auth can refuse an old server up front and name the server it acts on.
pub(crate) async fn connect_hello_for_auth() -> Result<(Tx, Rx, u32, connection::Resolved)> {
    let (mut tx, mut rx, target) = open().await?;
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            config_protocol, ..
        }) => Ok((tx, rx, config_protocol, target)),
        other => Err(CoreError::Provider(format!(
            "expected welcome, got {other:?}"
        ))),
    }
}

/// Manage a remote (server / device) host's config over the connection: connect,
/// send `ConfigExec`, print the rendered result and when it takes effect. A
/// connection failure suggests `--target local`.
async fn config_remote(target: ConfigTarget, args: &[String]) -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await.map_err(|e| {
        CoreError::Message(format!(
            "could not reach the server: {} — use `--target local` to edit this host, or set the server URL with `fleety init <ws-url>`",
            e.report().message
        ))
    })?;
    send(
        &mut tx,
        &ClientMsg::ConfigExec {
            target,
            args: args.to_vec(),
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::ConfigResult {
            ok,
            output,
            effect,
            error,
        }) => {
            if ok {
                let out = output.trim_end();
                if !out.is_empty() {
                    println!("{out}");
                }
                if let Some(eff) = effect {
                    println!(
                        "{}",
                        match eff {
                            Effect::NextConnection =>
                                "(applied — takes effect on the next connection)",
                            Effect::Restart =>
                                "(applied — takes effect after a server restart: run \
                                 `fleety-server restart` on the server host)",
                        }
                    );
                }
            } else if let Some(e) = error {
                eprintln!("error: {}", e.message);
                if let Some(hint) = e.remediation {
                    eprintln!("hint: {hint}");
                }
            }
            Ok(())
        }
        Some(ServerMsg::Error { error }) => {
            eprintln!("error: {}", error.message);
            Ok(())
        }
        other => Err(CoreError::Provider(format!(
            "expected a config result, got {other:?}"
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

/// Clamp a conversation preview to `max` characters for single-line display,
/// appending an ellipsis when it was cut. Truncation is on char boundaries, not
/// byte indices, so multibyte (e.g. CJK) previews never panic or split a
/// codepoint. The server already sends a short one-line preview; this is a
/// display-side safety net so a non-conforming/large preview can't blow up the
/// row.
fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

async fn conversations(limit: Option<u32>) -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(&mut tx, &ClientMsg::ConversationList { limit }).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::ConversationListResult { conversations_json }) => {
            let items: Vec<serde_json::Value> =
                serde_json::from_str(&conversations_json).unwrap_or_default();
            if items.is_empty() {
                println!("(no conversations)");
            } else {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                for item in &items {
                    let id = item
                        .get("conversation_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let ts = item
                        .get("last_ts_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let preview = item.get("preview").and_then(|v| v.as_str()).unwrap_or("");
                    let when = format_relative(now, ts);
                    let preview = truncate_preview(preview, 80);
                    if preview.is_empty() {
                        println!("{id}  {when:>8}");
                    } else {
                        println!("{id}  {when:>8}  {preview}");
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
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                for b in &backups {
                    let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let path = b
                        .get("original_rel_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ts = b.get("ts_secs").and_then(|v| v.as_u64()).unwrap_or(0);
                    // Same relative rendering as `audit list` — the two history
                    // commands should read the same way.
                    println!("{id}  {:>8}  {path}", format_relative(now, ts));
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

/// Connect to the current server, complete the Hello handshake, and return the
/// split streams plus the server's config-protocol version (from `Welcome`).
/// Used by the interactive config panel to decide the Server region path.
pub(crate) async fn open_panel() -> Result<((Tx, Rx), u32)> {
    let target = resolve_target()?;
    let (mut tx, mut rx) = transport::connect(&target.url, target.token.as_deref())
        .await?
        .split();
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    let config_protocol = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            config_protocol, ..
        }) => config_protocol,
        other => {
            return Err(CoreError::Provider(format!(
                "expected welcome, got {other:?}"
            )))
        }
    };
    Ok(((tx, rx), config_protocol))
}

pub(crate) async fn send(tx: &mut Tx, msg: &ClientMsg) -> Result<()> {
    let json = serde_json::to_string(msg)
        .map_err(|e| CoreError::Message(format!("serialize client frame: {e}")))?;
    tx.send_text(json).await
}

pub(crate) async fn recv(rx: &mut Rx) -> Result<Option<ServerMsg>> {
    match rx.recv_text().await {
        Some(text) => {
            let msg = serde_json::from_str(&text)
                .map_err(|e| CoreError::Provider(format!("malformed server frame: {e}")))?;
            Ok(Some(msg))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::{format_relative, format_uptime, truncate_preview};

    #[test]
    fn preview_truncation_is_char_safe_and_ellipsizes() {
        // Short strings pass through unchanged (no ellipsis).
        assert_eq!(truncate_preview("hello", 80), "hello");
        assert_eq!(truncate_preview("", 80), "");
        // Exactly at the bound → unchanged.
        assert_eq!(truncate_preview("abcde", 5), "abcde");
        // Over the bound → cut to `max` chars plus an ellipsis.
        assert_eq!(truncate_preview("abcdef", 5), "abcde…");
        // Multibyte (CJK) is cut on char boundaries, never mid-codepoint, so it
        // never panics and the kept part is exactly `max` characters.
        let cut = truncate_preview("這是一段很長的中文預覽內容", 5);
        assert_eq!(cut, "這是一段很…");
        assert_eq!(cut.chars().count(), 6); // 5 kept + ellipsis
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
        temp_home: PathBuf,
    }

    impl EnvGuard {
        fn new(name: &str) -> Self {
            let keys = [
                "HOME",
                "USERPROFILE",
                "FLEETY_AGENT_URL",
                "FLEETY_DEVICE_ID",
                "FLEETY_TOKEN",
                "FLEETY_MDNS_DISABLED",
                "FLEETY_CONNECTIONS",
                "COMPUTERNAME",
                "HOSTNAME",
            ];
            let saved = keys
                .into_iter()
                .map(|key| (key, std::env::var(key).ok()))
                .collect::<Vec<_>>();
            let temp_home =
                std::env::temp_dir().join(format!("fleety-cli-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&temp_home);
            std::fs::create_dir_all(&temp_home).expect("temp home");

            std::env::set_var("HOME", &temp_home);
            std::env::set_var("USERPROFILE", &temp_home);
            for key in [
                "FLEETY_AGENT_URL",
                "FLEETY_DEVICE_ID",
                "FLEETY_TOKEN",
                "FLEETY_MDNS_DISABLED",
                "FLEETY_CONNECTIONS",
                "COMPUTERNAME",
                "HOSTNAME",
            ] {
                std::env::remove_var(key);
            }

            Self { saved, temp_home }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            let _ = std::fs::remove_dir_all(&self.temp_home);
        }
    }

    #[test]
    fn agent_url_prefers_env_then_current_profile_then_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("agent-url");
        // No LAN probe: keep the "nothing configured" case fast and deterministic.
        std::env::set_var("FLEETY_MDNS_DISABLED", "1");

        // Nothing configured → the localhost default.
        assert_eq!(agent_url(), connection::DEFAULT_URL);

        // A current profile in connections.toml → its url.
        let mut conns = connection::Connections::default();
        conns.profiles.insert(
            "home".to_string(),
            connection::Profile {
                url: "ws://cfg".to_string(),
                ..Default::default()
            },
        );
        conns.current = Some("home".to_string());
        connection::save(&conns).expect("save connections");
        assert_eq!(agent_url(), "ws://cfg");

        // The env override wins over the current profile.
        std::env::set_var("FLEETY_AGENT_URL", "ws://env");
        assert_eq!(agent_url(), "ws://env");
    }

    #[test]
    fn device_id_is_stable_and_nonempty() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("device-id");

        let first = device_id();
        assert!(!first.is_empty());
        assert_eq!(device_id(), first);
    }

    #[test]
    fn init_upsert_then_pair_token_land_on_current_profile() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("upsert-pair");
        std::env::set_var("FLEETY_MDNS_DISABLED", "1");

        // `init` sugar: add-or-update the profile + make it current; a fresh
        // profile has no token yet.
        let token = upsert_profile_and_use("default", "ws://srv:8787").expect("upsert");
        assert!(token.is_none());
        let conns = connection::load().expect("load");
        assert_eq!(conns.current.as_deref(), Some("default"));
        assert_eq!(
            conns.current_profile().map(|p| p.url.as_str()),
            Some("ws://srv:8787")
        );

        // `pair` writes the minted token onto the current profile.
        set_current_token("minted-token").expect("set token");
        let conns = connection::load().expect("reload");
        assert_eq!(
            conns.current_profile().and_then(|p| p.token.as_deref()),
            Some("minted-token")
        );
        // A re-init of the same server keeps the token (returns it).
        let token = upsert_profile_and_use("default", "ws://srv:8787").expect("re-init");
        assert_eq!(token.as_deref(), Some("minted-token"));

        // With no current server, pairing has nowhere to put the token.
        let empty = connection::Connections {
            current: None,
            ..Default::default()
        };
        connection::save(&empty).expect("clear");
        assert!(set_current_token("x").is_err());
    }

    #[test]
    fn hello_carries_the_resolved_token_pairing_and_device() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("hello");
        let expected_device_id = device_id();

        // The token is passed in (resolved by the caller), not read from env.
        match hello(Some("tok-1".to_string()), Some("pair-1".to_string())) {
            ClientMsg::Hello {
                device_id,
                protocol,
                token,
                pairing_code,
                ..
            } => {
                assert_eq!(device_id, expected_device_id);
                assert_eq!(protocol, PROTOCOL_VERSION);
                assert_eq!(token.as_deref(), Some("tok-1"));
                assert_eq!(pairing_code.as_deref(), Some("pair-1"));
            }
            other => panic!("unexpected hello: {other:?}"),
        }
    }

    #[test]
    fn origin_reports_os_and_best_effort_context() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("origin");

        let value = origin();
        assert_eq!(value.os.as_deref(), Some(std::env::consts::OS));
        assert!(value.cwd.is_some());
    }

    #[test]
    fn unexpected_pair_reply_is_readable_not_a_debug_dump() {
        // A struct-carrying frame that would actually hit the pair `other` arm.
        // Its Debug form would contain `{ ... }` with the internal fields; the
        // readable message must not.
        let frame = ServerMsg::AssistantDelta {
            conversation_id: "c1".to_string(),
            chunk: "secret-token-ish".to_string(),
        };
        let msg = unexpected_pair_reply(Some(&frame));
        assert!(!msg.contains('{'), "no Debug struct dump: {msg}");
        assert!(!msg.contains(":?"), "no Debug format artifact: {msg}");
        assert!(
            !msg.contains("secret-token-ish"),
            "internal field values not leaked: {msg}"
        );
        assert!(msg.contains("unexpected"), "message is descriptive: {msg}");
        assert!(msg.contains("retry"), "message states the next step: {msg}");
        // It names the frame kind from the wire tag (readable), not the type.
        assert!(
            msg.contains("assistant_delta"),
            "names the frame kind: {msg}"
        );

        // A closed connection (no reply) is its own readable message.
        let closed = unexpected_pair_reply(None);
        assert!(!closed.contains('{'));
        assert!(closed.contains("closed"));
        assert!(closed.contains("retry"));
    }

    #[test]
    fn server_msg_kind_reads_the_wire_tag() {
        let done = ServerMsg::Done {
            conversation_id: "c1".to_string(),
        };
        assert_eq!(server_msg_kind(&done), "done");
    }
}
