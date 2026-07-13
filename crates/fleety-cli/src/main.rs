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
mod model_picker;
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
    println!(
        "  status                           this host (cli version, daemon) + the connected server"
    );
    println!("  version                          print the CLI version (also --version / -v)");
    println!("  config <list|get|set|unset|edit> [--target server|daemon|cli|<device-id>]");
    println!("  config provider|model <...>      manage the connected server's providers + roles");
    println!("  auth <login|status|logout>       ChatGPT/Codex OAuth sign-in");
    println!("  audit list [<limit>]             this device's audit-log entries");
    println!("  audit show <index>               one audit entry in full");
    println!("  rollback list                    backups available to restore");
    println!("  rollback apply <backup_id>       restore a file from a backup");
    println!("  pair <code>                      enroll this device (auth-required servers)");
    println!("  pair-code                        mint a pairing code on the current server");
    println!("  daemon <verb>                    manage the local daemon (install/start/...)");
    println!("  update                           update every fleety component on this host");
    println!("  acp [install [zed]]              run as an ACP agent (editors launch this)");
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Parse side-effect-free top-level queries before logging, config seeding,
    // or legacy migration. `--help` and `--version` must never touch user data.
    let (args, target) = take_server_override(std::env::args().collect());
    match args.get(1).map(String::as_str) {
        Some("help" | "--help" | "-h") if args.len() == 2 => {
            print_help();
            return std::process::ExitCode::SUCCESS;
        }
        Some("version" | "--version" | "-v" | "-V") if args.len() == 2 => {
            println!("fleety {}", agent_core::VERSION);
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }
    obs::init();
    // Seed env from ~/.fleety/config.toml so client settings (e.g. transport mode)
    // set via `fleety config` apply; an explicit env var still wins.
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    // One-time, idempotent migration of the legacy config.json into
    // connections.toml. A real migration failure is actionable and must not be
    // hidden behind a partially initialized command.
    if let Err(e) = connection::migrate_from_config_json() {
        return fail(e);
    }
    let _ = OVERRIDE.set(target);
    match args.get(1).map(String::as_str) {
        Some("init") => {
            // `fleety init <ws-url> [--name <name>]` — positional url plus an
            // optional profile name (default `default`). With NO url on a TTY,
            // init turns into the guided first run: scan the LAN, pick a server
            // from the list, save it, and offer to pair right away.
            let mut url = String::new();
            let mut name: Option<String> = None;
            let mut pairing_code: Option<String> = None;
            let mut it = args.iter().skip(2);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--name" => {
                        if name.is_some() {
                            return usage("usage: fleety init <ws-url> [--name <name>] [--pairing-code <code>]");
                        }
                        let Some(n) = it.next() else {
                            return usage("usage: fleety init <ws-url> [--name <name>] [--pairing-code <code>]");
                        };
                        name = Some(n.clone());
                    }
                    "--pairing-code" => {
                        if pairing_code.is_some() {
                            return usage("usage: fleety init <ws-url> [--name <name>] [--pairing-code <code>]");
                        }
                        let Some(code) = it.next() else {
                            return usage("usage: fleety init <ws-url> [--name <name>] [--pairing-code <code>]");
                        };
                        pairing_code = Some(code.clone());
                    }
                    _ if url.is_empty() => url = a.clone(),
                    _ => {
                        return usage(
                            "usage: fleety init <ws-url> [--name <name>] [--pairing-code <code>]",
                        )
                    }
                }
            }
            if url.is_empty() {
                if std::io::IsTerminal::is_terminal(&std::io::stdout())
                    && std::env::var("FLEETY_MDNS_DISABLED").is_err()
                {
                    return done(init_interactive(name).await);
                }
                return usage(
                    "usage: fleety init <ws-url> [--name <name>]   (e.g. ws://host:8787)",
                );
            }
            let name = name.unwrap_or_else(|| "default".to_string());
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
            done(init(url, name, pairing_code).await)
        }
        Some("ask") => {
            let (text, attachment_paths) = match parse_ask_args(&args[2..]) {
                Ok(parsed) => parsed,
                Err(e) => return fail(e),
            };
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
            if !(args.len() == 3 || args.len() == 4) {
                return usage("usage: fleety resume <conversation_id> [after_seq]");
            }
            let conversation_id = args[2].clone();
            let after_seq = match args.get(3) {
                Some(value) => match value.parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        return usage(&format!(
                            "invalid after_seq '{value}'; expected an unsigned integer\nusage: fleety resume <conversation_id> [after_seq]"
                        ))
                    }
                },
                None => 0,
            };
            done(resume(conversation_id, after_seq).await)
        }
        Some("tui") if args.len() == 2 => done(run_tui().await),
        Some("tui") => usage("usage: fleety tui"),
        Some("conversations") => {
            if args.len() > 3 {
                return usage("usage: fleety conversations [<limit>]");
            }
            let limit = match args.get(2) {
                Some(value) => match value.parse::<u32>() {
                    Ok(value) => Some(value),
                    Err(_) => {
                        return usage(&format!(
                            "invalid limit '{value}'; expected an unsigned integer\nusage: fleety conversations [<limit>]"
                        ))
                    }
                },
                None => None,
            };
            done(conversations(limit).await)
        }
        Some("audit") => {
            let sub = args.get(2).cloned().unwrap_or_default();
            match sub.as_str() {
                "list" => {
                    if args.len() > 4 {
                        return usage("usage: fleety audit list [<limit>]");
                    }
                    let limit = match args.get(3) {
                        Some(value) => match value.parse::<u32>() {
                            Ok(value) => Some(value),
                            Err(_) => {
                                return usage(&format!(
                                    "invalid audit limit '{value}'; expected an unsigned integer\nusage: fleety audit list [<limit>]"
                                ))
                            }
                        },
                        None => None,
                    };
                    done(audit_list(limit).await)
                }
                "show" if args.len() == 4 => {
                    match args.get(3).and_then(|s| s.parse::<u64>().ok()) {
                        Some(i) => done(audit_show(i).await),
                        None => usage(&format!(
                            "invalid audit index '{}'; expected an unsigned integer\nusage: fleety audit show <index>",
                            args[3]
                        )),
                    }
                }
                "show" => usage("usage: fleety audit show <index>"),
                _ => usage("usage: fleety audit list [<limit>]  |  fleety audit show <index>"),
            }
        }
        Some("rollback") => {
            let sub = args.get(2).cloned().unwrap_or_default();
            match sub.as_str() {
                "list" if args.len() == 3 => done(rollback_list().await),
                "apply" if args.len() == 4 => done(rollback_apply(args[3].clone()).await),
                "apply" => usage("usage: fleety rollback apply <backup_id>"),
                _ => usage("usage: fleety rollback list  |  fleety rollback apply <backup_id>"),
            }
        }
        Some("server") => done(server::run(&args[2..])),
        Some("status") if args.len() == 2 => done(status().await),
        Some("status") => usage("usage: fleety status"),
        Some("voice") if args.len() == 2 => done(voice_chat().await),
        Some("voice") => usage("usage: fleety voice"),
        Some("config") => {
            let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
            let res = match config::split_target(&args[2..]) {
                Err(e) => Err(e),
                Ok((_requested, rest)) if rest.is_empty() && is_tty => config_panel::run().await,
                Ok((requested, rest)) => {
                    async {
                        let target = config::resolve_target(requested, &rest, &device_id())?;
                        if matches!(target, config::Target::Auto)
                            && matches!(rest.first().map(String::as_str), None | Some("list"))
                        {
                            config_list_all().await
                        } else if config::is_remote_provider_edit(&rest, &target) && is_tty {
                            config::provider_edit_remote().await
                        } else if matches!(target, config::Target::Cli) {
                            config::run(&rest)
                        } else {
                            config_remote(config::wire_target(&target)?, &rest).await
                        }
                    }
                    .await
                }
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
            if sub.is_empty() || sub.len() > 1 {
                usage(
                    "usage: fleety daemon <install|uninstall|start|stop|restart|enable|disable|status|up|down|update>",
                )
            } else {
                let verb = match sub[0].as_str() {
                    "up" => "start",
                    "down" => "stop",
                    known @ ("install" | "uninstall" | "start" | "stop" | "restart"
                    | "enable" | "disable" | "status" | "update") => known,
                    _ => {
                        return usage(
                            "usage: fleety daemon <install|uninstall|start|stop|restart|enable|disable|status|up|down|update>",
                        )
                    }
                };
                done(daemon_delegate(&[verb.to_string()]))
            }
        }
        Some("update") if args.len() == 2 => {
            // Update every fleety component installed on this host (CLI + any
            // local server + daemon). One command, per the unified update model.
            done(update_all().await)
        }
        Some("update") => usage("usage: fleety update"),
        Some("acp") => {
            // `fleety acp install [--server <url>]` writes the Zed agent-server
            // config; plain `fleety acp` runs the adapter over stdio (stdout is
            // only JSON-RPC, logs go to stderr, so the editor's parser is safe).
            if args.get(2).is_none() {
                done(async { acp::run_resolved(resolve_target()?).await }.await)
            } else if args.get(2).map(String::as_str) == Some("install") {
                let server = args
                    .iter()
                    .position(|a| a == "--server")
                    .and_then(|i| args.get(i + 1))
                    .cloned();
                // `fleety acp install [<editor>]` — <editor> (e.g. `zed`) auto-
                // configures that editor; with none, print the generic setup that
                // works with any ACP-capable editor.
                let target = args.get(3).filter(|a| !a.starts_with("--")).cloned();
                let known = args[3..].iter().all(|arg| {
                    arg == "zed" || arg == "--server" || server.as_deref() == Some(arg.as_str())
                });
                let invalid = !known
                    || args.iter().filter(|arg| *arg == "--server").count() > 1
                    || (args.iter().any(|arg| arg == "--server") && server.is_none());
                if invalid {
                    usage("usage: fleety acp install [zed] [--server <url>]")
                } else {
                    done(acp::install(target, server))
                }
            } else if matches!(
                args.get(2).map(String::as_str),
                Some("help" | "--help" | "-h")
            ) && args.len() == 3
            {
                println!("usage: fleety acp [install [zed] [--server <url>]]");
                std::process::ExitCode::SUCCESS
            } else {
                usage("usage: fleety acp [install [zed] [--server <url>]]")
            }
        }
        Some("pair") if args.len() == 3 => done(pair(args[2].clone()).await),
        Some("pair") => usage(
            "usage: fleety pair <pairing-code>   (mint one with `fleety pair-code` on the server)",
        ),
        Some("pair-code") if args.len() == 2 => done(pair_code().await),
        Some("pair-code") => usage("usage: fleety pair-code"),
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

    // fleety-server: the shared host-wide sibling path ({bin}-template gated;
    // an updated server restarts idle-deferred).
    fleety_tools::update::update_siblings_to_latest(&["fleety-server"]).await?;

    // fleetyd: delegate to its own complete update (binary + insyra + restart +
    // its own sibling pass, which is a no-op after the update above).
    if let Some(exe) = sibling_bin("fleetyd") {
        let status = std::process::Command::new(&exe)
            .arg("update")
            .status()
            .map_err(|e| {
                CoreError::Message(format!(
                    "could not run fleetyd update ({}): {e}",
                    exe.display()
                ))
            })?;
        if !status.success() {
            return Err(CoreError::Message(format!(
                "fleetyd update failed with {status}; fleety update is incomplete"
            )));
        }
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
/// Whether a server `Error.kind` is an authentication rejection — terminal for
/// the TUI (no amount of reconnecting fixes an unpaired device). Pure.
fn is_auth_rejection(kind: &str) -> bool {
    kind == "unauthenticated"
}

async fn run_tui() -> Result<()> {
    use ratatui::crossterm::event::{Event, KeyEventKind};

    let (mut tx, mut rx, target) = open().await?;
    let url = target.url.clone();
    let token = target.token.clone();
    send(&mut tx, &hello(token.clone(), None)).await?;

    // Converge to a newer server before entering the UI. The Welcome frame is
    // the greeting; the TUI does not otherwise consume it.
    if let Ok(Some(ServerMsg::Welcome { server_version, .. })) = recv(&mut rx).await {
        maybe_converge_cli(&server_version).await;
    }

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
                        app.turn_in_flight = false;
                        if is_auth_rejection(&error.kind) {
                            // Not a transient drop — the server won't take us
                            // without pairing. Say so and stop, instead of
                            // reconnecting forever.
                            app.status = "Not paired with this server — run `fleety pair <code>` \
                                          (mint one with `fleety pair-code` on the server host), \
                                          then reopen the TUI."
                                .to_string();
                            app.should_quit = true;
                        } else {
                            // The turn ended with an error: clear in-flight so
                            // Esc goes back to quitting.
                            app.status = format!("agent error: {}", error.message);
                        }
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
    // Discovery step (only reached with no override/env/sticky profile): prefer
    // a local server on loopback over an mDNS advertisement. On the server host,
    // mDNS returns this box's own LAN IP, and a same-host connection to that LAN
    // IP is NOT loopback-trusted (it would demand pairing); 127.0.0.1 is. Plain
    // mDNS advertises no server fingerprint yet, so a discovered server never
    // inherits a pinned profile's token (the guard stays closed).
    let r = connection::resolve(&conns, &over, env_url, env_token, || {
        prefer_loopback_discovery(
            || loopback_server_up(std::time::Duration::from_millis(300)),
            || discover_via_mdns(std::time::Duration::from_secs(2)),
        )
    })?;
    match &r.source {
        connection::Source::Env => {
            eprintln!(
                "note: FLEETY_AGENT_URL overrides the current server ({})",
                r.url
            )
        }
        connection::Source::Mdns if r.url == local_server_url() => eprintln!(
            "using this host's local server ({}) — same-host trusted, no pairing needed",
            r.url
        ),
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

/// Resolve + connect in one step: returns the split streams plus the resolved
/// target (so callers can read its url/token). Every non-`init` connect site
/// goes through here so they share one resolution (one mDNS probe, one token).
/// When the sticky profile URL fails and the profile carries a pinned server
/// fingerprint, one heal scan runs: an advertiser with the SAME fingerprint at
/// a new address is adopted (persisted) and the connect retried once — any
/// other advertiser is ignored and the original failure surfaces.
async fn open() -> Result<(Tx, Rx, connection::Resolved)> {
    let mut target = resolve_target()?;
    match transport::connect(&target.url, target.token.as_deref()).await {
        Ok(ws) => {
            let (tx, rx) = ws.split();
            Ok((tx, rx, target))
        }
        Err(e) => {
            if !matches!(target.source, connection::Source::Profile(_)) {
                return Err(e);
            }
            let Some(new_url) = connection::heal_current_profile(&target.url) else {
                return Err(e);
            };
            println!(
                "server '{}' moved to {new_url} (same identity fingerprint); reconnecting…",
                match &target.source {
                    connection::Source::Profile(name) => name.as_str(),
                    _ => "current",
                }
            );
            target.url = new_url;
            let (tx, rx) = transport::connect(&target.url, target.token.as_deref())
                .await?
                .split();
            Ok((tx, rx, target))
        }
    }
}

/// The collecting scan + entry type live in `fleety_tools::connection` (shared
/// with the daemon's sticky healing); the picker below is CLI-only.
use fleety_tools::connection::{discover_all_via_mdns, DiscoveredServer};

/// The picker's parse of one input line: a 1-based pick mapped to its index,
/// a cancel (empty input / EOF), or garbage worth a re-prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    Pick(usize),
    Cancel,
    Invalid,
}

/// Parse the picker input against a list of `n` entries. Pure.
fn parse_selection(input: &str, n: usize) -> Selection {
    let t = input.trim();
    if t.is_empty() {
        return Selection::Cancel;
    }
    match t.parse::<usize>() {
        Ok(i) if (1..=n).contains(&i) => Selection::Pick(i - 1),
        _ => Selection::Invalid,
    }
}

/// One numbered picker line; the local server is flagged "no pairing" and any
/// server already in a saved profile is flagged "saved". Pure.
fn render_pick_line(
    idx: usize,
    s: &DiscoveredServer,
    saved_urls: &[String],
    local_url: Option<&str>,
) -> String {
    let mut tag = String::new();
    if local_url == Some(s.url.as_str()) {
        tag.push_str("  (local, no pairing)");
    }
    if saved_urls.iter().any(|u| u == &s.url) {
        tag.push_str("  (saved)");
    }
    format!("  {}. {}  {}{}", idx + 1, s.name, s.url, tag)
}

/// The local server's WebSocket URL: `ws://127.0.0.1:<port>`, port taken from
/// `FLEETY_ADDR` (`host:port`) or the default. Pure.
pub(crate) fn local_server_url() -> String {
    let port = std::env::var("FLEETY_ADDR")
        .ok()
        .and_then(|a| a.rsplit(':').next().map(String::from))
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(8787);
    format!("ws://127.0.0.1:{port}")
}

/// Whether a local server is listening on loopback right now: a quick blocking
/// TCP connect to `127.0.0.1:<port>` (no WS handshake — the real connect
/// follows). Sync so it slots into the resolution discovery closure. A host with
/// no local server returns fast (connection refused is immediate, not the full
/// timeout). Returns the loopback URL when up, else `None`.
fn loopback_server_up(timeout: std::time::Duration) -> Option<String> {
    use std::net::ToSocketAddrs;
    let url = local_server_url();
    let addr = url.strip_prefix("ws://")?.to_socket_addrs().ok()?.next()?;
    std::net::TcpStream::connect_timeout(&addr, timeout).ok()?;
    Some(url)
}

/// Co-located discovery preference: a local server on loopback wins over an mDNS
/// advertisement. The same host connecting to its own LAN IP — what mDNS returns
/// — is NOT loopback-trusted and would have to pair, whereas `127.0.0.1` is
/// same-host trusted (no pairing). Pure over the two injected probes so the
/// precedence is unit-testable; the loopback probe runs first and mDNS only when
/// it finds nothing.
fn prefer_loopback_discovery(
    loopback: impl FnOnce() -> Option<String>,
    mdns: impl FnOnce() -> Option<String>,
) -> Option<connection::Discovered> {
    if let Some(url) = loopback() {
        // Loopback is same-host trusted, so it never carries/needs a token.
        return Some(connection::Discovered {
            url,
            fingerprint: None,
        });
    }
    mdns().map(|url| connection::Discovered {
        url,
        fingerprint: None,
    })
}

/// Probe the local server: connect on loopback with a short timeout and read a
/// `Welcome`. Returns it as a discovery entry named `local` (carrying any
/// advertised fingerprint) when it answers, else `None` — a host with no local
/// server is not delayed beyond the timeout, and the probe never errors init.
pub(crate) async fn probe_local_server(
    url: &str,
    timeout: std::time::Duration,
) -> Option<DiscoveredServer> {
    let ws = tokio::time::timeout(timeout, transport::connect(url, None))
        .await
        .ok()?
        .ok()?;
    let (mut tx, mut rx) = ws.split();
    send(&mut tx, &hello(None, None)).await.ok()?;
    let reply = tokio::time::timeout(timeout, recv(&mut rx))
        .await
        .ok()?
        .ok()??;
    let _ = tx.close().await;
    match reply {
        ServerMsg::Welcome {
            server_fingerprint, ..
        } => Some(DiscoveredServer {
            name: "local".to_string(),
            url: url.to_string(),
            fingerprint: server_fingerprint,
        }),
        _ => None,
    }
}

/// Read one line from stdin (the picker / pairing prompts); EOF reads as empty.
fn read_prompt_line() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line
}

/// Guided first-run `fleety init` (no URL, on a TTY): scan the LAN, pick a
/// server from a numbered list, save it as the current profile, and offer to
/// pair right away. Falls back to the usage guidance when nothing is found.
async fn init_interactive(name_override: Option<String>) -> Result<()> {
    // Probe the local server first (loopback, short timeout): a same-host server
    // needs no pairing and should be the default pick.
    let local_url = local_server_url();
    let local = probe_local_server(&local_url, std::time::Duration::from_secs(1)).await;
    if local.is_some() {
        println!("Found a local server at {local_url}.");
    }
    println!("Scanning the LAN for Fleety servers… (3s)");
    let mut found = discover_all_via_mdns(std::time::Duration::from_secs(3));
    // The local server leads the list (default pick); drop any mDNS duplicate of it.
    if let Some(local) = local {
        found.retain(|d| d.url != local.url);
        found.insert(0, local);
    }
    if found.is_empty() {
        return Err(CoreError::Message(
            "No Fleety server found locally or on the LAN. Start fleety-server, or run `fleety init ws://host:8787 --name <name> --pairing-code <code>`. Mint a code with `fleety pair-code` on an already paired device"
                .to_string(),
        ));
    }
    let saved_urls: Vec<String> = connection::load()
        .map(|c| c.profiles.values().map(|p| p.url.clone()).collect())
        .unwrap_or_default();
    println!("Found {} server(s):", found.len());
    for (i, s) in found.iter().enumerate() {
        println!("{}", render_pick_line(i, s, &saved_urls, Some(&local_url)));
    }
    let picked = loop {
        // Default (empty input) picks #1 — the local server when present.
        eprint!("Pick a server [1-{}] (Enter for 1): ", found.len());
        match parse_selection(&read_prompt_line(), found.len()) {
            Selection::Pick(i) => break i,
            Selection::Cancel => break 0,
            Selection::Invalid => {
                eprintln!("Please enter a number from 1 to {}.", found.len());
                continue;
            }
        }
    };
    let chosen = found[picked].clone();
    let is_local = chosen.url == local_url;
    let profile = name_override.unwrap_or_else(|| chosen.name.clone());
    let pairing_code = if is_local {
        None
    } else {
        eprint!(
            "Pairing code — mint one with `fleety pair-code` on an already-paired device \
             (Enter only if this profile is already paired): "
        );
        let code = read_prompt_line().trim().to_string();
        (!code.is_empty()).then_some(code)
    };
    init(chosen.url.clone(), profile.clone(), pairing_code).await?;
    println!("Using verified server '{profile}' ({}).", chosen.url);
    Ok(())
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
    let target = resolve_target()?;
    let profile_name = match &target.source {
        connection::Source::Profile(name) => name.clone(),
        _ => {
            return Err(CoreError::Message(
                "pairing needs a named server profile so the credential has an unambiguous owner. Run `fleety init <ws-url> --name <name>`, then retry; no profile was modified"
                    .to_string(),
            ))
        }
    };
    let (mut tx, mut rx) = transport::connect(&target.url, target.token.as_deref())
        .await?
        .split();
    let url = target.url.clone();
    send(&mut tx, &hello(target.token.clone(), Some(code))).await?;
    let result = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            token: Some(tok),
            server_fingerprint,
            ..
        }) => {
            connection::store_profile_pairing(
                &profile_name,
                &url,
                &tok,
                server_fingerprint.as_deref(),
            )?;
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

/// `fleety pair-code`: ask the connected server (local via loopback trust, or a
/// token-authenticated remote) to mint a short-lived pairing code, and print it
/// for enrolling another device.
async fn pair_code() -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await?;
    send(&mut tx, &ClientMsg::MintPairingCode).await?;
    let reply = recv(&mut rx).await?;
    let _ = tx.close().await;
    match reply {
        Some(ServerMsg::PairingCode {
            code: Some(code), ..
        }) => {
            println!("Pairing code: {code}");
            println!("On the other device, run:  fleety pair {code}   (expires soon)");
            Ok(())
        }
        Some(ServerMsg::PairingCode { error: Some(e), .. }) => {
            Err(CoreError::Message(match e.remediation {
                Some(r) => format!("{} — {r}", e.message),
                None => e.message,
            }))
        }
        Some(ServerMsg::Error { error }) => Err(CoreError::Message(format!(
            "the server is too old to mint pairing codes ({}) — update it (`fleety update` on the \
             server host), or use the code printed at the server's first run",
            error.message
        ))),
        other => Err(CoreError::Provider(format!(
            "expected a pairing-code reply, got {other:?}"
        ))),
    }
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

fn parse_ask_args(args: &[String]) -> Result<(String, Vec<(PathBuf, &'static str)>)> {
    let mut words = Vec::new();
    let mut attachments = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let kind = match args[index].as_str() {
            "--image" | "-i" => Some("image"),
            "--audio" => Some("audio"),
            "--video" => Some("video"),
            "--file" => Some("file"),
            flag if flag.starts_with('-') => {
                return Err(CoreError::Message(format!("unknown ask flag '{flag}'")))
            }
            _ => None,
        };
        if let Some(kind) = kind {
            let value = args
                .get(index + 1)
                .ok_or_else(|| CoreError::Message(format!("{} needs a file path", args[index])))?;
            if value.starts_with('-') {
                return Err(CoreError::Message(format!(
                    "{} needs a file path, got '{value}'",
                    args[index]
                )));
            }
            attachments.push((PathBuf::from(value), kind));
            index += 2;
        } else {
            words.push(args[index].clone());
            index += 1;
        }
    }
    Ok((words.join(" "), attachments))
}

/// `fleety init <ws-url> [--name <name>]`: sugar for `server add <name> <url>
/// --use` plus enrollment. Records/updates the named profile (default `default`),
/// makes it current, connects, and registers this device.
async fn init(url: String, name: String, pairing_code: Option<String>) -> Result<()> {
    // Build the proposed state in memory. Nothing is persisted until this exact
    // endpoint returns a valid Welcome (and, when supplied, redeems the pairing
    // code), so a typo or unreachable server cannot poison current selection.
    let mut proposed = connection::load()?;
    if proposed.device_id.is_empty() {
        proposed.device_id = fleety_tools::device::device_id();
    }
    let profile = proposed.profiles.entry(name.clone()).or_default();
    let token = profile.token.clone();
    let old_fingerprint = profile.fingerprint.clone();
    profile.url = url.clone();
    proposed.current = Some(name.clone());
    let (mut tx, mut rx) = transport::connect(&url, token.as_deref()).await?.split();

    send(&mut tx, &hello(token, pairing_code)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            session_id,
            token: minted_token,
            server_fingerprint,
            ..
        }) => {
            if let Some(seen) = server_fingerprint.as_deref() {
                if connection::tofu_pin_decision(old_fingerprint.as_deref(), seen)
                    == connection::PinDecision::IdentityChanged
                {
                    return Err(CoreError::Message(format!(
                        "server '{name}' has a different identity fingerprint; connections.toml was not changed"
                    )));
                }
            }
            let profile = proposed.profiles.get_mut(&name).ok_or_else(|| {
                CoreError::Message("proposed server profile disappeared".to_string())
            })?;
            if let Some(minted) = minted_token {
                profile.token = Some(minted);
            }
            if old_fingerprint.is_none() {
                profile.fingerprint = server_fingerprint;
            }
            connection::save(&proposed)?;
            println!("✓ connected to {url}");
            println!(
                "✓ registered device '{}' as server '{name}' (session {session_id})",
                device_id()
            );
        }
        Some(ServerMsg::Error { error }) => {
            return Err(CoreError::Message(format!(
                "server rejected init: {}. connections.toml was not changed. If pairing is required, mint a code with `fleety pair-code` and pass `--pairing-code <code>`",
                error.message
            )))
        }
        other => return Err(CoreError::Provider(format!(
            "unexpected reply during init: {other:?}; connections.toml was not changed"
        ))),
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
        Some(ServerMsg::Welcome { server_version, .. }) => {
            maybe_converge_cli(&server_version).await;
        }
        other => return Err(CoreError::Message(hello_failure_message(other.as_ref()))),
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
            None => {
                return Err(CoreError::Provider(
                    "connection closed before the request completed".to_string(),
                ))
            }
            Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
            // Credential / pairing-code replies belong to their own commands'
            // request/reply exchanges; in the ask loop they are stray noise.
            Some(ServerMsg::CredentialResult { .. })
            | Some(ServerMsg::CredentialStatusResult { .. })
            | Some(ServerMsg::ProviderModelListResult { .. })
            | Some(ServerMsg::PairingCode { .. }) => {}
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
            server_version,
            conversation_id,
            audio_input,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            (conversation_id, audio_input)
        }
        other => return Err(CoreError::Message(hello_failure_message(other.as_ref()))),
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
                Some(ServerMsg::Done { .. }) => break,
                None => {
                    return Err(CoreError::Provider(
                        "connection closed before the voice turn completed".to_string(),
                    ))
                }
                Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
        Some(ServerMsg::Welcome { server_version, .. }) => {
            maybe_converge_cli(&server_version).await;
        }
        other => return Err(CoreError::Message(hello_failure_message(other.as_ref()))),
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
            Some(ServerMsg::Done { .. }) => break,
            None => {
                return Err(CoreError::Provider(
                    "connection closed before resume completed".to_string(),
                ))
            }
            Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
            _ => {}
        }
    }
    let _ = tx.close().await;
    Ok(())
}

/// Open a connection, send Hello, await Welcome, return the streams. Common
/// preamble for audit/rollback commands.
/// Whether the CLI auto-converges to a newer server on connect
/// (`FLEETY_CLI_AUTO_UPDATE`, default on). Only an explicit `0`/`off`/`false`
/// disables it.
fn cli_auto_update_enabled() -> bool {
    match std::env::var("FLEETY_CLI_AUTO_UPDATE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false"
        ),
        Err(_) => true,
    }
}

/// Pure decision: should the CLI converge to `server_version`? Only when enabled,
/// not already converged this run, the server reported a version, and it is
/// strictly newer than `me` (forward-only — never downgrade or re-run on equal).
fn should_converge(server_version: &str, me: &str, enabled: bool, already_converged: bool) -> bool {
    enabled
        && !already_converged
        && !server_version.is_empty()
        && fleety_tools::update::is_newer(server_version, me)
}

/// Forward-only convergence for the interactive CLI: when the connected server is
/// newer, update this binary to the server's exact version and re-exec the
/// current command on it. No-op when up to date, disabled, or already converged
/// once this run; a failed self-update warns and lets the command proceed on the
/// current version (never blocks). Mirrors the daemon's `converge_to_server_version`.
async fn maybe_converge_cli(server_version: &str) {
    let me = agent_core::VERSION;
    let already = std::env::var("FLEETY_CONVERGED").is_ok();
    if !should_converge(server_version, me, cli_auto_update_enabled(), already) {
        return;
    }
    eprintln!("updating fleety {me} → {server_version} to match the server…");
    match fleety_tools::update::converge_self_to_version(server_version).await {
        Ok(true) => reexec_current(), // replaces the process on success
        Ok(false) => eprintln!(
            "note: no matching build to converge to the server's {server_version} yet — \
             continuing on {me}"
        ),
        Err(e) => eprintln!(
            "note: could not self-update to match the server ({server_version}): {} — \
             continuing on {me}",
            e.report().message
        ),
    }
}

/// Re-run the current command with the same arguments after a convergence
/// self-update. Sets `FLEETY_CONVERGED` first so the fresh process converges at
/// most once (no loop). Unix replaces the process image; other platforms spawn +
/// wait + exit with the child's code. Returns only on failure, so the caller
/// proceeds on the current binary.
fn reexec_current() {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("note: cannot locate the updated binary to re-run ({e}); continuing");
            return;
        }
    };
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::env::set_var("FLEETY_CONVERGED", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).args(&args).exec();
        eprintln!("note: could not re-run the updated binary ({err}); continuing on the old one");
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(&exe).args(&args).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(0)),
            Err(e) => {
                eprintln!(
                    "note: could not re-run the updated binary ({e}); continuing on the old one"
                );
            }
        }
    }
}

async fn connect_hello() -> Result<(Tx, Rx)> {
    let (mut tx, mut rx, target) = open().await?;
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            server_fingerprint,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), &target);
            Ok((tx, rx))
        }
        other => Err(CoreError::Message(hello_failure_message(other.as_ref()))),
    }
}

/// A human-readable reason a Hello handshake did not yield a `Welcome`. Never a
/// Debug dump of the internal frame. Pure. An `unauthenticated` rejection is the
/// common "not paired yet" case and gets actionable guidance.
fn hello_failure_message(reply: Option<&ServerMsg>) -> String {
    match reply {
        Some(ServerMsg::Error { error }) if is_auth_rejection(&error.kind) => {
            "not paired with this server — run `fleety pair <code>` (mint a code with \
             `fleety pair-code` on the server host)"
                .to_string()
        }
        Some(ServerMsg::Error { error }) => {
            format!("the server rejected the connection: {}", error.message)
        }
        Some(other) => format!("unexpected reply from server ({})", server_msg_kind(other)),
        None => "the connection closed before the server replied".to_string(),
    }
}

/// Trust-on-authenticated-connect: back-fill the current profile's server
/// fingerprint from a successful Welcome (devices enrolled before fingerprints
/// existed gain healing without re-pairing); warn — never overwrite — when the
/// identity changed. Best-effort and quiet on the happy path.
fn tofu_pin(fingerprint: Option<&str>, target: &connection::Resolved) {
    let Some(fp) = fingerprint.filter(|f| !f.is_empty()) else {
        return;
    };
    let connection::Source::Profile(ref name) = target.source else {
        return;
    };
    if let Ok(connection::PinDecision::IdentityChanged) =
        connection::pin_profile_fingerprint(name, fp)
    {
        eprintln!(
            "warning: the server's identity fingerprint changed since it was pinned; keeping \
             the old pin — re-pair (`fleety init` / `fleety pair`) if the server was \
             intentionally rebuilt"
        );
    }
}

/// `connect_hello` for the `auth` command: also returns the server's advertised
/// config protocol (the credential-support gate) and the resolved target, so
/// auth can refuse an old server up front and name the server it acts on.
pub(crate) async fn connect_hello_for_auth() -> Result<(Tx, Rx, u32, connection::Resolved)> {
    let target = resolve_target()?;
    let (tx, rx, config_protocol, _fingerprint) = connect_hello_for_auth_target(&target).await?;
    Ok((tx, rx, config_protocol, target))
}

/// Resolve once for a long-running auth transaction and retain both the target
/// and the server identity observed during preflight.
pub(crate) async fn connect_hello_for_auth_transaction(
) -> Result<(Tx, Rx, u32, connection::Resolved, Option<String>)> {
    let target = resolve_target()?;
    let (tx, rx, config_protocol, fingerprint) = connect_hello_for_auth_target(&target).await?;
    Ok((tx, rx, config_protocol, target, fingerprint))
}

/// Connect to one immutable target snapshot. This deliberately does not
/// re-resolve current profile or mDNS, so a browser wait cannot redirect a
/// credential to another server.
pub(crate) async fn connect_hello_for_auth_target(
    target: &connection::Resolved,
) -> Result<(Tx, Rx, u32, Option<String>)> {
    let ws = transport::connect(&target.url, target.token.as_deref()).await?;
    let (mut tx, mut rx) = ws.split();
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            config_protocol,
            server_fingerprint,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), target);
            Ok((tx, rx, config_protocol, server_fingerprint))
        }
        other => Err(CoreError::Message(hello_failure_message(other.as_ref()))),
    }
}

/// Manage a remote (server / device) host's config over the connection: connect,
/// send `ConfigExec`, print the rendered result and when it takes effect. A
/// A connection failure is returned; configuration never falls back to files
/// owned by another runtime.
async fn config_remote(target: ConfigTarget, args: &[String]) -> Result<()> {
    let (mut tx, mut rx) = connect_hello().await.map_err(|e| {
        CoreError::Message(format!(
            "could not reach the configuration owner: {} — no local file fallback was used; check the server/daemon connection or select the correct --target",
            e.report().message
        ))
    })?;
    let restart_hint = match &target {
        ConfigTarget::Server => "restart the server (`fleety-server restart`)",
        ConfigTarget::Device(_) => "restart the daemon (`fleetyd restart`)",
        ConfigTarget::Local => "restart the owning CLI process",
    };
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
                    let message = match eff {
                        Effect::NextConnection => {
                            "(applied — takes effect on the next connection)".to_string()
                        }
                        Effect::Restart => {
                            format!("(applied — takes effect after you {restart_hint})")
                        }
                    };
                    println!("{message}");
                }
                Ok(())
            } else {
                let message = error
                    .map(|e| match e.remediation {
                        Some(hint) => format!("{} — {hint}", e.message),
                        None => e.message,
                    })
                    .unwrap_or_else(|| "configuration request was rejected".to_string());
                Err(CoreError::Message(message))
            }
        }
        Some(ServerMsg::Error { error }) => Err(CoreError::Message(match error.remediation {
            Some(hint) => format!("{} — {hint}", error.message),
            None => error.message,
        })),
        other => Err(CoreError::Provider(format!(
            "expected a config result, got {other:?}"
        ))),
    }
}

async fn config_list_all() -> Result<()> {
    println!("CLI settings:");
    config::run(&["list".to_string()])?;
    println!("\nDaemon settings:");
    config_remote(ConfigTarget::Device(device_id()), &["list".to_string()]).await?;
    println!("\nServer settings:");
    config_remote(ConfigTarget::Server, &["list".to_string()]).await
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
            let items: Vec<serde_json::Value> = serde_json::from_str(&conversations_json)
                .map_err(|e| CoreError::Provider(format!("malformed conversation list: {e}")))?;
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
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
            let entries: Vec<serde_json::Value> = serde_json::from_str(&entries_json)
                .map_err(|e| CoreError::Provider(format!("malformed audit list: {e}")))?;
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
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
            let value: serde_json::Value = serde_json::from_str(&event_json)
                .map_err(|e| CoreError::Provider(format!("malformed audit event: {e}")))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or(event_json)
            );
        }
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
            let backups: Vec<serde_json::Value> = serde_json::from_str(&backups_json)
                .map_err(|e| CoreError::Provider(format!("malformed rollback list: {e}")))?;
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
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

/// Local (this host) daemon status: running (from its pidfile), else installed
/// (binary present) but stopped, else not installed. A server install does not
/// include a daemon, so "not installed" is the normal case there.
fn local_daemon_status() -> String {
    let pid = fleety_tools::service::read_pid(&fleety_tools::service::pidfile_path("fleetyd"));
    match pid.filter(|p| fleety_tools::service::pid_alive(*p)) {
        Some(p) => format!("running (pid {p})"),
        None => {
            let name = if cfg!(windows) {
                "fleetyd.exe"
            } else {
                "fleetyd"
            };
            let beside = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|d| d.join(name)))
                .map(|p| p.is_file())
                .unwrap_or(false);
            let on_path = std::env::var_os("PATH")
                .map(|paths| std::env::split_paths(&paths).any(|d| d.join(name).is_file()))
                .unwrap_or(false);
            if beside || on_path {
                "installed, not running".to_string()
            } else {
                "not installed".to_string()
            }
        }
    }
}

async fn status() -> Result<()> {
    // This host first: the CLI's own version and whether a daemon runs here.
    println!("fleety (this host)");
    println!("  cli version:    {}", agent_core::VERSION);
    println!("  daemon:         {}", local_daemon_status());
    let server_url = resolve_target()?.url;
    println!();

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
            let ids: Vec<String> = serde_json::from_str(&device_ids_json)
                .map_err(|e| CoreError::Provider(format!("malformed device id list: {e}")))?;
            println!("fleety-server ({server_url})");
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
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
                return Err(CoreError::Message(format!("rollback failed: {message}")));
            }
        }
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
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
            server_version,
            config_protocol,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            config_protocol
        }
        other => return Err(CoreError::Message(hello_failure_message(other.as_ref()))),
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

    // (Display-name derivation and URL de-dup live in fleety_tools::connection
    // now, tested there.)

    #[test]
    fn pick_selection_parses_bounds_cancel_and_garbage() {
        // 1-based picks within bounds; empty input cancels; anything else re-prompts.
        assert_eq!(parse_selection("1", 3), Selection::Pick(0));
        assert_eq!(parse_selection(" 3 ", 3), Selection::Pick(2));
        assert_eq!(parse_selection("", 3), Selection::Cancel);
        assert_eq!(parse_selection("  \n", 3), Selection::Cancel);
        assert_eq!(parse_selection("0", 3), Selection::Invalid);
        assert_eq!(parse_selection("4", 3), Selection::Invalid);
        assert_eq!(parse_selection("abc", 3), Selection::Invalid);
    }

    #[test]
    fn pick_lines_number_and_flag_saved_servers() {
        let s = DiscoveredServer {
            name: "mini".into(),
            url: "ws://10.0.0.2:8787".into(),
            fingerprint: Some("fp-1".into()),
        };
        let saved = vec!["ws://10.0.0.2:8787".to_string()];
        let line = render_pick_line(0, &s, &saved, None);
        assert!(line.starts_with("  1."));
        assert!(line.contains("mini") && line.contains("ws://10.0.0.2:8787"));
        assert!(line.contains("(saved)"));
        let other = DiscoveredServer {
            name: "nas".into(),
            url: "ws://10.0.0.3:8787".into(),
            fingerprint: None,
        };
        assert!(!render_pick_line(1, &other, &saved, None).contains("(saved)"));
        // The local server is flagged as needing no pairing.
        let local = DiscoveredServer {
            name: "local".into(),
            url: "ws://127.0.0.1:8787".into(),
            fingerprint: None,
        };
        let line = render_pick_line(0, &local, &[], Some("ws://127.0.0.1:8787"));
        assert!(line.contains("(local, no pairing)"));
    }

    #[test]
    fn auth_rejection_is_terminal_only_for_unauthenticated() {
        assert!(is_auth_rejection("unauthenticated"));
        assert!(!is_auth_rejection("unsupported"));
        assert!(!is_auth_rejection("invalid"));
        assert!(!is_auth_rejection("io"));
        assert!(!is_auth_rejection(""));
    }

    #[test]
    fn hello_failure_messages_are_readable_never_debug() {
        let err = |kind: &str, msg: &str| {
            Some(ServerMsg::Error {
                error: fleety_protocol::WireError {
                    kind: kind.into(),
                    message: msg.into(),
                    remediation: None,
                },
            })
        };
        // Unauthenticated → not-paired guidance mentioning pair / pair-code.
        let m = hello_failure_message(err("unauthenticated", "nope").as_ref());
        assert!(m.contains("not paired") && m.contains("fleety pair"));
        assert!(m.contains("pair-code"));
        // Other server error → surfaces the server message, not a Debug dump.
        let m = hello_failure_message(err("io", "disk full").as_ref());
        assert!(m.contains("disk full") && !m.contains("Error {"));
        // Closed connection.
        assert!(hello_failure_message(None).contains("closed"));
        // Any other frame → readable tag, not a `{variant:?}` dump.
        let other = Some(ServerMsg::Done {
            conversation_id: "c1".into(),
        });
        let m = hello_failure_message(other.as_ref());
        assert!(m.contains("unexpected reply") && !m.contains("conversation_id"));
    }

    #[test]
    fn local_server_url_takes_the_addr_port() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("FLEETY_ADDR");
        assert_eq!(local_server_url(), "ws://127.0.0.1:8787");
        std::env::set_var("FLEETY_ADDR", "0.0.0.0:9000");
        assert_eq!(local_server_url(), "ws://127.0.0.1:9000");
        std::env::set_var("FLEETY_ADDR", "garbage");
        assert_eq!(local_server_url(), "ws://127.0.0.1:8787");
        std::env::remove_var("FLEETY_ADDR");
    }

    #[test]
    fn loopback_wins_over_mdns_in_discovery() {
        let lb = "ws://127.0.0.1:8787".to_string();
        let lan = "ws://192.168.1.109:8787".to_string();
        // A local server on loopback is preferred even when mDNS also finds a
        // LAN advertiser (the same-host box's own outward IP) — loopback is
        // trusted, the LAN IP would demand pairing.
        let r = prefer_loopback_discovery(|| Some(lb.clone()), || Some(lan.clone()));
        assert_eq!(r.unwrap().url, lb);
        // No local server → fall through to the mDNS advertiser.
        let r = prefer_loopback_discovery(|| None, || Some(lan.clone()));
        assert_eq!(r.unwrap().url, lan);
        // Neither → None, so resolution proceeds to the localhost default.
        let r = prefer_loopback_discovery(|| None, || None);
        assert!(r.is_none());
    }

    #[test]
    fn should_converge_truth_table() {
        // Server strictly newer, enabled, not yet converged → converge.
        assert!(should_converge("0.2.0", "0.1.0", true, false));
        // Forward-only: equal or older server → never.
        assert!(!should_converge("0.1.0", "0.1.0", true, false));
        assert!(!should_converge("0.1.0", "0.2.0", true, false));
        // Disabled → never.
        assert!(!should_converge("0.2.0", "0.1.0", false, false));
        // Loop guard: already converged this run → never.
        assert!(!should_converge("0.2.0", "0.1.0", true, true));
        // Old server reports no version → never.
        assert!(!should_converge("", "0.1.0", true, false));
    }

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
    fn resolved_target_prefers_env_then_current_profile_then_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("agent-url");
        // No LAN probe: keep the "nothing configured" case fast and deterministic.
        std::env::set_var("FLEETY_MDNS_DISABLED", "1");

        // Nothing configured → the localhost default.
        assert_eq!(resolve_target().unwrap().url, connection::DEFAULT_URL);

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
        assert_eq!(resolve_target().unwrap().url, "ws://cfg");

        // The env override wins over the current profile.
        std::env::set_var("FLEETY_AGENT_URL", "ws://env");
        assert_eq!(resolve_target().unwrap().url, "ws://env");
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
