//! fleety — the Fleety CLI.
//!
//! M2: `fleety ask "<message>"` connects to the Agent over WebSocket, does one
//! conversation round-trip, and prints the reply. Interactive TUI comes later.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

// Route command text through one sink. Human mode writes exactly as before;
// JSON mode captures legacy command text so `done` can emit one envelope. The
// dedicated structured handlers (status/config/doctor) bypass the capture only
// when they flush their semantic envelope.
macro_rules! print {
    () => {
        crate::output_stdout(String::new(), false)
    };
    ($($arg:tt)*) => {
        crate::output_stdout(format!($($arg)*), false)
    };
}

macro_rules! println {
    () => {
        crate::output_stdout(String::new(), true)
    };
    ($($arg:tt)*) => {
        crate::output_stdout(format!($($arg)*), true)
    };
}

macro_rules! eprint {
    () => {
        crate::output_stderr(String::new(), false)
    };
    ($($arg:tt)*) => {
        crate::output_stderr(format!($($arg)*), false)
    };
}

macro_rules! eprintln {
    () => {
        crate::output_stderr(String::new(), true)
    };
    ($($arg:tt)*) => {
        crate::output_stderr(format!($($arg)*), true)
    };
}

mod acp;
mod auth;
mod clipboard;
mod commands;
mod config;
mod config_panel;
mod input;
mod markdown;
mod model_picker;
mod provider_service;
mod provider_tui;
mod server;
mod tui;
mod voice;
pub mod workspace;

use std::path::{Path, PathBuf};

use agent_core::{obs, CoreError, Result};
use fleety_protocol::{
    ClientMsg, ConfigTarget, Effect, OriginContext, ServerMsg, WireAttachment,
    CONFIG_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
// The client transport (WebSocket with SSE+POST fallback) lives in fleety-tools;
// `Tx`/`Rx` are its split halves so the existing connect sites barely change.
use fleety_tools::connection::{self, Target};
use fleety_tools::transport::{self, Receiver as Rx, Sender as Tx};

#[derive(Debug, Default, Clone, Copy)]
struct OutputOptions {
    json: bool,
    quiet: bool,
    no_color: bool,
    warnings: bool,
}

static OUTPUT_OPTIONS: std::sync::OnceLock<OutputOptions> = std::sync::OnceLock::new();
static JSON_CONTEXT: std::sync::OnceLock<std::sync::Mutex<Option<serde_json::Value>>> =
    std::sync::OnceLock::new();
static JSON_EMITTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CAPTURED_OUTPUT: std::sync::OnceLock<std::sync::Mutex<CapturedOutput>> =
    std::sync::OnceLock::new();

#[derive(Debug, Default)]
struct CapturedOutput {
    stdout: String,
    stderr: String,
}

fn output_options() -> OutputOptions {
    OUTPUT_OPTIONS.get().copied().unwrap_or_default()
}

fn json_mode() -> bool {
    output_options().json
}

fn quiet_mode() -> bool {
    output_options().quiet
}

fn output_stdout(mut text: String, newline: bool) {
    if newline {
        text.push('\n');
    }
    if json_mode() {
        if let Ok(mut captured) = CAPTURED_OUTPUT
            .get_or_init(|| std::sync::Mutex::new(CapturedOutput::default()))
            .lock()
        {
            captured.stdout.push_str(&text);
        }
    } else {
        std::print!("{text}");
    }
}

fn output_stderr(mut text: String, newline: bool) {
    if newline {
        text.push('\n');
    }
    if json_mode() {
        if let Ok(mut captured) = CAPTURED_OUTPUT
            .get_or_init(|| std::sync::Mutex::new(CapturedOutput::default()))
            .lock()
        {
            captured.stderr.push_str(&text);
        }
    } else {
        std::eprint!("{text}");
    }
}

fn take_captured_data() -> serde_json::Value {
    let mut captured = CAPTURED_OUTPUT
        .get_or_init(|| std::sync::Mutex::new(CapturedOutput::default()))
        .lock()
        .ok();
    let Some(captured) = captured.as_deref_mut() else {
        return serde_json::Value::Null;
    };
    let stdout = std::mem::take(&mut captured.stdout);
    let stderr = std::mem::take(&mut captured.stderr);
    let mut data = serde_json::json!({
        "output": stdout.trim_end_matches(['\r', '\n']),
    });
    if !stderr.is_empty() {
        data["diagnostics"] =
            serde_json::Value::String(stderr.trim_end_matches(['\r', '\n']).to_string());
    }
    data
}

fn set_json_context(context: serde_json::Value) {
    if let Ok(mut slot) = JSON_CONTEXT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *slot = Some(context);
    }
}

fn json_context() -> serde_json::Value {
    JSON_CONTEXT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn emit_json(data: serde_json::Value, errors: serde_json::Value) {
    let data = merge_captured_output(data, take_captured_data());
    let ok = errors.as_array().is_some_and(Vec::is_empty);
    let value = serde_json::json!({
        "schema_version": 1,
        "ok": ok,
        "context": json_context(),
        "data": data,
        "errors": errors,
    });
    match serde_json::to_string(&value) {
        Ok(rendered) => std::println!("{rendered}"),
        Err(error) => std::println!(
            "{{\"schema_version\":1,\"ok\":false,\"context\":{{}},\"data\":null,\"errors\":[{{\"owner\":\"cli\",\"kind\":\"serialization\",\"message\":\"{}\"}}]}}",
            terminal_safe_field(&error.to_string())
        ),
    }
    JSON_EMITTED.store(true, std::sync::atomic::Ordering::Release);
}

fn merge_captured_output(
    mut data: serde_json::Value,
    captured: serde_json::Value,
) -> serde_json::Value {
    let output = captured
        .get("output")
        .and_then(serde_json::Value::as_str)
        .filter(|output| !output.is_empty());
    let diagnostics = captured
        .get("diagnostics")
        .and_then(serde_json::Value::as_str)
        .filter(|diagnostics| !diagnostics.is_empty());
    if output.is_none() && diagnostics.is_none() {
        return data;
    }
    if !data.is_object() {
        data = serde_json::json!({ "value": data });
    }
    if let Some(output) = output {
        let field = if data.get("output").is_some() {
            "additional_output"
        } else {
            "output"
        };
        data[field] = serde_json::Value::String(output.to_string());
    }
    if let Some(diagnostics) = diagnostics {
        data["diagnostics"] = serde_json::Value::String(diagnostics.to_string());
    }
    data
}

fn json_error(owner: &str, kind: &str, message: &str, remediation: Option<&str>) {
    let mut error = serde_json::json!({
        "owner": owner,
        "kind": kind,
        "message": message,
    });
    if let Some(remediation) = remediation {
        error["remediation"] = serde_json::Value::String(remediation.to_string());
    }
    emit_json(take_captured_data(), serde_json::json!([error]));
}

/// Print an error report (message + hint when present); yields the failure code
/// so every command reports failure the same way — and scripts can rely on it.
fn fail(e: CoreError) -> std::process::ExitCode {
    if json_mode() && JSON_EMITTED.load(std::sync::atomic::Ordering::Acquire) {
        return std::process::ExitCode::FAILURE;
    }
    let report = e.report();
    let message = redact_urls_in_text(&report.message);
    let remediation = report.remediation.as_deref().map(redact_urls_in_text);
    if json_mode() {
        let owner = json_context()
            .get("owner")
            .and_then(|owner| owner.as_str())
            .unwrap_or("cli")
            .to_string();
        json_error(&owner, "runtime", &message, remediation.as_deref());
        return std::process::ExitCode::FAILURE;
    }
    eprintln!("error: {}", terminal_safe_field(&message));
    if let Some(hint) = remediation {
        eprintln!("hint: {}", terminal_safe_field(&hint));
    }
    std::process::ExitCode::FAILURE
}

/// Map a command result to the process exit code (0 ok, 1 failure).
fn done(res: Result<()>) -> std::process::ExitCode {
    match res {
        Ok(()) => {
            if json_mode() && !JSON_EMITTED.load(std::sync::atomic::Ordering::Acquire) {
                emit_json(take_captured_data(), serde_json::json!([]));
            }
            std::process::ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// Usage error: print to stderr, exit 2 (distinct from runtime failures).
fn usage(msg: &str) -> std::process::ExitCode {
    if json_mode() {
        json_error("cli", "usage", msg, None);
        return std::process::ExitCode::from(2);
    }
    eprintln!("{}", terminal_safe_multiline(msg));
    std::process::ExitCode::from(2)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Parse side-effect-free top-level queries before logging, config seeding,
    // or legacy migration. `--help` and `--version` must never touch user data.
    let (args, output) = take_output_options(std::env::args().collect());
    let _ = OUTPUT_OPTIONS.set(output);
    let _show_warnings = output.warnings;
    if output.no_color {
        std::env::set_var("NO_COLOR", "1");
    }
    let args = commands::normalize_trailing_help(args);
    if let Err(error) = commands::validate(&args) {
        if json_mode() && error.exit_code() != 0 {
            return usage(error.to_string().trim_end());
        }
        return commands::render_error(error);
    }
    // Clap accepts both `--option value` and `--option=value`. Existing
    // execution handlers consume a normalized token stream, so expand the
    // validated equals form once instead of teaching every handler a second
    // parser. Tokens after `--` remain positional data.
    let args = expand_long_option_equals(args);
    let (args, target) = match take_server_override(args) {
        Ok(parsed) => parsed,
        Err(message) => return usage(&message),
    };
    if let Err(message) = validate_invocation_target(&target) {
        return usage(&message);
    }
    if let Some(warning) = compatibility_warning(&args) {
        if output.warnings || std::io::IsTerminal::is_terminal(&std::io::stderr()) {
            eprintln!("warning: {warning}");
        }
    }
    let args = commands::Command::normalize(args).into_dispatch_args();
    if let Some(reason) = unsupported_json_mode(&args) {
        return usage(reason);
    }
    if let Err(error) = preflight_owner_routing(&args) {
        return fail(error);
    }
    let _ = OVERRIDE.set(target);
    if args.len() == 1 {
        let stdin_is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
        let stdout_is_terminal = std::io::IsTerminal::is_terminal(&std::io::stdout());
        if matches!(
            workspace::choose_entry(
                workspace::InteractiveEntry::Bare,
                stdin_is_terminal,
                stdout_is_terminal,
            ),
            workspace::EntryDecision::Help
        ) {
            let mut command = commands::command();
            let _ = command.print_help();
            println!();
            return std::process::ExitCode::SUCCESS;
        }
    }
    if args.get(1).map(String::as_str) == Some("version") && args.len() == 2 {
        println!("fleety {}", agent_core::VERSION);
        return done(Ok(()));
    }
    if args.get(1).map(String::as_str) == Some("completion") {
        let values = args[2..]
            .iter()
            .filter(|arg| arg.as_str() != "--")
            .collect::<Vec<_>>();
        return match values.as_slice() {
            [shell] => generate_completion(shell),
            _ => usage("usage: fleety completion <bash|zsh|fish|powershell|elvish>"),
        };
    }
    if args.get(1).map(String::as_str) == Some("doctor") && args.len() == 2 {
        return doctor().await;
    }
    // Machine and quiet modes guarantee clean streams. The shared tracing
    // subscriber writes directly to process stderr, outside the CLI output
    // sink, so do not install it for these modes.
    if !json_mode() && !quiet_mode() {
        obs::init();
    }
    // Seed env from ~/.fleety/config.toml so client settings (e.g. transport mode)
    // set via `fleety config` apply; an explicit env var still wins.
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    // `init` defers migration until its explicit URL has passed local syntax
    // validation. Other already-validated commands retain the one-time legacy
    // migration before they resolve a connection.
    if args.get(1).map(String::as_str) != Some("init") {
        if let Err(error) = connection::migrate_from_config_json() {
            return fail(error);
        }
    }
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
                    if let Err(error) = connection::migrate_from_config_json() {
                        return fail(error);
                    }
                    return done(init_interactive(name).await);
                }
                return usage(
                    "usage: fleety init <ws-url> [--name <name>]   (e.g. ws://host:8787)",
                );
            }
            let name = name.unwrap_or_else(|| "default".to_string());
            // Validate the same explicit endpoint contract used by set-url and
            // Settings before any migration or network work.
            if let Err(error) = connection::validate_ws_url(&url) {
                eprintln!(
                    "error: {} ({})",
                    terminal_safe_text(&error.report().message),
                    terminal_safe_endpoint(&url)
                );
                eprintln!("hint: e.g. fleety init ws://192.168.1.10:8787");
                return std::process::ExitCode::from(2);
            }
            // Migrate only after the explicit endpoint has passed local syntax
            // validation. A malformed invocation must not create or rename any
            // user file before returning its usage error.
            if let Err(error) = connection::migrate_from_config_json() {
                return fail(error);
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
        Some("tui") if args.len() == 2 => done(workspace::run(workspace::Route::Chat).await),
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
        Some("model-catalog") => done(model_catalog(&args[2..]).await),
        Some("status") if args.len() == 2 => done(status().await),
        Some("status") => usage("usage: fleety status"),
        Some("voice") if args.len() == 2 => done(voice_chat().await),
        Some("voice") => usage("usage: fleety voice"),
        Some("config") => {
            let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin())
                && std::io::IsTerminal::is_terminal(&std::io::stdout());
            let res = match config::split_target(&args[2..]) {
                Err(e) => Err(e),
                Ok((_requested, rest)) if rest.is_empty() && is_tty => {
                    workspace::run(workspace::Route::Settings(
                        workspace::SettingsPage::Connection,
                    ))
                    .await
                }
                Ok((requested, rest)) if matches!(rest.as_slice(), [command] if command == "open" || command == "edit") => {
                    if !is_tty {
                        Err(CoreError::Message(
                            "`fleety config open` needs an interactive terminal to open the shared Settings workspace"
                                .into(),
                        ))
                    } else {
                        let page = match &requested {
                            config::Target::Server => workspace::SettingsPage::Server,
                            config::Target::Daemon | config::Target::Device(_) => {
                                workspace::SettingsPage::Daemon
                            }
                            config::Target::Auto | config::Target::Cli => {
                                workspace::SettingsPage::Cli
                            }
                        };
                        let mut session =
                            workspace::WorkspaceSession::new(workspace::Route::Settings(page));
                        if let config::Target::Device(id) = requested {
                            session = session.with_daemon_device_id(id);
                        }
                        workspace::run_session(session).await
                    }
                }
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
                            record_local_cli_context();
                            if json_mode() {
                                let output = fleety_tools::config::run_rendered_scoped(
                                    &rest,
                                    Some(fleety_tools::config::LOCAL_SCOPES),
                                )?;
                                println!("{}", terminal_safe_multiline_redacted(output.trim_end()));
                                Ok(())
                            } else {
                                config::run(&rest)
                            }
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
        None => done(workspace::run(workspace::Route::Chat).await),
    }
}

async fn model_catalog(args: &[String]) -> Result<()> {
    let mut provider = None;
    let mut role = "main".to_string();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--role" => {
                role = args
                    .get(index + 1)
                    .cloned()
                    .ok_or_else(|| CoreError::Message("--role needs a model role".to_string()))?;
                index += 2;
            }
            value if !value.starts_with('-') && provider.is_none() => {
                provider = Some(value.to_string());
                index += 1;
            }
            value => {
                return Err(CoreError::Message(format!(
                    "unexpected model catalog argument '{value}'"
                )))
            }
        }
    }
    let provider = provider.ok_or_else(|| {
        CoreError::Message(
            "model catalog needs a Provider name. Usage: fleety model catalog <provider> [--role <role>]"
                .to_string(),
        )
    })?;
    if !matches!(role.as_str(), "main" | "cheap") {
        return Err(CoreError::Message(format!(
            "unknown model role '{role}' — expected main or cheap"
        )));
    }

    let (mut tx, mut rx, config_protocol, target) = connect_hello_for_auth().await?;
    let snapshot =
        crate::provider_service::load_snapshot(&mut tx, &mut rx, config_protocol).await?;
    let provider_config = snapshot.config.provider(&provider).ok_or_else(|| {
        CoreError::Message(format!(
            "No Provider named '{provider}' on this Server — run `fleety provider list`"
        ))
    })?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let auth_states = crate::provider_service::load_auth_states(
        &mut tx,
        &mut rx,
        config_protocol,
        &snapshot.config,
        now_secs,
    )
    .await;
    let auth = auth_states
        .get(&provider)
        .cloned()
        .unwrap_or(fleety_tools::provider_service::AuthState::NotApplicable);
    match crate::provider_service::catalog_gate(&provider_config.kind, &auth, config_protocol) {
        fleety_tools::provider_service::CatalogState::Idle => {}
        fleety_tools::provider_service::CatalogState::Unavailable(issue)
        | fleety_tools::provider_service::CatalogState::Failed(issue) => {
            return Err(crate::provider_service::issue_as_error(issue))
        }
        _ => {}
    }
    let selection = crate::provider_service::ModelSelection::loading(
        target.url,
        provider.clone(),
        role.clone(),
    );
    let models = crate::provider_service::fetch_catalog(
        &mut tx,
        &mut rx,
        config_protocol,
        &selection.request(),
    )
    .await
    .map_err(crate::provider_service::issue_as_error)?;
    if json_mode() {
        emit_json(
            serde_json::json!({
                "provider": provider,
                "role": role,
                "models": models,
            }),
            serde_json::json!([]),
        );
    } else {
        if !quiet_mode() {
            println!(
                "Available models for Provider '{}' (role {}):",
                terminal_safe_text(&provider),
                terminal_safe_text(&role)
            );
        }
        for model in models {
            if quiet_mode() {
                println!("{}", terminal_safe_text(&model));
            } else {
                println!("  {}", terminal_safe_text(&model));
            }
        }
    }
    Ok(())
}

fn record_local_cli_context() {
    set_json_context(serde_json::json!({
        "profile": null,
        "source": "local",
        "owner": "cli",
        "device_id": null,
        "endpoint": null,
        "server_identity": null,
    }));
}

fn compatibility_warning(args: &[String]) -> Option<String> {
    match args.get(1).map(String::as_str) {
        Some("server") => {
            Some("`fleety server` is a compatibility alias; prefer `fleety connection`".to_string())
        }
        Some("tui") => {
            Some("`fleety tui` is a compatibility alias; prefer `fleety chat`".to_string())
        }
        Some("auth") => match args.get(2).map(String::as_str) {
            Some(action @ ("login" | "logout" | "status")) => {
                let provider = args.get(3).map(String::as_str).unwrap_or("<provider>");
                Some(format!(
                    "`fleety auth` is a compatibility alias; prefer `fleety provider {action} {provider}`"
                ))
            }
            _ => Some(
                "`fleety auth` is a compatibility alias; prefer `fleety provider login|logout|status`"
                    .to_string(),
            ),
        },
        Some("config") => {
            let mut index = 2;
            while matches!(
                args.get(index).map(String::as_str),
                Some("--owner" | "--target")
            ) {
                index += 2;
            }
            match args.get(index).map(String::as_str) {
                Some("provider") => Some(
                    "`fleety config provider` is a compatibility alias; prefer `fleety provider`"
                        .to_string(),
                ),
                Some("model") => Some(
                    "`fleety config model` is a compatibility alias; prefer `fleety model`"
                        .to_string(),
                ),
                _ => None,
            }
        }
        _ => None,
    }
}

fn unsupported_json_mode(args: &[String]) -> Option<&'static str> {
    if !json_mode() {
        return None;
    }
    match args.get(1).map(String::as_str) {
        None => Some("--json needs a command; run `fleety --help` to choose one"),
        Some("tui" | "voice") => Some(
            "--json is not available for interactive terminal sessions; use a non-interactive command",
        ),
        Some("acp") => Some(
            "--json cannot wrap ACP because stdout is reserved for JSON-RPC protocol frames",
        ),
        Some("daemon" | "update") => Some(
            "--json is not yet available for delegated service lifecycle commands",
        ),
        Some("completion") => Some(
            "--json cannot wrap completion because stdout must contain only shell completion source",
        ),
        Some("auth") if args.get(2).map(String::as_str) == Some("login") => Some(
            "--json is not available for browser-based OAuth login; use `fleety provider status <provider> --json` for machine-readable state",
        ),
        Some("config")
            if args.iter().any(|arg| arg == "edit")
                || (args.iter().any(|arg| arg == "provider")
                    && args.iter().any(|arg| arg == "edit")) =>
        {
            Some("--json is not available for interactive settings editors")
        }
        _ => None,
    }
}

/// Validate semantic config ownership before logging, config reads, legacy
/// migration, network access, or persistence. The placeholder device id is
/// never sent or stored; it only lets the pure router materialize `daemon` so
/// explicit owner/key mismatches can fail at the true pre-I/O boundary.
fn preflight_owner_routing(args: &[String]) -> Result<()> {
    if args.get(1).map(String::as_str) != Some("config") {
        return Ok(());
    }
    let (requested, rest) = config::split_target(args.get(2..).unwrap_or_default())?;
    let _ = config::resolve_target(requested, &rest, "preflight-device")?;
    Ok(())
}

/// Write generated completion source directly to stdout. This command is
/// dispatched before logging, config seeding, migration, or any network work;
/// installing the source remains the shell's responsibility.
fn generate_completion(shell: &str) -> std::process::ExitCode {
    let shell = match shell {
        "bash" => clap_complete::Shell::Bash,
        "zsh" => clap_complete::Shell::Zsh,
        "fish" => clap_complete::Shell::Fish,
        "powershell" => clap_complete::Shell::PowerShell,
        "elvish" => clap_complete::Shell::Elvish,
        _ => return usage("usage: fleety completion <bash|zsh|fish|powershell|elvish>"),
    };
    let mut command = commands::command();
    clap_complete::generate(shell, &mut command, "fleety", &mut std::io::stdout());
    std::process::ExitCode::SUCCESS
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

struct ChatWelcome {
    server_identity: Option<String>,
    server_version: String,
    config_protocol: u32,
}

async fn receive_chat_welcome(rx: &mut Rx) -> Result<ChatWelcome> {
    match recv(rx).await? {
        Some(ServerMsg::Welcome {
            server_fingerprint,
            server_version,
            config_protocol,
            ..
        }) => Ok(ChatWelcome {
            server_identity: server_fingerprint,
            server_version,
            config_protocol,
        }),
        Some(ServerMsg::Error { error }) => Err(CoreError::Message(format!(
            "the Server rejected Chat: {}",
            error.message
        ))),
        other => Err(CoreError::Message(format!(
            "expected Chat Welcome, got {}",
            server_msg_kind_option(other.as_ref())
        ))),
    }
}

fn chat_model_context(
    entries: &[fleety_protocol::ConfigEntry],
    providers_json: &str,
) -> (Option<String>, Option<String>) {
    if let Ok(config) =
        serde_json::from_str::<fleety_tools::providers_config::ProvidersConfig>(providers_json)
    {
        if let Some(member) = config
            .models
            .get("main")
            .and_then(|pool| pool.members.first())
        {
            return (Some(member.provider.clone()), Some(member.model.clone()));
        }
    }
    let model = entries
        .iter()
        .find(|entry| entry.key == "FLEETY_MODEL")
        .map(|entry| entry.value.clone())
        .filter(|value| !value.is_empty());
    (None, model)
}

async fn load_chat_model_context(
    tx: &mut Tx,
    rx: &mut Rx,
    config_protocol: u32,
) -> (Option<String>, Option<String>) {
    if config_protocol < 5 {
        return (None, None);
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        crate::provider_service::load_snapshot(tx, rx, config_protocol),
    )
    .await
    {
        Ok(Ok(snapshot)) => {
            let providers_json = serde_json::to_string(&snapshot.config).unwrap_or_default();
            chat_model_context(&snapshot.entries, &providers_json)
        }
        _ => (None, None),
    }
}

async fn run_tui(session: workspace::WorkspaceSession) -> Result<workspace::SessionResult> {
    let workspace::WorkspaceSession {
        mut workspace,
        chat: mut app,
        chat_transport: _,
        mut input,
        daemon_device_id,
    } = session;

    let mut chat_transport = None;
    workspace.reduce(workspace::Action::Connect);
    let (mut tx, mut rx, target) = match open(&RemoteOwner::Server).await {
        Ok(connection) => connection,
        Err(error) => {
            let message = error.report().message;
            workspace.reduce(workspace::Action::Offline(message.clone()));
            workspace.reduce(workspace::Action::PushNotice(
                workspace::Notice::error("Chat connection unavailable")
                    .details(message)
                    .remediation("Select a reachable profile in Settings"),
            ));
            workspace.reduce(workspace::Action::Navigate(workspace::Route::Settings(
                workspace::SettingsPage::Connection,
            )));
            return Ok(workspace::SessionResult::Continue(Box::new(
                workspace::WorkspaceSession {
                    workspace,
                    chat: app,
                    chat_transport,
                    input,
                    daemon_device_id,
                },
            )));
        }
    };
    let token = target.token.clone();
    send(&mut tx, &hello(token.clone(), None)).await?;

    // Chat is not considered connected until the authenticated Welcome has
    // supplied the transport identity used by the persistent header.
    let welcome = match receive_chat_welcome(&mut rx).await {
        Ok(welcome) => welcome,
        Err(error) => {
            let _ = tx.close().await;
            let message = error.report().message;
            workspace.reduce(workspace::Action::Offline(message.clone()));
            workspace.reduce(workspace::Action::PushNotice(
                workspace::Notice::error("Chat handshake failed")
                    .details(message)
                    .remediation("Verify or select the Server profile in Settings"),
            ));
            workspace.reduce(workspace::Action::Navigate(workspace::Route::Settings(
                workspace::SettingsPage::Connection,
            )));
            return Ok(workspace::SessionResult::Continue(Box::new(
                workspace::WorkspaceSession {
                    workspace,
                    chat: app,
                    chat_transport,
                    input,
                    daemon_device_id,
                },
            )));
        }
    };
    maybe_converge_cli(&welcome.server_version).await;
    tofu_pin(welcome.server_identity.as_deref(), &target);
    let server_identity = welcome.server_identity.clone();
    let (provider, model) =
        load_chat_model_context(&mut tx, &mut rx, welcome.config_protocol).await;
    workspace::activate_chat_transport(
        &mut workspace,
        &mut chat_transport,
        workspace::ChatTransportContext {
            profile: workspace_profile_label(&target),
            endpoint: target.url.clone(),
            server_identity: server_identity.clone(),
            server_version: (!welcome.server_version.is_empty())
                .then_some(welcome.server_version.clone()),
            provider,
            model,
        },
    );

    let mut terminal = ratatui::init();
    app.status = remote_context(&target, &RemoteOwner::Server, server_identity.as_deref());
    // Redraw only when something changed — a key/frame event, or a spinner tick
    // while waiting. Idle ticks must not force periodic repaints (the spinner is
    // static when no turn is in flight).
    let mut dirty = true;
    let mut conversations_requested = false;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(120));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let result = loop {
        if matches!(&workspace.route, workspace::Route::Settings(_)) {
            break Ok(workspace::SessionResult::Continue(Box::new(
                workspace::WorkspaceSession {
                    workspace,
                    chat: app,
                    chat_transport,
                    input,
                    daemon_device_id,
                },
            )));
        }
        if matches!(&workspace.route, workspace::Route::Conversations) {
            if !conversations_requested {
                app.conversations_status = "Loading conversations…".into();
                conversations_requested = true;
                if let Err(error) =
                    send(&mut tx, &ClientMsg::ConversationList { limit: Some(50) }).await
                {
                    app.conversations_status =
                        format!("Could not load conversations: {}", error.report().message);
                }
                dirty = true;
            }
        } else {
            conversations_requested = false;
        }
        if dirty {
            if let Err(e) = terminal.draw(|frame| {
                workspace::render(frame, &workspace, |frame, area| match &workspace.route {
                    workspace::Route::Chat => tui::render_in_area(frame, &app, area),
                    workspace::Route::Conversations => {
                        tui::render_conversations_in_area(frame, &app, area)
                    }
                    workspace::Route::Settings(page) => frame.render_widget(
                        ratatui::widgets::Paragraph::new(format!(
                            "{page:?} settings are loading into the shared workspace."
                        ))
                        .block(ratatui::widgets::Block::bordered().title("Settings")),
                        area,
                    ),
                    workspace::Route::ConnectionPicker => frame.render_widget(
                        ratatui::widgets::Paragraph::new(
                            "Select a saved profile. Esc keeps the current connection.",
                        )
                        .block(ratatui::widgets::Block::bordered().title("Profiles")),
                        area,
                    ),
                    workspace::Route::CommandPalette | workspace::Route::Modal(_) => {}
                });
            }) {
                break Err(CoreError::Message(format!("draw failed: {e}")));
            }
            dirty = false;
        }
        if app.should_quit {
            break Ok(workspace::SessionResult::Exit);
        }
        tokio::select! {
            key = input.recv() => {
                dirty = true;
                match key {
                Some(k) => {
                    let key_context = workspace::KeyContext {
                        turn_in_flight: app.turn_in_flight,
                        has_unsent_input: !app.input.text().is_empty()
                            || !app.pending_attachments.is_empty(),
                        has_dirty_owner: workspace.owners.values().any(|owner| {
                            matches!(
                                owner,
                                workspace::OwnerState::Dirty(_)
                                    | workspace::OwnerState::Applying(_)
                                    | workspace::OwnerState::Conflict(_, _)
                                    | workspace::OwnerState::Failed(_, _)
                            )
                        }),
                        text_input_focused: !app.input.text().is_empty(),
                    };
                    match workspace::on_key(&mut workspace, k, key_context) {
                        workspace::KeyOutcome::ExitRequested => {
                            app.should_quit = true;
                            continue;
                        }
                        workspace::KeyOutcome::Consumed(effects) => {
                            for effect in effects {
                                match effect {
                                    workspace::Effect::CancelTurn => {
                                        if let Err(error) = send(
                                            &mut tx,
                                            &ClientMsg::CancelTurn {
                                                conversation_id: None,
                                            },
                                        )
                                        .await
                                        {
                                            app.status = format!(
                                                "cancel failed: {}",
                                                error.report().message
                                            );
                                        }
                                    }
                                    workspace::Effect::ConnectCurrentProfile => {
                                        if let Some((new_tx, new_rx)) = reconnect(
                                            &target,
                                            server_identity.as_deref(),
                                            &mut app,
                                            &mut workspace,
                                            &mut chat_transport,
                                            &mut terminal,
                                            &mut input,
                                        )
                                        .await
                                        {
                                            tx = new_tx;
                                            rx = new_rx;
                                        }
                                    }
                                    workspace::Effect::RetryNotice(id) => {
                                        app.status = format!(
                                            "retry requested for notice {id}; use the active route action"
                                        );
                                    }
                                    workspace::Effect::RunDoctor => {
                                        workspace.reduce(workspace::Action::PushNotice(
                                            workspace::Notice::error(
                                                "Doctor runs in command mode",
                                            )
                                            .remediation(
                                                "Exit the workspace and run `fleety doctor`",
                                            ),
                                        ));
                                    }
                                    workspace::Effect::ApplyOwner(_) => {}
                                }
                            }
                            continue;
                        }
                        workspace::KeyOutcome::Forward => {}
                    }
                    if matches!(&workspace.route, workspace::Route::Conversations) {
                        match k.code {
                            ratatui::crossterm::event::KeyCode::Up => {
                                app.conversation_selected =
                                    app.conversation_selected.saturating_sub(1);
                            }
                            ratatui::crossterm::event::KeyCode::Down => {
                                app.conversation_selected = (app.conversation_selected + 1)
                                    .min(app.conversations.len().saturating_sub(1));
                            }
                            ratatui::crossterm::event::KeyCode::Enter => {
                                if let Some(conversation) =
                                    app.conversations.get(app.conversation_selected)
                                {
                                    let conversation_id = conversation.conversation_id.clone();
                                    if let Err(error) = send(
                                        &mut tx,
                                        &ClientMsg::Resume {
                                            conversation_id: conversation_id.clone(),
                                            after_seq: 0,
                                        },
                                    )
                                    .await
                                    {
                                        app.conversations_status = format!(
                                            "Could not open conversation: {}",
                                            error.report().message
                                        );
                                    } else {
                                        app.last_conversation_id = Some(conversation_id.clone());
                                        app.last_seq = 0;
                                        workspace.context.conversation_id = Some(conversation_id);
                                        workspace.reduce(workspace::Action::Navigate(
                                            workspace::Route::Chat,
                                        ));
                                        app.status = "restoring conversation…".into();
                                    }
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if !matches!(&workspace.route, workspace::Route::Chat) {
                        continue;
                    }
                    if k.code == ratatui::crossterm::event::KeyCode::Enter
                        && !k
                            .modifiers
                            .contains(ratatui::crossterm::event::KeyModifiers::ALT)
                        && !workspace::chat_submission_enabled(
                            &workspace,
                            chat_transport.as_ref(),
                        )
                    {
                        app.status =
                            "Chat is reconnecting; draft and attachments are retained".into();
                        continue;
                    }
                    match tui::on_key(&mut app, k) {
                    tui::Action::Send { text, attachments } => {
                        if let Err(e) = send(&mut tx, &ClientMsg::UserMessage {
                            conversation_id: None,
                            text,
                            origin: OriginContext::default(),
                            attachments,
                            voice: false,
                            acting_user: None,
                        }).await {
                            app.status = format!("send failed: {}", e.report().message);
                        } else {
                            app.commit_send();
                            app.status = "sent; waiting…".to_string();
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
                        if let Err(e) = send(&mut tx, &ClientMsg::Approve { approval_id: approval_id.clone() }).await {
                            app.status = format!("approve failed: {}", e.report().message);
                        } else {
                            app.commit_approval(&approval_id, true);
                        }
                    }
                    tui::Action::Deny(approval_id) => {
                        if let Err(e) = send(&mut tx, &ClientMsg::Deny { approval_id: approval_id.clone() }).await {
                            app.status = format!("deny failed: {}", e.report().message);
                        } else {
                            app.commit_approval(&approval_id, false);
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
                }
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
                    Ok(ServerMsg::ConversationListResult { conversations_json }) => {
                        match tui::parse_conversation_summaries(&conversations_json) {
                            Ok(conversations) => {
                                app.conversations = conversations;
                                app.conversation_selected = app
                                    .conversation_selected
                                    .min(app.conversations.len().saturating_sub(1));
                                app.conversations_status = format!(
                                    "{} conversation(s) · Enter opens the selected conversation",
                                    app.conversations.len()
                                );
                            }
                            Err(error) => {
                                app.conversations_status =
                                    format!("Invalid conversation list: {error}");
                            }
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
                            workspace.reduce(workspace::Action::AuthenticationRequired);
                        } else {
                            // The turn ended with an error: clear in-flight so
                            // Esc goes back to quitting.
                            app.status = format!("agent error: {}", error.message);
                            workspace.reduce(workspace::Action::PushNotice(
                                workspace::Notice::error("Agent error")
                                    .details(error.message)
                                    .remediation(
                                        error.remediation.unwrap_or_else(|| "Retry".to_string()),
                                    ),
                            ));
                        }
                    }
                    _ => {}
                },
                None => {
                    // The link dropped: try to reconnect with capped backoff and
                    // resume the conversation, instead of exiting outright. On a
                    // give-up, reconnect() has already set the status + should_quit.
                    if let Some((new_tx, new_rx)) =
                        reconnect(
                            &target,
                            server_identity.as_deref(),
                            &mut app,
                            &mut workspace,
                            &mut chat_transport,
                            &mut terminal,
                            &mut input,
                        )
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
/// streams. Identity changes are typed separately from ordinary transport
/// failures so the caller can fail closed without matching human error text.
#[derive(Debug)]
enum ChatReconnectError {
    IdentityChanged,
    Other,
}

impl From<CoreError> for ChatReconnectError {
    fn from(_: CoreError) -> Self {
        Self::Other
    }
}

async fn reconnect_chat_once(
    target: &connection::Resolved,
    expected_identity: Option<&str>,
    conversation_id: Option<String>,
    after_seq: u64,
) -> std::result::Result<(Tx, Rx, workspace::ChatTransportContext), ChatReconnectError> {
    let connection = transport::connect(&target.url, target.token.as_deref()).await?;
    let (mut tx, mut rx) = connection.split();
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    let welcome = receive_chat_welcome(&mut rx).await?;
    if welcome.server_identity.as_deref() != expected_identity {
        let _ = tx.close().await;
        return Err(ChatReconnectError::IdentityChanged);
    }
    let (provider, model) =
        load_chat_model_context(&mut tx, &mut rx, welcome.config_protocol).await;
    if let Some(conversation_id) = conversation_id {
        send(
            &mut tx,
            &ClientMsg::Resume {
                conversation_id,
                after_seq,
            },
        )
        .await?;
    }
    let context = workspace::ChatTransportContext {
        profile: workspace_profile_label(target),
        endpoint: target.url.clone(),
        server_identity: welcome.server_identity,
        server_version: (!welcome.server_version.is_empty()).then_some(welcome.server_version),
        provider,
        model,
    };
    Ok((tx, rx, context))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconnectKeyResult {
    Continue,
    LeaveReconnect,
}

fn handle_reconnect_key(
    app: &mut tui::App,
    workspace: &mut workspace::WorkspaceState,
    key: ratatui::crossterm::event::KeyEvent,
) -> ReconnectKeyResult {
    let context = workspace::KeyContext {
        turn_in_flight: false,
        has_unsent_input: !app.input.text().is_empty() || !app.pending_attachments.is_empty(),
        has_dirty_owner: workspace.owners.values().any(|owner| {
            matches!(
                owner,
                workspace::OwnerState::Dirty(_)
                    | workspace::OwnerState::Applying(_)
                    | workspace::OwnerState::Conflict(_, _)
                    | workspace::OwnerState::Failed(_, _)
            )
        }),
        text_input_focused: !app.input.text().is_empty(),
    };
    match workspace::on_key(workspace, key, context) {
        workspace::KeyOutcome::ExitRequested => {
            app.should_quit = true;
            return ReconnectKeyResult::LeaveReconnect;
        }
        workspace::KeyOutcome::Consumed(effects) => {
            for effect in effects {
                match effect {
                    workspace::Effect::RetryNotice(_)
                    | workspace::Effect::ConnectCurrentProfile => {
                        app.status = "reconnect already in progress".into();
                    }
                    workspace::Effect::RunDoctor => {
                        workspace.reduce(workspace::Action::PushNotice(
                            workspace::Notice::error("Doctor runs in command mode")
                                .remediation("Exit the workspace and run `fleety doctor`"),
                        ));
                    }
                    workspace::Effect::CancelTurn | workspace::Effect::ApplyOwner(_) => {}
                }
            }
        }
        workspace::KeyOutcome::Forward => {
            if matches!(&workspace.route, workspace::Route::Chat) {
                use ratatui::crossterm::event::{KeyCode, KeyModifiers};
                if key.code == KeyCode::Enter && !key.modifiers.contains(KeyModifiers::ALT) {
                    app.status = "Chat is reconnecting; draft and attachments are retained".into();
                } else {
                    match tui::on_key(app, key) {
                        tui::Action::PasteFromClipboard => {
                            app.status = "Clipboard paste is available after reconnect".into();
                        }
                        tui::Action::Approve(_) | tui::Action::Deny(_) => {
                            app.status = "Approval is retained until reconnect succeeds".into();
                        }
                        tui::Action::Quit => return ReconnectKeyResult::LeaveReconnect,
                        tui::Action::Send { .. } | tui::Action::CancelTurn | tui::Action::None => {}
                    }
                }
            }
        }
    }
    if app.should_quit || matches!(&workspace.route, workspace::Route::Settings(_)) {
        ReconnectKeyResult::LeaveReconnect
    } else {
        ReconnectKeyResult::Continue
    }
}

fn mark_reconnect_exhausted(app: &mut tui::App, workspace: &mut workspace::WorkspaceState) {
    app.status = "disconnected — reconnect attempts exhausted".to_string();
    workspace.reduce(workspace::Action::Offline(
        "Reconnect attempts exhausted".to_string(),
    ));
    workspace.reduce(workspace::Action::PushNotice(
        workspace::Notice::error("Chat reconnect attempts exhausted")
            .remediation("Check the selected profile and reconnect from Connection Settings"),
    ));
    workspace.reduce(workspace::Action::Navigate(workspace::Route::Settings(
        workspace::SettingsPage::Connection,
    )));
}

fn prepare_for_chat_reconnect(
    app: &mut tui::App,
    chat_transport: &mut Option<workspace::ChatTransportContext>,
) {
    // A dropped link ends the active turn and every approval gate owned by the
    // old transport. Draft text and attachments are deliberately untouched.
    app.turn_in_flight = false;
    app.expire_pending_approvals();
    *chat_transport = None;
}

async fn reconnect(
    target: &connection::Resolved,
    expected_identity: Option<&str>,
    app: &mut tui::App,
    workspace: &mut workspace::WorkspaceState,
    chat_transport: &mut Option<workspace::ChatTransportContext>,
    terminal: &mut ratatui::DefaultTerminal,
    input: &mut workspace::WorkspaceInput,
) -> Option<(Tx, Rx)> {
    prepare_for_chat_reconnect(app, chat_transport);

    const MAX_ATTEMPTS: u32 = 8;
    const MAX_DELAY_MS: u64 = 30_000;
    let mut delay_ms: u64 = 500;

    for attempt in 1..=MAX_ATTEMPTS {
        app.advance_spinner();
        app.status = format!(
            "{} reconnecting… (attempt {attempt}/{MAX_ATTEMPTS}) — Ctrl+C to quit",
            app.spinner_char()
        );
        workspace.reduce(workspace::Action::ConnectionLost {
            attempt,
            backoff_ms: delay_ms,
        });
        let _ = terminal.draw(|frame| {
            workspace::render(frame, workspace, |frame, area| {
                if matches!(&workspace.route, workspace::Route::Chat) {
                    tui::render_in_area(frame, app, area);
                }
            });
        });

        let reconnect_attempt = reconnect_chat_once(
            target,
            expected_identity,
            app.last_conversation_id.clone(),
            app.last_seq,
        );
        tokio::pin!(reconnect_attempt);
        let attempt_result = loop {
            tokio::select! {
                result = &mut reconnect_attempt => break result,
                key = input.recv() => {
                    let Some(key) = key else {
                        app.should_quit = true;
                        return None;
                    };
                    if handle_reconnect_key(app, workspace, key)
                        == ReconnectKeyResult::LeaveReconnect
                    {
                        return None;
                    }
                    let _ = terminal.draw(|frame| {
                        workspace::render(frame, workspace, |frame, area| {
                            if matches!(&workspace.route, workspace::Route::Chat) {
                                tui::render_in_area(frame, app, area);
                            }
                        });
                    });
                }
            }
        };

        match attempt_result {
            Ok((tx, rx, context)) => {
                app.status = "reconnected".to_string();
                workspace::activate_chat_transport(workspace, chat_transport, context);
                return Some((tx, rx));
            }
            Err(ChatReconnectError::IdentityChanged) => {
                app.status = "reconnect refused: Server identity changed".into();
                workspace.reduce(workspace::Action::Offline(
                    "Server identity changed during Chat reconnect".into(),
                ));
                workspace.reduce(workspace::Action::PushNotice(
                    workspace::Notice::error("Chat reconnect identity changed")
                        .remediation("Verify the selected profile in Settings"),
                ));
                workspace.reduce(workspace::Action::Navigate(workspace::Route::Settings(
                    workspace::SettingsPage::Connection,
                )));
                return None;
            }
            Err(ChatReconnectError::Other) => {}
        }

        // Keep the complete local workspace responsive during backoff. Draft
        // editing, help, the command palette, and navigation to Settings all
        // remain available while transport recovery continues.
        let sleep = tokio::time::sleep(std::time::Duration::from_millis(delay_ms));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                key = input.recv() => {
                    let Some(key) = key else {
                        app.should_quit = true;
                        return None;
                    };
                    if handle_reconnect_key(app, workspace, key)
                        == ReconnectKeyResult::LeaveReconnect
                    {
                        return None;
                    }
                    let _ = terminal.draw(|frame| {
                        workspace::render(frame, workspace, |frame, area| {
                            if matches!(&workspace.route, workspace::Route::Chat) {
                                tui::render_in_area(frame, app, area);
                            }
                        });
                    });
                }
                _ = &mut sleep => break,
            }
        }
        delay_ms = delay_ms.saturating_mul(2).min(MAX_DELAY_MS);
    }

    mark_reconnect_exhausted(app, workspace);
    None
}

/// The per-invocation profile or URL override (set once in `main`;
/// `Target::Current` when none).
static OVERRIDE: std::sync::OnceLock<Target> = std::sync::OnceLock::new();

/// Extract value-free output controls before command normalization. Like the
/// profile selector pass, this stops at `--` so flag-like prompt text remains
/// data. Repeating a boolean flag is harmless and resolves to the same mode.
fn take_output_options(mut args: Vec<String>) -> (Vec<String>, OutputOptions) {
    let mut options = OutputOptions::default();
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--" {
            break;
        }
        let recognized = match args[index].as_str() {
            "--json" => {
                options.json = true;
                true
            }
            "--quiet" => {
                options.quiet = true;
                true
            }
            "--no-color" => {
                options.no_color = true;
                true
            }
            "--warnings" => {
                options.warnings = true;
                true
            }
            _ => false,
        };
        if recognized {
            args.remove(index);
        } else {
            index += 1;
        }
    }
    (args, options)
}

fn expand_long_option_equals(args: Vec<String>) -> Vec<String> {
    let mut expanded = Vec::with_capacity(args.len());
    let mut positional_only = false;
    for arg in args {
        if positional_only {
            expanded.push(arg);
            continue;
        }
        if arg == "--" {
            positional_only = true;
            expanded.push(arg);
            continue;
        }
        if let Some(option) = arg.strip_prefix("--") {
            if let Some((name, value)) = option.split_once('=') {
                expanded.push(format!("--{name}"));
                expanded.push(value.to_string());
                continue;
            }
        }
        if let Some(value) = arg.strip_prefix("-s").filter(|value| !value.is_empty()) {
            expanded.push("-s".to_string());
            expanded.push(value.strip_prefix('=').unwrap_or(value).to_string());
            continue;
        }
        expanded.push(arg);
    }
    expanded
}

/// Pull a per-invocation selector out of the argument list: `--profile <name>`
/// selects a saved profile, while legacy `-s`/`--server <ws-url>` and `--url`
/// connect directly without persisting anything.
/// Returns the cleaned args and the resolved [`Target`].
fn take_server_override(
    mut args: Vec<String>,
) -> std::result::Result<(Vec<String>, Target), String> {
    let mut target = Target::Current;
    let mut seen = false;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--" {
            break;
        }
        match args[i].as_str() {
            // `acp install --server` belongs to the ACP output config, not the
            // invocation profile selector.
            "-s" | "--server"
                if !(args.get(1).map(String::as_str) == Some("acp")
                    && args.get(2).map(String::as_str) == Some("install")) =>
            {
                if seen {
                    return Err("choose only one of --profile, --server, or --url".to_string());
                }
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
                    .ok_or_else(|| format!("{} needs a WebSocket URL", args[i]))?;
                target = Target::Url(value);
                args.drain(i..=i + 1);
                seen = true;
            }
            "--profile" => {
                if seen {
                    return Err("choose only one of --profile, --server, or --url".to_string());
                }
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
                    .ok_or_else(|| "--profile needs a profile name".to_string())?;
                target = Target::Named(value);
                args.drain(i..=i + 1);
                seen = true;
            }
            "--url" => {
                if seen {
                    return Err("choose only one of --profile, --server, or --url".to_string());
                }
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with('-'))
                    .cloned()
                    .ok_or_else(|| "--url needs a WebSocket URL".to_string())?;
                target = Target::Url(value);
                args.drain(i..=i + 1);
                seen = true;
            }
            _ => i += 1,
        }
    }
    Ok((args, target))
}

fn validate_invocation_target(target: &Target) -> std::result::Result<(), String> {
    let Target::Url(value) = target else {
        return Ok(());
    };
    let valid = !value.chars().any(char::is_control)
        && reqwest::Url::parse(value)
            .is_ok_and(|url| matches!(url.scheme(), "ws" | "wss") && url.host_str().is_some());
    if valid {
        Ok(())
    } else {
        Err(
            "--server needs a valid ws:// or wss:// URL; use `fleety connection use <profile>` for a saved profile"
                .to_string(),
        )
    }
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
    // The collecting mDNS path preserves advertised fingerprints as display /
    // ordering hints only. Unsigned TXT data never grants profile provenance or
    // a stored token.
    let r = connection::resolve(&conns, &over, env_url, env_token, || {
        prefer_loopback_discovery(
            || loopback_server_up(std::time::Duration::from_millis(300)),
            || connection::discover_for_connections(&conns, std::time::Duration::from_secs(2)),
        )
    })?;
    if !json_mode() && !quiet_mode() {
        match &r.source {
            connection::Source::Env => {
                eprintln!(
                    "note: FLEETY_AGENT_URL overrides the current server ({})",
                    redact_endpoint(&r.url)
                )
            }
            connection::Source::Mdns if r.url == local_server_url() => eprintln!(
                "using this host's local server ({}) — same-host trusted, no pairing needed",
                redact_endpoint(&r.url)
            ),
            connection::Source::Mdns => {
                eprintln!("discovered agent on the LAN: {}", redact_endpoint(&r.url))
            }
            connection::Source::Default => eprintln!(
                "no server configured and none found on the LAN — trying the local default \
                 {} (point at one with `fleety init <ws-url>`)",
                redact_endpoint(&r.url)
            ),
            connection::Source::OverrideProfile(_)
            | connection::Source::OverrideUrl
            | connection::Source::Profile(_) => {}
        }
    }
    Ok(r)
}

/// Resolve + connect in one step: returns the split streams plus the resolved
/// target (so callers can read its url/token). Every non-`init` connect site
/// goes through here so they share one resolution (one mDNS probe, one token).
/// A failed saved endpoint is never healed from unsigned mDNS TXT metadata.
/// The user must explicitly select and re-pair the intended Server.
async fn open(owner: &RemoteOwner) -> Result<(Tx, Rx, connection::Resolved)> {
    let target = resolve_target()?;
    record_remote_context(&target, owner, None);
    match transport::connect(&target.url, target.token.as_deref()).await {
        Ok(ws) => {
            let (tx, rx) = ws.split();
            Ok((tx, rx, target))
        }
        Err(e) => match &target.source {
            connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => Err(
                CoreError::Message(profile_recovery_error(name, &e.report().message)),
            ),
            _ => Err(e),
        },
    }
}

fn profile_recovery_error(profile: &str, cause: &str) -> String {
    format!(
        "could not reach saved Server profile '{}': {}. {}",
        terminal_safe_field(profile),
        transport::redact_urls_in_text(cause),
        connection::explicit_repair_guidance()
    )
}

/// The collecting scan + entry type live in `fleety_tools::connection` (shared
/// with the daemon's discovery); the picker below is CLI-only.
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
    format!(
        "  {}. {}  {}{}",
        idx + 1,
        terminal_safe_field(&s.name),
        terminal_safe_endpoint(&s.url),
        tag
    )
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
    mdns: impl FnOnce() -> Option<connection::Discovered>,
) -> Option<connection::Discovered> {
    if let Some(url) = loopback() {
        // Loopback is same-host trusted, so it never carries/needs a token.
        return Some(connection::Discovered {
            url,
            fingerprint: None,
        });
    }
    mdns()
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
    println!(
        "Using verified server '{}' ({}).",
        terminal_safe_text(&profile),
        terminal_safe_endpoint(&chosen.url)
    );
    Ok(())
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

/// `fleety pair <code>`: enroll against the current or explicitly overridden
/// named profile; the minted token is written only to that exact profile.
async fn pair(code: String) -> Result<()> {
    let target = resolve_target()?;
    let profile_name = match &target.source {
        connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => {
            name.clone()
        }
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
    print_remote_context(&target, &RemoteOwner::Server, None);
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
            print_remote_context(&target, &RemoteOwner::Server, server_fingerprint.as_deref());
            println!(
                "✓ paired with Server at {}; token saved to profile '{}'",
                terminal_safe_endpoint(&url),
                terminal_safe_text(&profile_name)
            );
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
            let code = terminal_safe_text(&code);
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
        if args[index] == "--" {
            words.extend(args[index + 1..].iter().cloned());
            break;
        }
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
    let initial = connection::load()?;
    let prior_profile = initial.profiles.get(&name).cloned();
    let initial_current = initial.current.clone();
    let credentialed_endpoint_change = prior_profile.as_ref().is_some_and(|profile| {
        profile.url != url
            && profile
                .token
                .as_deref()
                .is_some_and(|token| !token.is_empty())
    });
    if credentialed_endpoint_change && pairing_code.is_none() {
        return Err(CoreError::Message(format!(
            "server profile '{}' is paired to a different endpoint; changing it requires an explicit re-pair. Retry with `fleety init <ws-url> --name <profile> --pairing-code <code>`; the old token was not sent and connections.toml was not changed",
            terminal_safe_field(&name)
        )));
    }
    let token = (!credentialed_endpoint_change)
        .then(|| {
            prior_profile
                .as_ref()
                .and_then(|profile| profile.token.clone())
        })
        .flatten();
    let old_fingerprint = (!credentialed_endpoint_change)
        .then(|| {
            prior_profile
                .as_ref()
                .and_then(|profile| profile.fingerprint.clone())
        })
        .flatten();
    let proposed_target = connection::Resolved {
        url: url.clone(),
        token: token.clone(),
        source: connection::Source::OverrideProfile(name.clone()),
    };
    print_remote_context(&proposed_target, &RemoteOwner::Server, None);
    let (mut tx, mut rx) = transport::connect(&url, token.as_deref()).await?.split();

    send(&mut tx, &hello(token, pairing_code)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            session_id,
            token: minted_token,
            server_fingerprint,
            ..
        }) => {
            if credentialed_endpoint_change && minted_token.is_none() {
                return Err(CoreError::Message(
                    "the selected Server did not complete re-pairing; the old token was not sent and connections.toml was not changed"
                        .to_string(),
                ));
            }
            let observed_fingerprint = server_fingerprint.clone();
            if let Some(seen) = server_fingerprint.as_deref() {
                if connection::tofu_pin_decision(old_fingerprint.as_deref(), seen)
                    == connection::PinDecision::IdentityChanged
                {
                    return Err(CoreError::Message(format!(
                        "server '{name}' has a different identity fingerprint; connections.toml was not changed"
                    )));
                }
            }
            connection::mutate(|live| {
                if live.current != initial_current || live.profiles.get(&name) != prior_profile.as_ref() {
                    return Err(CoreError::Message(
                        "connection profiles changed while init was connecting; nothing was overwritten — retry init"
                            .to_string(),
                    ));
                }
                if live.device_id.is_empty() {
                    live.device_id = fleety_tools::device::device_id();
                }
                let profile = live.profiles.entry(name.clone()).or_default();
                profile.url = url.clone();
                if credentialed_endpoint_change {
                    profile.token = minted_token.clone();
                    profile.fingerprint = server_fingerprint.clone();
                } else if let Some(minted) = minted_token.as_ref() {
                    profile.token = Some(minted.clone());
                }
                if !credentialed_endpoint_change && old_fingerprint.is_none() {
                    profile.fingerprint = server_fingerprint.clone();
                }
                live.current = Some(name.clone());
                Ok(())
            })?;
            let connected = connection::Resolved {
                url: url.clone(),
                token: None,
                source: connection::Source::Profile(name.clone()),
            };
            print_remote_context(
                &connected,
                &RemoteOwner::Server,
                observed_fingerprint.as_deref(),
            );
            println!("✓ connected to Server at {}", terminal_safe_endpoint(&url));
            println!(
                "✓ registered device '{}' with Server profile '{}' (session {})",
                terminal_safe_field(&device_id()),
                terminal_safe_field(&name),
                terminal_safe_field(&session_id)
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
    let (mut tx, mut rx, target) = open(&RemoteOwner::Server).await?;

    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            server_fingerprint,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), &target);
            eprint_remote_context(&target, &RemoteOwner::Server, server_fingerprint.as_deref());
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
            Some(ServerMsg::Assistant { text, .. }) => {
                println!("{}", semantic_or_human_multiline(&text))
            }
            Some(ServerMsg::Done { conversation_id }) => {
                // stderr, so piping the reply stays clean — without this line
                // the id `fleety resume` needs is never shown anywhere.
                let conversation_id = terminal_safe_text(&conversation_id);
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
                eprintln!(
                    "Approve tool '{}' (risk: {})? {}",
                    terminal_safe_text(&tool),
                    terminal_safe_text(&risk),
                    terminal_safe_text(&summary)
                );
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
            println!("you: {}", terminal_safe_multiline_redacted(&spoken));
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
    let (mut tx, mut rx, target) = open(&RemoteOwner::Server).await?;

    send(&mut tx, &hello(target.token.clone(), None)).await?;
    let (conversation, audio_input) = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            conversation_id,
            audio_input,
            server_fingerprint,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), &target);
            print_remote_context(&target, &RemoteOwner::Server, server_fingerprint.as_deref());
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
                    println!("{}", terminal_safe_multiline_redacted(&text));
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
                            Some(url) => println!(
                                "→ look at {} on {}: {}",
                                terminal_safe_text(&a.look_at),
                                terminal_safe_text(&a.device),
                                terminal_safe_endpoint(&url)
                            ),
                            None => println!(
                                "→ look at {} on {}",
                                terminal_safe_text(&a.look_at),
                                terminal_safe_text(&a.device)
                            ),
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
                    eprintln!(
                        "Approve tool '{}' (risk: {})? {}",
                        terminal_safe_text(&tool),
                        terminal_safe_text(&risk),
                        terminal_safe_text(&summary)
                    );
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
    let (mut tx, mut rx, target) = open(&RemoteOwner::Server).await?;

    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            server_fingerprint,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), &target);
            print_remote_context(&target, &RemoteOwner::Server, server_fingerprint.as_deref());
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
            }) => println!(
                "[{seq}] {}: {}",
                terminal_safe_field(&role),
                terminal_safe_field(&content)
            ),
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
    eprintln!(
        "updating fleety {} → {} to match the server…",
        terminal_safe_text(me),
        terminal_safe_text(server_version)
    );
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteOwner {
    Server,
    Daemon(String),
}

impl RemoteOwner {
    fn label(&self) -> String {
        match self {
            Self::Server => "Server".to_string(),
            Self::Daemon(id) => format!("Daemon '{id}'"),
        }
    }
}

fn profile_label(target: &connection::Resolved) -> String {
    match &target.source {
        connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => {
            format!("profile '{name}'")
        }
        connection::Source::OverrideUrl => "transient URL override".to_string(),
        connection::Source::Env => "environment override".to_string(),
        connection::Source::Mdns => "discovered Server".to_string(),
        connection::Source::Default => "default Server".to_string(),
    }
}

pub(crate) fn workspace_profile_label(target: &connection::Resolved) -> String {
    match &target.source {
        connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => {
            name.clone()
        }
        connection::Source::OverrideUrl => "transient URL".into(),
        connection::Source::Env => "environment".into(),
        connection::Source::Mdns => "discovered".into(),
        connection::Source::Default => "default".into(),
    }
}

/// Render one untrusted wire/config field as a single terminal-safe line.
/// Preserve ordinary Unicode, but expose control characters as text so a
/// Server, profile, URL, or device id cannot inject lines or ANSI sequences.
pub(crate) fn terminal_safe_field(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => safe.push_str("\\n"),
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

/// Keep an endpoint useful for diagnosis while removing URL userinfo, query
/// values, and fragments. Those fields are not needed to identify an owner and
/// are common credential/token carriers.
pub(crate) fn redact_endpoint(value: &str) -> String {
    transport::redact_endpoint(value)
}

/// Redact every URL embedded in an error string. Transport errors can convert
/// the configured ws URL to an http SSE URL and append session query values, so
/// replacing only the original endpoint is insufficient.
pub(crate) fn redact_urls_in_text(value: &str) -> String {
    transport::redact_urls_in_text(value)
}

/// Shared human/TUI presentation boundary for untrusted text that may contain
/// both terminal controls and credential-bearing URLs.
pub(crate) fn terminal_safe_text(value: &str) -> String {
    terminal_safe_field(&redact_urls_in_text(value))
}

/// Terminal-safe content boundary for prose whose newlines are semantic, such
/// as Assistant replies, generated usage, and multi-line config results.
pub(crate) fn terminal_safe_multiline(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => safe.push('\n'),
            '\r' => safe.push_str("\\r"),
            '\t' => safe.push('\t'),
            control if control.is_control() => {
                safe.push_str(&format!("\\u{{{:x}}}", control as u32));
            }
            printable => safe.push(printable),
        }
    }
    safe
}

pub(crate) fn terminal_safe_multiline_redacted(value: &str) -> String {
    terminal_safe_multiline(&redact_urls_in_text(value))
}

fn semantic_or_human_multiline(value: &str) -> String {
    if json_mode() {
        value.to_string()
    } else {
        terminal_safe_multiline_redacted(value)
    }
}

pub(crate) fn terminal_safe_endpoint(value: &str) -> String {
    terminal_safe_field(&redact_endpoint(value))
}

fn remote_context(
    target: &connection::Resolved,
    owner: &RemoteOwner,
    server_identity: Option<&str>,
) -> String {
    let mut context = format!(
        "context: {}; owner: {}; endpoint: {}",
        terminal_safe_field(&profile_label(target)),
        terminal_safe_field(&owner.label()),
        terminal_safe_field(&redact_endpoint(&target.url))
    );
    if let Some(identity) = server_identity.filter(|identity| !identity.is_empty()) {
        context.push_str("; server identity: ");
        context.push_str(&terminal_safe_field(identity));
    }
    context
}

fn print_remote_context(
    target: &connection::Resolved,
    owner: &RemoteOwner,
    server_identity: Option<&str>,
) {
    record_remote_context(target, owner, server_identity);
    if json_mode() || quiet_mode() {
        return;
    }
    println!("{}", remote_context(target, owner, server_identity));
}

fn eprint_remote_context(
    target: &connection::Resolved,
    owner: &RemoteOwner,
    server_identity: Option<&str>,
) {
    record_remote_context(target, owner, server_identity);
    if json_mode() || quiet_mode() {
        return;
    }
    eprintln!("{}", remote_context(target, owner, server_identity));
}

fn record_remote_context(
    target: &connection::Resolved,
    owner: &RemoteOwner,
    server_identity: Option<&str>,
) {
    let (profile, source) = match &target.source {
        connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => {
            (serde_json::Value::String(name.clone()), "profile")
        }
        connection::Source::OverrideUrl => (serde_json::Value::Null, "transient_url"),
        connection::Source::Env => (serde_json::Value::Null, "environment"),
        connection::Source::Mdns => (serde_json::Value::Null, "discovery"),
        connection::Source::Default => (serde_json::Value::Null, "default"),
    };
    let (owner_name, device_id) = match owner {
        RemoteOwner::Server => ("server", serde_json::Value::Null),
        RemoteOwner::Daemon(id) => ("daemon", serde_json::Value::String(id.clone())),
    };
    set_json_context(serde_json::json!({
        "profile": profile,
        "source": source,
        "owner": owner_name,
        "device_id": device_id,
        "endpoint": redact_endpoint(&target.url),
        "server_identity": server_identity,
    }));
}

/// Audit data is stored and served by the connected Server. The device ID on
/// an audit frame narrows that Server-owned log; it is context, not a Daemon
/// execution target.
fn record_remote_device_filter(device_id: &str) {
    if let Ok(mut slot) = JSON_CONTEXT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        if let Some(context) = slot.as_mut() {
            context["device_id"] = serde_json::Value::String(device_id.to_string());
        }
    }
    if !json_mode() && !quiet_mode() {
        println!("device filter: {}", terminal_safe_field(device_id));
    }
}

async fn connect_hello_for_owner(
    owner: RemoteOwner,
) -> Result<(Tx, Rx, u32, connection::Resolved)> {
    let (mut tx, mut rx, target) = open(&owner).await?;
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            server_fingerprint,
            config_protocol,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), &target);
            print_remote_context(&target, &owner, server_fingerprint.as_deref());
            Ok((tx, rx, config_protocol, target))
        }
        other => Err(CoreError::Message(hello_failure_message(other.as_ref()))),
    }
}

async fn connect_hello() -> Result<(Tx, Rx)> {
    let (tx, rx, _, _) = connect_hello_for_owner(RemoteOwner::Server).await?;
    Ok((tx, rx))
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
/// existed gain an identity pin without re-pairing); warn — never overwrite —
/// when the identity changed. Best-effort and quiet on the happy path.
fn tofu_pin(fingerprint: Option<&str>, target: &connection::Resolved) {
    let Some(fp) = fingerprint.filter(|f| !f.is_empty()) else {
        return;
    };
    let name = match &target.source {
        connection::Source::Profile(name) | connection::Source::OverrideProfile(name) => name,
        _ => return,
    };
    match connection::pin_profile_fingerprint(name, &target.url, fp) {
        Ok(connection::PinDecision::IdentityChanged) => eprintln!(
            "warning: the server's identity fingerprint changed since it was pinned; keeping \
             the old pin — re-pair (`fleety init` / `fleety pair`) if the server was \
             intentionally rebuilt"
        ),
        Err(error) if !quiet_mode() && !json_mode() => {
            eprintln!("warning: {}", terminal_safe_text(&error.report().message));
        }
        _ => {}
    }
}

/// `connect_hello` for the `auth` command: also returns the server's advertised
/// config protocol (the credential-support gate) and the resolved target, so
/// auth can refuse an old server up front and name the server it acts on.
pub(crate) async fn connect_hello_for_auth() -> Result<(Tx, Rx, u32, connection::Resolved)> {
    let target = resolve_target()?;
    record_remote_context(&target, &RemoteOwner::Server, None);
    let (tx, rx, config_protocol, fingerprint) = connect_hello_for_auth_target(&target).await?;
    print_remote_context(&target, &RemoteOwner::Server, fingerprint.as_deref());
    Ok((tx, rx, config_protocol, target))
}

/// Resolve once for a long-running auth transaction and retain both the target
/// and the server identity observed during preflight.
pub(crate) async fn connect_hello_for_auth_transaction(
) -> Result<(Tx, Rx, u32, connection::Resolved, Option<String>)> {
    let target = resolve_target()?;
    record_remote_context(&target, &RemoteOwner::Server, None);
    let (tx, rx, config_protocol, fingerprint) = connect_hello_for_auth_target(&target).await?;
    print_remote_context(&target, &RemoteOwner::Server, fingerprint.as_deref());
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
    let owner_name = match &target {
        ConfigTarget::Server => "server",
        ConfigTarget::Device(_) => "daemon",
        ConfigTarget::Local => "cli",
    };
    match config_remote_request(target, args).await? {
        ConfigRequestResult::Success {
            output,
            effect,
            providers,
        } => {
            if json_mode() {
                let mut data = serde_json::json!({
                    "owner": owner_name,
                    "output": redact_urls_in_text(&output),
                });
                if let Some(providers) = providers {
                    data["providers"] = provider_rows_value(&providers);
                }
                if let Some(effect) = effect_name(effect) {
                    data["effect"] = serde_json::Value::String(effect.to_string());
                }
                emit_json(data, serde_json::json!([]));
                return Ok(());
            }
            render_config_success(&output, effect, owner_name);
            Ok(())
        }
        ConfigRequestResult::Rejected(error) => {
            if json_mode() {
                emit_json(
                    serde_json::Value::Null,
                    serde_json::json!([wire_error_json(owner_name, &error)]),
                );
            }
            Err(CoreError::Message(match error.remediation {
                Some(hint) => format!("{} — {hint}", error.message),
                None => error.message,
            }))
        }
    }
}

#[derive(Debug)]
enum ConfigRequestResult {
    Success {
        output: String,
        effect: Option<Effect>,
        providers: Option<Vec<ProviderJson>>,
    },
    Rejected(fleety_protocol::WireError),
}

#[derive(Debug)]
struct ProviderJson {
    name: String,
    kind: String,
    endpoint: String,
    auth: String,
    catalog: String,
    roles: Vec<String>,
    key_present: Option<bool>,
}

fn provider_views_json(
    views: &[fleety_tools::provider_service::ProviderView],
) -> Vec<ProviderJson> {
    views
        .iter()
        .map(|view| ProviderJson {
            name: view.name.clone(),
            kind: view.kind.clone(),
            endpoint: view.endpoint.label().to_string(),
            auth: view.auth.label().to_string(),
            catalog: crate::provider_service::catalog_label(&view.catalog).to_string(),
            roles: view.roles.clone(),
            key_present: view
                .key
                .map(|key| matches!(key, fleety_tools::provider_service::ApiKeyState::Set)),
        })
        .collect()
}

fn provider_rows_value(rows: &[ProviderJson]) -> serde_json::Value {
    serde_json::Value::Array(
        rows.iter()
            .map(|row| {
                let mut value = serde_json::json!({
                    "name": row.name,
                    "type": row.kind,
                    "endpoint": row.endpoint,
                    "auth": row.auth,
                    "catalog": row.catalog,
                    "roles": row.roles,
                });
                if let Some(key_present) = row.key_present {
                    value["key_present"] = serde_json::Value::Bool(key_present);
                }
                value
            })
            .collect(),
    )
}

fn effect_name(effect: Option<Effect>) -> Option<&'static str> {
    match effect {
        Some(Effect::NextConnection) => Some("next_connection"),
        Some(Effect::Restart) => Some("restart"),
        None => None,
    }
}

fn wire_error_json(owner: &str, error: &fleety_protocol::WireError) -> serde_json::Value {
    let mut value = serde_json::json!({
        "owner": owner,
        "kind": error.kind,
        "message": redact_urls_in_text(&error.message),
    });
    if let Some(remediation) = &error.remediation {
        value["remediation"] = serde_json::Value::String(redact_urls_in_text(remediation));
    }
    value
}

fn render_config_success(output: &str, effect: Option<Effect>, owner: &str) {
    let out = output.trim_end();
    if !out.is_empty() {
        println!("{}", terminal_safe_multiline_redacted(out));
    }
    if let Some(effect) = effect {
        let message = match (effect, owner) {
            (Effect::NextConnection, _) => {
                "(applied — takes effect on the next connection)".to_string()
            }
            (Effect::Restart, "server") => {
                "(applied — takes effect after you restart the server (`fleety-server restart`))"
                    .to_string()
            }
            (Effect::Restart, "daemon") => {
                "(applied — takes effect after you restart the daemon (`fleetyd restart`))"
                    .to_string()
            }
            (Effect::Restart, _) => "(applied — restart the owning process)".to_string(),
        };
        println!("{message}");
    }
}

async fn config_remote_request(
    target: ConfigTarget,
    args: &[String],
) -> Result<ConfigRequestResult> {
    let owner = match &target {
        ConfigTarget::Server => RemoteOwner::Server,
        ConfigTarget::Device(id) => RemoteOwner::Daemon(id.clone()),
        ConfigTarget::Local => {
            return Err(CoreError::Message(
                "CLI-owned configuration must be applied locally".to_string(),
            ))
        }
    };
    let (mut tx, mut rx, config_protocol, _) = connect_hello_for_owner(owner).await.map_err(|e| {
        CoreError::Message(format!(
            "could not reach the configuration owner: {} — no local file fallback was used; check the server/daemon connection or select the correct --target",
            e.report().message
        ))
    })?;
    if matches!(target, ConfigTarget::Server)
        && matches!(args.first().map(String::as_str), Some("provider" | "model"))
    {
        let command = match fleety_tools::config::parse_providers(args) {
            Ok(command) => command,
            Err(error) => {
                return Ok(ConfigRequestResult::Rejected(fleety_protocol::WireError {
                    kind: "invalid_provider_command".to_string(),
                    message: error.report().message,
                    remediation: Some("Run the command with --help".to_string()),
                }))
            }
        };
        let is_provider_list = matches!(command, fleety_tools::config::ProviderCmd::ProviderList);
        let snapshot =
            crate::provider_service::load_snapshot(&mut tx, &mut rx, config_protocol).await?;
        let mut outcome = match crate::provider_service::apply_command(
            &snapshot.config,
            &snapshot.key_present,
            command,
        ) {
            Ok(outcome) => outcome,
            Err(issue) => {
                return Ok(ConfigRequestResult::Rejected(fleety_protocol::WireError {
                    kind: issue.kind,
                    message: issue.message,
                    remediation: issue.remediation,
                }))
            }
        };
        let mut providers = None;
        if is_provider_list {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let auth_states = crate::provider_service::load_auth_states(
                &mut tx,
                &mut rx,
                config_protocol,
                &snapshot.config,
                now_secs,
            )
            .await;
            let views = crate::provider_service::provider_views(
                &snapshot.config,
                &snapshot.key_present,
                &auth_states,
                config_protocol,
            );
            providers = Some(provider_views_json(&views));
            outcome.output = crate::provider_service::render_provider_views(&views);
        }
        if outcome.changed {
            if let Err(issue) = crate::provider_service::apply_snapshot(
                &mut tx,
                &mut rx,
                snapshot.revision,
                &outcome.config,
                &outcome.clear_keys,
            )
            .await
            {
                return Ok(ConfigRequestResult::Rejected(fleety_protocol::WireError {
                    kind: issue.kind,
                    message: issue.message,
                    remediation: issue.remediation,
                }));
            }
        }
        return Ok(ConfigRequestResult::Success {
            output: outcome.output,
            effect: outcome.changed.then_some(Effect::NextConnection),
            providers,
        });
    }
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
                Ok(ConfigRequestResult::Success {
                    output,
                    effect,
                    providers: None,
                })
            } else {
                Ok(ConfigRequestResult::Rejected(error.unwrap_or(
                    fleety_protocol::WireError {
                        kind: "rejected".to_string(),
                        message: "configuration request was rejected".to_string(),
                        remediation: None,
                    },
                )))
            }
        }
        Some(ServerMsg::Error { error }) => Ok(ConfigRequestResult::Rejected(error)),
        other => Err(CoreError::Provider(format!(
            "expected a config result, got {other:?}"
        ))),
    }
}

async fn config_list_all() -> Result<()> {
    let list = ["list".to_string()];
    let cli_output =
        fleety_tools::config::run_rendered_scoped(&list, Some(fleety_tools::config::LOCAL_SCOPES))?;
    if quiet_mode() {
        let map = fleety_tools::config::load(&fleety_tools::config::config_path());
        for (key, _, value, _) in
            fleety_tools::config::rows_in_scopes(&map, fleety_tools::config::LOCAL_SCOPES)
        {
            println!(
                "{}={}",
                terminal_safe_text(&key),
                terminal_safe_text(&value)
            );
        }
    } else if !json_mode() {
        println!("CLI settings:");
        println!(
            "{}",
            terminal_safe_multiline_redacted(cli_output.trim_end())
        );
    }

    let mut errors = Vec::new();
    let mut data = serde_json::json!({
        "cli": { "output": cli_output },
    });

    let daemon = config_remote_request(ConfigTarget::Device(device_id()), &list).await;
    collect_config_owner_result("daemon", daemon, &mut data, &mut errors);
    let server = config_remote_request(ConfigTarget::Server, &list).await;
    collect_config_owner_result("server", server, &mut data, &mut errors);

    if json_mode() {
        if let Some(context) = JSON_CONTEXT
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
        {
            let mut context = context;
            context["owner"] = serde_json::Value::String("multiple".to_string());
            context["device_id"] = serde_json::Value::Null;
            set_json_context(context);
        }
        emit_json(data, serde_json::Value::Array(errors.clone()));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        if !json_mode() && !quiet_mode() {
            eprintln!(
                "PARTIAL: available owner data is shown above; one or more owners could not be read"
            );
        }
        Err(CoreError::Message(
            "one or more configuration owners were unavailable; available owner data was preserved"
                .to_string(),
        ))
    }
}

fn collect_config_owner_result(
    owner: &str,
    result: Result<ConfigRequestResult>,
    data: &mut serde_json::Value,
    errors: &mut Vec<serde_json::Value>,
) {
    if !json_mode() && !quiet_mode() {
        println!(
            "\n{} settings:",
            if owner == "daemon" {
                "Daemon"
            } else {
                "Server"
            }
        );
    }
    match result {
        Ok(ConfigRequestResult::Success {
            output,
            effect,
            providers,
        }) => {
            data[owner] = serde_json::json!({
                "output": output,
                "effect": effect_name(effect),
            });
            if let Some(providers) = providers {
                data[owner]["providers"] = provider_rows_value(&providers);
            }
            if !json_mode() {
                if quiet_mode() {
                    let out = output.trim_end();
                    if !out.is_empty() {
                        println!("{}", terminal_safe_multiline_redacted(out));
                    }
                } else {
                    render_config_success(&output, effect, owner);
                }
            }
        }
        Ok(ConfigRequestResult::Rejected(error)) => {
            if !json_mode() {
                eprintln!("UNAVAILABLE: {}", terminal_safe_text(&error.message));
                if let Some(remediation) = &error.remediation {
                    eprintln!("remediation: {}", terminal_safe_text(remediation));
                }
            }
            errors.push(wire_error_json(owner, &error));
        }
        Err(error) => {
            let report = error.report();
            if !json_mode() {
                eprintln!("UNAVAILABLE: {}", terminal_safe_text(&report.message));
                if let Some(remediation) = &report.remediation {
                    eprintln!("remediation: {}", terminal_safe_text(remediation));
                }
            }
            let mut value = serde_json::json!({
                "owner": owner,
                "kind": "unavailable",
                "message": report.message,
            });
            if let Some(remediation) = report.remediation {
                value["remediation"] = serde_json::Value::String(remediation);
            }
            errors.push(value);
        }
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
                    let id = terminal_safe_field(id);
                    let preview = truncate_preview(&terminal_safe_field(preview), 80);
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
    let id = device_id();
    let (mut tx, mut rx, _, _) = connect_hello_for_owner(RemoteOwner::Server).await?;
    record_remote_device_filter(&id);
    send(
        &mut tx,
        &ClientMsg::AuditList {
            device_id: id,
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
                        println!("[{idx:>5}] {when:>8}  {}", terminal_safe_field(kind));
                    } else {
                        println!(
                            "[{idx:>5}] {when:>8}  {:<12} {}",
                            terminal_safe_field(kind),
                            terminal_safe_field(tool)
                        );
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
    let id = device_id();
    let (mut tx, mut rx, _, _) = connect_hello_for_owner(RemoteOwner::Server).await?;
    record_remote_device_filter(&id);
    send(
        &mut tx,
        &ClientMsg::AuditShow {
            device_id: id,
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
    let id = device_id();
    let (mut tx, mut rx, _, _) = connect_hello_for_owner(rollback_owner()).await?;
    send(&mut tx, &ClientMsg::RollbackList { device_id: id }).await?;
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
                    println!(
                        "{}  {:>8}  {}",
                        terminal_safe_field(id),
                        format_relative(now, ts),
                        terminal_safe_field(path)
                    );
                }
            }
        }
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorLevel {
    Pass,
    Warn,
    Fail,
}

impl DoctorLevel {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug)]
struct DoctorCheck {
    name: &'static str,
    level: DoctorLevel,
    detail: String,
    remediation: Option<String>,
}

impl DoctorCheck {
    fn new(
        name: &'static str,
        level: DoctorLevel,
        detail: impl Into<String>,
        remediation: Option<impl Into<String>>,
    ) -> Self {
        let detail = redact_urls_in_text(&detail.into());
        Self {
            name,
            level,
            detail,
            remediation: remediation
                .map(Into::into)
                .map(|value| redact_urls_in_text(&value)),
        }
    }

    fn json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "name": self.name,
            "status": self.level.label(),
            "detail": self.detail,
        });
        if let Some(remediation) = &self.remediation {
            value["remediation"] = serde_json::Value::String(remediation.clone());
        }
        value
    }
}

/// Bounded, read-only diagnostics. Unlike normal command execution this runs
/// before config seeding and migration, and its connection path deliberately
/// skips version convergence and TOFU pinning.
async fn doctor() -> std::process::ExitCode {
    let mut checks = vec![DoctorCheck::new(
        "CLI",
        DoctorLevel::Pass,
        format!("fleety {}", agent_core::VERSION),
        None::<String>,
    )];

    let daemon_status = local_daemon_status();
    let (daemon_level, daemon_remediation) = if daemon_status.starts_with("running") {
        (DoctorLevel::Pass, None)
    } else if daemon_status.starts_with("installed") {
        (DoctorLevel::Warn, Some("fleety daemon start"))
    } else {
        (DoctorLevel::Warn, Some("fleety daemon install"))
    };
    checks.push(DoctorCheck::new(
        "Daemon installation",
        daemon_level,
        daemon_status,
        daemon_remediation,
    ));

    match resolve_target() {
        Ok(target) => {
            record_remote_context(&target, &RemoteOwner::Server, None);
            checks.push(DoctorCheck::new(
                "Profile",
                DoctorLevel::Pass,
                format!(
                    "{} -> {}",
                    profile_label(&target),
                    redact_endpoint(&target.url)
                ),
                None::<String>,
            ));
            let remote = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                doctor_remote(&target, &mut checks),
            )
            .await;
            match remote {
                Ok(Ok(())) => {}
                Ok(Err(error)) => mark_server_failed(&mut checks, error.report().message),
                Err(_) => mark_server_failed(
                    &mut checks,
                    "diagnostic timed out after 5 seconds".to_string(),
                ),
            }
        }
        Err(error) => {
            checks.push(DoctorCheck::new(
                "Profile",
                DoctorLevel::Fail,
                error.report().message,
                Some("fleety connection list"),
            ));
            checks.push(DoctorCheck::new(
                "Server",
                DoctorLevel::Fail,
                "not checked because profile resolution failed",
                Some("fleety connection list"),
            ));
        }
    }

    add_unchecked_remote_checks(&mut checks);

    if !checks.iter().any(|check| check.name == "Daemon connection") {
        checks.push(DoctorCheck::new(
            "Daemon connection",
            DoctorLevel::Warn,
            "not checked because the Server is unavailable",
            Some("fleety daemon start"),
        ));
    }

    const ORDER: [&str; 9] = [
        "CLI",
        "Profile",
        "Server",
        "Config protocol",
        "Providers",
        "OAuth",
        "Active model",
        "Daemon installation",
        "Daemon connection",
    ];
    checks.sort_by_key(|check| {
        ORDER
            .iter()
            .position(|name| *name == check.name)
            .unwrap_or(ORDER.len())
    });

    let failed = checks.iter().any(|check| check.level == DoctorLevel::Fail);
    if json_mode() {
        let data = serde_json::json!({
            "checks": checks.iter().map(DoctorCheck::json).collect::<Vec<_>>(),
        });
        let errors = checks
            .iter()
            .filter(|check| check.level == DoctorLevel::Fail)
            .map(|check| {
                let mut error = serde_json::json!({
                    "owner": if check.name == "Profile" || check.name == "CLI" { "cli" } else { "server" },
                    "kind": "diagnostic",
                    "message": format!("{}: {}", check.name, check.detail),
                });
                if let Some(remediation) = &check.remediation {
                    error["remediation"] = serde_json::Value::String(remediation.clone());
                }
                error
            })
            .collect::<Vec<_>>();
        emit_json(data, serde_json::Value::Array(errors));
    } else {
        for check in &checks {
            println!(
                "{:<5} {}: {}",
                check.level.label(),
                check.name,
                terminal_safe_field(&check.detail)
            );
            if let Some(remediation) = &check.remediation {
                println!("      Fix: {}", terminal_safe_field(remediation));
            }
        }
    }
    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

fn mark_server_failed(checks: &mut Vec<DoctorCheck>, detail: String) {
    let detail = redact_urls_in_text(&detail);
    if let Some(check) = checks.iter_mut().find(|check| check.name == "Server") {
        check.level = DoctorLevel::Fail;
        check.detail = detail;
        check.remediation = Some("fleety connection show".to_string());
    } else {
        checks.push(DoctorCheck::new(
            "Server",
            DoctorLevel::Fail,
            detail,
            Some("fleety connection show"),
        ));
    }
}

fn add_unchecked_remote_checks(checks: &mut Vec<DoctorCheck>) {
    for (name, remediation) in [
        ("Config protocol", "fleety update"),
        ("Providers", "fleety provider list"),
        ("OAuth", "fleety provider status"),
        ("Active model", "fleety model list"),
    ] {
        if !checks.iter().any(|check| check.name == name) {
            checks.push(DoctorCheck::new(
                name,
                DoctorLevel::Warn,
                "not checked because the Server is unavailable",
                Some(remediation),
            ));
        }
    }
}

async fn doctor_remote(target: &connection::Resolved, checks: &mut Vec<DoctorCheck>) -> Result<()> {
    let ws = transport::connect(&target.url, target.token.as_deref()).await?;
    let (mut tx, mut rx) = ws.split();
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    let (server_version, config_protocol, server_fingerprint) = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            config_protocol,
            server_fingerprint,
            ..
        }) => (server_version, config_protocol, server_fingerprint),
        other => return Err(CoreError::Message(hello_failure_message(other.as_ref()))),
    };
    record_remote_context(target, &RemoteOwner::Server, server_fingerprint.as_deref());
    let identity = server_fingerprint.as_deref().unwrap_or("not advertised");
    let version = if server_version.is_empty() {
        "not advertised"
    } else {
        &server_version
    };
    checks.push(DoctorCheck::new(
        "Server",
        DoctorLevel::Pass,
        format!("connected; version {version}; identity {identity}"),
        None::<String>,
    ));
    checks.push(DoctorCheck::new(
        "Config protocol",
        if config_protocol >= CONFIG_PROTOCOL_VERSION {
            DoctorLevel::Pass
        } else {
            DoctorLevel::Warn
        },
        format!("server {config_protocol}, client {CONFIG_PROTOCOL_VERSION}"),
        (config_protocol < CONFIG_PROTOCOL_VERSION).then_some("fleety update"),
    ));

    if config_protocol < 5 {
        checks.push(DoctorCheck::new(
            "Providers",
            DoctorLevel::Warn,
            "not inspectable until the Server supports write-only Provider snapshots",
            Some("fleety update"),
        ));
        checks.push(DoctorCheck::new(
            "OAuth",
            DoctorLevel::Warn,
            "not inspectable until the Server supports write-only Provider snapshots",
            Some("fleety update"),
        ));
        checks.push(DoctorCheck::new(
            "Active model",
            DoctorLevel::Warn,
            "not inspectable until the Server supports write-only Provider snapshots",
            Some("fleety update"),
        ));
    } else {
        inspect_server_configuration(&mut tx, &mut rx, checks, config_protocol).await?;
    }

    if config_protocol == 0 {
        checks.push(DoctorCheck::new(
            "Daemon connection",
            DoctorLevel::Warn,
            "not inspectable on this Server version",
            Some("fleety daemon start"),
        ));
    } else {
        send(
            &mut tx,
            &ClientMsg::ConfigSnapshot {
                target: ConfigTarget::Device(device_id()),
            },
        )
        .await?;
        match recv(&mut rx).await? {
            Some(ServerMsg::ConfigSnapshotResult { .. }) => checks.push(DoctorCheck::new(
                "Daemon connection",
                DoctorLevel::Pass,
                "connected through the selected Server",
                None::<String>,
            )),
            Some(ServerMsg::Error { error }) => checks.push(DoctorCheck::new(
                "Daemon connection",
                DoctorLevel::Warn,
                error.message,
                Some(
                    error
                        .remediation
                        .unwrap_or_else(|| "fleety daemon start".to_string()),
                ),
            )),
            Some(ServerMsg::ConfigResult {
                error: Some(error), ..
            }) => {
                checks.push(DoctorCheck::new(
                    "Daemon connection",
                    DoctorLevel::Warn,
                    error.message,
                    Some(
                        error
                            .remediation
                            .unwrap_or_else(|| "fleety daemon start".to_string()),
                    ),
                ));
            }
            other => checks.push(DoctorCheck::new(
                "Daemon connection",
                DoctorLevel::Warn,
                format!(
                    "unexpected diagnostic reply ({})",
                    server_msg_kind_option(other.as_ref())
                ),
                Some("fleety daemon restart"),
            )),
        }
    }
    let _ = tx.close().await;
    Ok(())
}

async fn inspect_server_configuration(
    tx: &mut Tx,
    rx: &mut Rx,
    checks: &mut Vec<DoctorCheck>,
    config_protocol: u32,
) -> Result<()> {
    let config = match crate::provider_service::load_snapshot(tx, rx, config_protocol).await {
        Ok(snapshot) => snapshot.config,
        Err(error) => {
            checks.push(DoctorCheck::new(
                "Providers",
                DoctorLevel::Fail,
                error.report().message,
                Some("fleety config --owner server list"),
            ));
            add_unavailable_provider_checks(checks);
            return Ok(());
        }
    };
    if config.providers.is_empty() {
        checks.push(DoctorCheck::new(
            "Providers",
            DoctorLevel::Warn,
            "no providers configured",
            Some("fleety provider add <name> --type <api|oauth:codex>"),
        ));
    } else {
        checks.push(DoctorCheck::new(
            "Providers",
            DoctorLevel::Pass,
            format!("{} configured", config.providers.len()),
            None::<String>,
        ));
    }

    match config.models.get("main") {
        Some(pool) if !pool.members.is_empty() => {
            let members = pool
                .members
                .iter()
                .map(|member| format!("{}/{}", member.provider, member.model))
                .collect::<Vec<_>>()
                .join(", ");
            checks.push(DoctorCheck::new(
                "Active model",
                DoctorLevel::Pass,
                format!("main -> {members}"),
                None::<String>,
            ));
        }
        _ => checks.push(DoctorCheck::new(
            "Active model",
            DoctorLevel::Warn,
            "main role is not configured",
            Some("fleety model set main --member <provider>/<model>"),
        )),
    }

    let oauth_providers = config
        .providers
        .iter()
        .filter(|(_, provider)| provider.is_oauth())
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if oauth_providers.is_empty() {
        checks.push(DoctorCheck::new(
            "OAuth",
            DoctorLevel::Warn,
            "no OAuth providers configured",
            Some("fleety provider add <name> --type oauth:codex"),
        ));
        return Ok(());
    }

    let mut unavailable = Vec::new();
    for provider in &oauth_providers {
        send(
            tx,
            &ClientMsg::CredentialStatus {
                kind: "codex-oauth".to_string(),
                provider: Some(provider.clone()),
            },
        )
        .await?;
        match recv(rx).await? {
            Some(ServerMsg::CredentialStatusResult {
                present: true,
                expires_at_secs,
                error: None,
                ..
            }) if expires_at_secs.map_or(true, |expiry| expiry > now_secs()) => {}
            Some(ServerMsg::CredentialStatusResult { .. }) => unavailable.push(provider.clone()),
            Some(ServerMsg::Error { .. }) | None => unavailable.push(provider.clone()),
            Some(_) => unavailable.push(provider.clone()),
        }
    }
    if unavailable.is_empty() {
        checks.push(DoctorCheck::new(
            "OAuth",
            DoctorLevel::Pass,
            format!("signed in: {}", oauth_providers.join(", ")),
            None::<String>,
        ));
    } else {
        let provider = unavailable[0].clone();
        checks.push(DoctorCheck::new(
            "OAuth",
            DoctorLevel::Warn,
            format!("not signed in or expired: {}", unavailable.join(", ")),
            Some(format!("fleety provider login {provider}")),
        ));
    }
    Ok(())
}

fn add_unavailable_provider_checks(checks: &mut Vec<DoctorCheck>) {
    checks.push(DoctorCheck::new(
        "OAuth",
        DoctorLevel::Warn,
        "not checked because Provider configuration is unavailable",
        Some("fleety provider status"),
    ));
    checks.push(DoctorCheck::new(
        "Active model",
        DoctorLevel::Warn,
        "not checked because Provider configuration is unavailable",
        Some("fleety model list"),
    ));
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn server_msg_kind_option(message: Option<&ServerMsg>) -> String {
    message
        .map(server_msg_kind)
        .unwrap_or_else(|| "connection_closed".to_string())
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
    let daemon_status = local_daemon_status();
    if !json_mode() {
        println!("fleety (this host)");
        println!("  cli version:    {}", agent_core::VERSION);
        println!("  daemon:         {daemon_status}");
    }
    let (mut tx, mut rx, _, target) = connect_hello_for_owner(RemoteOwner::Server).await?;
    let server_url = redact_endpoint(&target.url);
    if !json_mode() {
        println!();
    }
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
            let extra = extra_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok());
            if json_mode() {
                emit_json(
                    serde_json::json!({
                        "cli": {
                            "version": agent_core::VERSION,
                        },
                        "daemon": {
                            "status": daemon_status,
                        },
                        "server": {
                            "url": server_url,
                            "version": version,
                            "uptime_secs": uptime_secs,
                            "connected_devices": connected_devices,
                            "device_ids": ids,
                            "extra": extra,
                        },
                    }),
                    serde_json::json!([]),
                );
                let _ = tx.close().await;
                return Ok(());
            }
            println!("fleety-server ({server_url})");
            println!("  version:        {}", terminal_safe_field(&version));
            println!("  uptime:         {}", format_uptime(uptime_secs));
            println!("  connected:      {connected_devices} device(s)");
            if !ids.is_empty() {
                println!(
                    "  device ids:     {}",
                    ids.iter()
                        .map(|id| terminal_safe_field(id))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if let Some(value) = extra {
                if let Some(sidecars) = value.get("sidecars").and_then(|s| s.as_object()) {
                    for (name, info) in sidecars {
                        let status = info.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                        let suffix = info
                            .get("path")
                            .and_then(|p| p.as_str())
                            .map(|p| format!(" ({})", terminal_safe_field(p)))
                            .unwrap_or_default();
                        println!(
                            "  {:<14}  {}{suffix}",
                            terminal_safe_field(name),
                            terminal_safe_field(status)
                        );
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
    let id = device_id();
    let (mut tx, mut rx, _, _) = connect_hello_for_owner(rollback_owner()).await?;
    send(
        &mut tx,
        &ClientMsg::RollbackApply {
            device_id: id,
            backup_id: backup_id.clone(),
        },
    )
    .await?;
    match recv(&mut rx).await? {
        Some(ServerMsg::RollbackResult { ok, message, .. }) => {
            if ok {
                println!("✓ {}", terminal_safe_field(&message));
            } else {
                return Err(CoreError::Message(format!(
                    "rollback failed: {}",
                    terminal_safe_field(&message)
                )));
            }
        }
        Some(ServerMsg::Error { error }) => return Err(CoreError::Message(error.message)),
        other => return Err(CoreError::Provider(format!("unexpected reply: {other:?}"))),
    }
    let _ = tx.close().await;
    Ok(())
}

fn rollback_owner() -> RemoteOwner {
    // The Server handler reads and mutates its own workspace/backups directory.
    // `device_id` on the frame is audit attribution, not an execution target.
    RemoteOwner::Server
}

/// Connect to the current server, complete the Hello handshake, and return the
/// split streams plus the server's config-protocol version (from `Welcome`).
/// Used by the interactive config panel to decide the Server region path.
pub(crate) async fn open_panel(
    target: &connection::Resolved,
) -> Result<((Tx, Rx), u32, Option<String>)> {
    let (mut tx, mut rx) = transport::connect(&target.url, target.token.as_deref())
        .await?
        .split();
    send(&mut tx, &hello(target.token.clone(), None)).await?;
    let (config_protocol, server_fingerprint) = match recv(&mut rx).await? {
        Some(ServerMsg::Welcome {
            server_version,
            config_protocol,
            server_fingerprint,
            ..
        }) => {
            maybe_converge_cli(&server_version).await;
            tofu_pin(server_fingerprint.as_deref(), target);
            (config_protocol, server_fingerprint)
        }
        other => return Err(CoreError::Message(hello_failure_message(other.as_ref()))),
    };
    Ok(((tx, rx), config_protocol, server_fingerprint))
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
    use super::{
        expand_long_option_equals, format_relative, format_uptime, profile_recovery_error,
        truncate_preview,
    };

    #[test]
    fn equals_options_expand_once_and_stop_at_the_option_terminator() {
        let args = [
            "fleety",
            "--profile=work",
            "config",
            "--owner=server",
            "set",
            "KEY=value=with=equals",
            "--",
            "--server=literal-data",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();

        assert_eq!(
            expand_long_option_equals(args),
            [
                "fleety",
                "--profile",
                "work",
                "config",
                "--owner",
                "server",
                "set",
                "KEY=value=with=equals",
                "--",
                "--server=literal-data",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn short_server_selector_attached_forms_normalize_like_separated_form() {
        for selector in ["-s=ws://host.test:8787", "-sws://host.test:8787"] {
            assert_eq!(
                expand_long_option_equals(vec!["fleety".into(), selector.into(), "status".into(),]),
                vec!["fleety", "-s", "ws://host.test:8787", "status"]
            );
        }
        assert_eq!(
            expand_long_option_equals(vec![
                "fleety".into(),
                "--".into(),
                "-sws://literal.test".into(),
            ]),
            vec!["fleety", "--", "-sws://literal.test"]
        );
    }

    #[test]
    fn profile_recovery_error_redacts_cause_and_directs_explicit_repair() {
        let notice = profile_recovery_error(
            "prod\u{1b}]52;c;STEAL\u{7}\r\nforged",
            "connect wss://user:pass@new.test/x?token=SECRET#tail failed",
        );
        for forbidden in ["pass", "SECRET", "#tail", "\u{1b}", "\u{7}", "\r", "\n"] {
            assert!(
                !notice.contains(forbidden),
                "leaked {forbidden:?}: {notice}"
            );
        }
        assert!(notice.contains("prod\\u{1b}]52;c;STEAL\\u{7}\\r\\nforged"));
        assert!(notice.contains("wss://new.test/x?token=<redacted>"));
        assert!(notice.contains("--pairing-code <code>"));
        assert!(notice.contains("will not send the stored token or change the URL"));
    }

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
    use futures::{SinkExt, StreamExt};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Mutex;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn reconnect_keeps_local_input_and_can_navigate_to_settings() {
        let mut app = tui::App::new("reconnecting");
        app.input.set_text("draft".into());
        let mut state = workspace::WorkspaceState::new(workspace::Route::Chat);
        state.reduce(workspace::Action::ConnectionLost {
            attempt: 1,
            backoff_ms: 500,
        });

        assert_eq!(
            handle_reconnect_key(
                &mut app,
                &mut state,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            ),
            ReconnectKeyResult::Continue
        );
        assert_eq!(app.input.text(), "draftx");
        let _ = handle_reconnect_key(
            &mut app,
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(app.input.text(), "draftx", "offline Enter retains draft");
        assert!(
            app.messages.is_empty(),
            "offline Enter does not fake a send"
        );

        let _ = handle_reconnect_key(
            &mut app,
            &mut state,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        );
        for character in "settings".chars() {
            let _ = handle_reconnect_key(
                &mut app,
                &mut state,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }
        assert_eq!(
            handle_reconnect_key(
                &mut app,
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            ),
            ReconnectKeyResult::LeaveReconnect
        );
        assert!(matches!(
            state.route,
            workspace::Route::Settings(workspace::SettingsPage::Connection)
        ));
        assert_eq!(app.input.text(), "draftx");
    }

    #[test]
    fn exhausted_reconnect_stays_in_workspace_with_session_intact() {
        let mut app = tui::App::new("reconnecting");
        app.input.set_text("unsent".into());
        let mut state = workspace::WorkspaceState::new(workspace::Route::Chat);

        mark_reconnect_exhausted(&mut app, &mut state);

        assert!(!app.should_quit);
        assert_eq!(app.input.text(), "unsent");
        assert!(matches!(
            state.connection,
            workspace::ConnectionState::Offline { .. }
        ));
        assert!(matches!(
            state.route,
            workspace::Route::Settings(workspace::SettingsPage::Connection)
        ));
        assert!(!state.notices.is_empty());
    }

    #[test]
    fn reconnect_start_expires_transport_bound_approval_only() {
        let mut app = tui::App::new("ready");
        app.input.set_text("unsent".into());
        app.request_approval("old-id".into(), "run_command", "critical", "deploy");
        app.turn_in_flight = true;
        let mut transport = Some(workspace::ChatTransportContext {
            profile: "A".into(),
            endpoint: "ws://a:8787".into(),
            server_identity: Some("server-a".into()),
            server_version: Some("1.0.0".into()),
            provider: Some("codex".into()),
            model: Some("gpt".into()),
        });

        prepare_for_chat_reconnect(&mut app, &mut transport);

        assert!(!app.turn_in_flight);
        assert!(transport.is_none());
        assert!(app.pending_approvals.is_empty());
        assert_eq!(app.input.text(), "unsent");
        assert!(app.status.contains("expired"));
    }

    #[test]
    fn auth_alias_warning_names_real_canonical_commands() {
        for action in ["login", "logout", "status"] {
            let args = vec![
                "fleety".to_string(),
                "auth".to_string(),
                action.to_string(),
                "codex".to_string(),
            ];
            let warning = compatibility_warning(&args).expect("alias warning");
            assert!(
                warning.contains(&format!("fleety provider {action} codex")),
                "{warning}"
            );
            assert!(!warning.contains("provider auth"), "{warning}");
        }
    }

    #[test]
    fn rollback_commands_identify_the_server_that_executes_them() {
        assert!(matches!(rollback_owner(), RemoteOwner::Server));
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    async fn start_chat_reconnect_server(
        identity: &str,
    ) -> (String, tokio::sync::oneshot::Receiver<Vec<ClientMsg>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind chat reconnect server");
        let address = listener.local_addr().expect("chat reconnect address");
        let identity = identity.to_string();
        let (requests_tx, requests_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept chat reconnect");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept chat websocket");
            let hello = websocket
                .next()
                .await
                .expect("hello frame")
                .expect("read hello");
            let hello: ClientMsg =
                serde_json::from_str(hello.to_text().expect("hello text")).expect("parse hello");
            websocket
                .send(Message::Text(
                    serde_json::to_string(&ServerMsg::Welcome {
                        session_id: "session-b".into(),
                        conversation_id: "new-conversation".into(),
                        protocol: PROTOCOL_VERSION,
                        server_version: "2.0.0".into(),
                        audio_input: false,
                        config_protocol: 0,
                        server_fingerprint: Some(identity),
                        loopback_trusted: false,
                        token: None,
                    })
                    .expect("serialize welcome"),
                ))
                .await
                .expect("send welcome");
            let mut requests = vec![hello];
            if let Ok(Some(Ok(frame))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), websocket.next()).await
            {
                if frame.is_text() {
                    requests.push(
                        serde_json::from_str(frame.to_text().expect("request text"))
                            .expect("parse request"),
                    );
                }
            }
            let _ = requests_tx.send(requests);
        });
        (format!("ws://{address}"), requests_rx)
    }

    #[tokio::test]
    async fn chat_reconnect_validates_identity_then_resumes_without_touching_draft() {
        let (url, requests) = start_chat_reconnect_server("server-b").await;
        let target = connection::Resolved {
            url: url.clone(),
            token: Some("token-b".into()),
            source: connection::Source::Profile("B".into()),
        };
        let mut app = tui::App::new("offline");
        app.input.set_text("first\n草稿".into());
        app.input.left();
        let cursor = app.input.cursor_row_col();
        app.attach(WireAttachment {
            mime: "image/png".into(),
            bytes_b64: Some("cG5n".into()),
            url: None,
            name: Some("draft.png".into()),
        });
        app.last_conversation_id = Some("conversation-a".into());
        app.last_seq = 42;

        let (mut tx, _rx, context) = reconnect_chat_once(
            &target,
            Some("server-b"),
            app.last_conversation_id.clone(),
            app.last_seq,
        )
        .await
        .expect("reconnect profile B");

        assert_eq!(context.profile, "B");
        assert_eq!(context.endpoint, url);
        assert_eq!(context.server_identity.as_deref(), Some("server-b"));
        assert_eq!(context.server_version.as_deref(), Some("2.0.0"));
        assert_eq!(app.input.text(), "first\n草稿");
        assert_eq!(app.input.cursor_row_col(), cursor);
        assert_eq!(app.pending_attachments.len(), 1);
        let requests = requests.await.expect("reconnect requests");
        assert!(matches!(requests[0], ClientMsg::Hello { .. }));
        assert!(matches!(
            &requests[1],
            ClientMsg::Resume {
                conversation_id,
                after_seq: 42
            } if conversation_id == "conversation-a"
        ));
        tx.close().await;
    }

    #[tokio::test]
    async fn chat_reconnect_identity_change_refuses_resume() {
        let (url, requests) = start_chat_reconnect_server("impostor").await;
        let target = connection::Resolved {
            url,
            token: None,
            source: connection::Source::Profile("B".into()),
        };
        let mut app = tui::App::new("offline");
        app.last_conversation_id = Some("conversation-a".into());
        app.last_seq = 42;

        let error = match reconnect_chat_once(
            &target,
            Some("server-b"),
            app.last_conversation_id.clone(),
            app.last_seq,
        )
        .await
        {
            Ok(_) => panic!("changed identity must fail closed"),
            Err(error) => error,
        };

        assert!(matches!(error, ChatReconnectError::IdentityChanged));
        let requests = requests.await.expect("reconnect requests");
        assert_eq!(
            requests.len(),
            1,
            "Resume must not precede identity validation"
        );
        assert!(matches!(requests[0], ClientMsg::Hello { .. }));
    }

    #[test]
    fn chat_model_context_prefers_structured_main_role_then_legacy_model() {
        let providers_json = serde_json::json!({
            "providers": { "codex": { "type": "oauth:codex" } },
            "models": {
                "main": {
                    "strategy": "single",
                    "members": [{ "provider": "codex", "model": "gpt-5" }]
                }
            }
        })
        .to_string();
        let entries = vec![fleety_protocol::ConfigEntry {
            key: "FLEETY_MODEL".into(),
            scope: "server".into(),
            value: "legacy-model".into(),
            default: String::new(),
            description: String::new(),
            secret: false,
            is_set: true,
            effect: None,
            choices: Vec::new(),
        }];

        assert_eq!(
            chat_model_context(&entries, &providers_json),
            (Some("codex".into()), Some("gpt-5".into()))
        );
        assert_eq!(
            chat_model_context(&entries, ""),
            (None, Some("legacy-model".into()))
        );
    }

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
    fn invocation_selector_distinguishes_profiles_urls_and_command_arguments() {
        let args = |items: &[&str]| items.iter().map(|item| item.to_string()).collect();

        let (clean, target) =
            take_server_override(args(&["fleety", "status", "--profile", "work"]))
                .expect("profile selector");
        assert_eq!(clean, args(&["fleety", "status"]));
        assert_eq!(target, Target::Named("work".into()));

        let (clean, target) =
            take_server_override(args(&["fleety", "status", "--server", "ws://host:8787"]))
                .expect("legacy URL selector");
        assert_eq!(clean, args(&["fleety", "status"]));
        assert_eq!(target, Target::Url("ws://host:8787".into()));

        let (clean, target) = take_server_override(args(&[
            "fleety",
            "acp",
            "install",
            "--server",
            "ws://editor:8787",
        ]))
        .expect("ACP server argument");
        assert_eq!(
            clean,
            args(&["fleety", "acp", "install", "--server", "ws://editor:8787"])
        );
        assert_eq!(target, Target::Current);

        let (clean, target) = take_server_override(args(&[
            "fleety",
            "ask",
            "--",
            "--server",
            "ws://example.invalid",
        ]))
        .expect("option terminator");
        assert_eq!(
            clean,
            args(&["fleety", "ask", "--", "--server", "ws://example.invalid"])
        );
        assert_eq!(target, Target::Current);
    }

    #[test]
    fn remote_context_escapes_terminal_controls_without_corrupting_unicode() {
        let target = connection::Resolved {
            url: "ws://host/\u{1b}[31m\nnext".into(),
            token: Some("must-never-render".into()),
            source: connection::Source::OverrideProfile("工作\rA".into()),
        };
        let rendered = remote_context(
            &target,
            &RemoteOwner::Daemon("裝置\t1".into()),
            Some("fp\nowner: CLI\u{1b}"),
        );

        assert!(rendered.contains("工作\\rA"), "{rendered}");
        assert!(rendered.contains("裝置\\t1"), "{rendered}");
        assert!(rendered.contains("fp\\nowner: CLI\\u{1b}"), "{rendered}");
        assert!(!rendered.chars().any(char::is_control), "{rendered:?}");
        assert!(!rendered.contains("must-never-render"));
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
    fn pick_line_sanitizes_hostile_service_identity_and_endpoint() {
        let server = DiscoveredServer {
            name: "mini\u{1b}]52;c;owned\u{7}\r\nforged".into(),
            url: "wss://user:PASS@example.test/x?token=SECRET#tail".into(),
            fingerprint: None,
        };
        let line = render_pick_line(0, &server, &[], None);
        assert!(line.contains("mini\\u{1b}]52;c;owned\\u{7}\\r\\nforged"));
        assert!(line.contains("wss://example.test/x?token=<redacted>"));
        for forbidden in ["PASS", "SECRET", "#tail", "\u{1b}", "\u{7}", "\r", "\n"] {
            assert!(!line.contains(forbidden), "leaked {forbidden:?}: {line:?}");
        }
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
        let discovered = || connection::Discovered {
            url: lan.clone(),
            fingerprint: Some("fp-lan".to_string()),
        };
        // A local server on loopback is preferred even when mDNS also finds a
        // LAN advertiser (the same-host box's own outward IP) — loopback is
        // trusted, the LAN IP would demand pairing.
        let r = prefer_loopback_discovery(|| Some(lb.clone()), || Some(discovered()));
        assert_eq!(r.unwrap().url, lb);
        // No local server → fall through to the mDNS advertiser.
        let r = prefer_loopback_discovery(|| None, || Some(discovered()));
        let r = r.unwrap();
        assert_eq!(r.url, lan);
        assert_eq!(r.fingerprint.as_deref(), Some("fp-lan"));
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
    fn tofu_pin_for_override_profile_updates_b_and_not_current_a() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("override-tofu");
        let mut conns = connection::Connections {
            current: Some("a".to_string()),
            ..Default::default()
        };
        conns.profiles.insert(
            "a".to_string(),
            connection::Profile {
                url: "ws://a:8787".to_string(),
                fingerprint: Some("fp-a".to_string()),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "b".to_string(),
            connection::Profile {
                url: "ws://b:8787".to_string(),
                ..Default::default()
            },
        );
        connection::save(&conns).expect("seed profiles");
        let target = connection::Resolved {
            url: "ws://b:8787".to_string(),
            token: None,
            source: connection::Source::OverrideProfile("b".to_string()),
        };

        tofu_pin(Some("fp-b"), &target);

        let after = connection::load().expect("reload profiles");
        assert_eq!(after.current.as_deref(), Some("a"));
        assert_eq!(after.profiles["a"].fingerprint.as_deref(), Some("fp-a"));
        assert_eq!(after.profiles["b"].fingerprint.as_deref(), Some("fp-b"));
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
