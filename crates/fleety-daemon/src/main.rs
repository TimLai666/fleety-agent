//! fleetyd — the Fleety device background service.
//!
//! Connects to the Agent on startup (registering this device) and stays
//! connected across drops and device sleep via an exponential-backoff reconnect
//! loop. Runs as a background OS service (systemd `--user` / launchd / Windows
//! SCM) controlled by the `install`/`start`/`stop`/`restart`/`enable`/`disable`/
//! `status` subcommands; single-instance via a pidfile in service mode. A clean
//! stop (Ctrl+C, SIGTERM, or SCM Stop) shuts down gracefully between frames, and
//! a self-update restarts the service when idle.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backoff;
mod ondevice;
mod poll_updates;
mod provision;
mod service;
mod update;
#[cfg(windows)]
mod winsvc;

use std::path::PathBuf;

use agent_core::{obs, CoreError, Result};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

/// `~/.fleety/fleetyd.token` — the path where fleetyd persists a token it
/// received from the server after a successful pair, so a restart can come
/// straight back without needing `FLEETY_TOKEN` or another pairing code.
fn token_path() -> Option<PathBuf> {
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(base).join(".fleety").join("fleetyd.token"))
}

fn read_saved_token() -> Option<String> {
    let path = token_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn write_saved_token(token: &str) -> Result<()> {
    let path =
        token_path().ok_or_else(|| CoreError::Message("no home dir for token".to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create ~/.fleety: {e}")))?;
    }
    std::fs::write(&path, token)
        .map_err(|e| CoreError::Message(format!("cannot save token: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn clear_saved_token() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

fn main() {
    obs::init();
    // Seed env from ~/.fleety/config.toml before reading env (env still wins;
    // only unset keys are filled). Best-effort.
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    let cmd = std::env::args().nth(1);

    // On Windows, `run-service` is the SCM entry point: it must talk the service
    // control protocol on this thread (not under a tokio runtime), so handle it
    // before building one. The dispatcher blocks until the service stops and runs
    // the daemon on its own runtime (see winsvc).
    #[cfg(windows)]
    if cmd.as_deref() == Some("run-service") {
        if let Err(e) = winsvc::dispatch() {
            tracing::error!(
                %e,
                "windows service dispatcher failed; `run-service` only works when started \
                 by the Service Control Manager (use `fleetyd start` after `fleetyd install`)"
            );
        }
        return;
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(%e, "cannot start tokio runtime");
            return;
        }
    };
    rt.block_on(async_main(cmd));
}

async fn async_main(cmd: Option<String>) {
    // Lifecycle subcommands act on the OS service manager and exit; `run-service`
    // and no subcommand fall through to actually run the daemon.
    match cmd.as_deref() {
        Some("install") => {
            if let Err(e) = service::install() {
                tracing::error!(report = ?e.report(), "install failed");
            }
            // Provision the data-analysis sidecar (best-effort).
            if let Err(e) = provision::ensure_insyra(false).await {
                tracing::warn!(report = ?e.report(), "could not provision fleety-insyra sidecar");
            }
            return;
        }
        Some("uninstall") => {
            if let Err(e) = service::uninstall() {
                tracing::error!(report = ?e.report(), "uninstall failed");
            }
            return;
        }
        Some("start") => return log_verb("start", service::start()),
        Some("stop") => return log_verb("stop", service::stop()),
        Some("restart") => return log_verb("restart", service::restart()),
        Some("enable") => return log_verb("enable", service::enable()),
        Some("disable") => return log_verb("disable", service::disable()),
        Some("status") => return log_verb("status", service::status()),
        Some("update") => {
            match update::update().await {
                Ok(true) => {
                    // We swapped the binary; restart the installed service (best
                    // effort) so it runs the new exe. The manager stop is graceful
                    // (SIGTERM / SCM Stop handled between frames).
                    if let Err(e) = service::restart() {
                        tracing::warn!(
                            report = ?e.report(),
                            "updated, but could not restart the service automatically — \
                             restart fleetyd to apply"
                        );
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::error!(report = ?e.report(), "update failed"),
            }
            // Refresh the data-analysis sidecar alongside fleetyd (best-effort).
            if let Err(e) = provision::ensure_insyra(true).await {
                tracing::warn!(report = ?e.report(), "could not refresh fleety-insyra sidecar");
            }
            return;
        }
        _ => {}
    }

    // Service mode (non-Windows run-service) claims the single-instance pidfile
    // (defense-in-depth on top of the manager); foreground dev runs do not, so a
    // developer can run one alongside an installed service. On Windows the
    // service path goes through winsvc, which claims the pidfile itself.
    let service_mode = cmd.as_deref() == Some("run-service");
    let _pid_guard = if service_mode {
        match fleety_tools::service::acquire("fleetyd") {
            Ok(fleety_tools::service::Acquire::Started(g)) => Some(g),
            Ok(fleety_tools::service::Acquire::AlreadyRunning(pid)) => {
                tracing::error!(pid, "another fleetyd is already running; exiting");
                return;
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(), "pidfile check failed; continuing without it");
                None
            }
        }
    } else {
        None
    };

    tracing::info!(version = agent_core::VERSION, "fleetyd starting");
    // Best-effort background update poller (no-op when the user hasn't set
    // FLEETY_UPDATE_MANIFEST — keeps the existing dev/install posture).
    poll_updates::spawn();
    if let Err(e) = run(None).await {
        tracing::error!(report = ?e.report(), "fleetyd exited with error");
    }
}

/// Log the outcome of a one-shot lifecycle verb (used by the CLI arms).
fn log_verb(verb: &str, res: Result<()>) {
    if let Err(e) = res {
        tracing::error!(report = ?e.report(), "{verb} failed");
    }
}

/// Ensure this device's external dependencies (best-effort, non-blocking).
/// Default subset insyra; add node/python via `FLEETY_DEPS` for device-side
/// skills/MCP. `FLEETY_AUTO_INSTALL_DEPS=0` disables installing.
async fn ensure_dependencies() {
    use fleety_tools::deps;
    let names = deps::selected_dep_names(
        std::env::var("FLEETY_DEPS").ok().as_deref(),
        deps::daemon_default_deps(),
    );
    let chosen: Vec<deps::Dependency> = names
        .iter()
        .filter_map(|n| match n.as_str() {
            "insyra" => Some(deps::insyra_dependency()),
            "node" => Some(deps::node_dependency()),
            "python" => Some(deps::python_dependency()),
            other => {
                tracing::warn!(dep = %other, "unknown dependency in FLEETY_DEPS; ignoring");
                None
            }
        })
        .collect();
    let outcomes = deps::ensure_all(&chosen).await;
    deps::log_outcomes(&outcomes);
}

/// Process-wide pending restart (set by the auto-update poller). The serve loop
/// checks it at each frame boundary — where the daemon is idle — and carries it
/// out via [`fleety_tools::restart`]'s defer-until-idle policy, so a self-update
/// never interrupts a running on-device tool.
fn pending_restart() -> &'static std::sync::Mutex<Option<fleety_tools::restart::PendingRestart>> {
    static P: std::sync::OnceLock<std::sync::Mutex<Option<fleety_tools::restart::PendingRestart>>> =
        std::sync::OnceLock::new();
    P.get_or_init(|| std::sync::Mutex::new(None))
}

/// Record a deferred self-restart (idempotent while one is already pending).
pub(crate) fn request_self_restart(reason: &str) {
    if let Ok(mut g) = pending_restart().lock() {
        if g.is_none() {
            *g = Some(fleety_tools::restart::PendingRestart::new(
                reason,
                false,
                std::time::Instant::now(),
            ));
            tracing::info!(reason, "self-restart requested; will restart when idle");
        }
    }
}

/// At an idle frame boundary, decide whether a pending restart is due. Consumes
/// it and returns true when the daemon should restart now.
fn restart_due_at_idle() -> bool {
    use fleety_tools::restart::{decide, Decision};
    let Ok(mut g) = pending_restart().lock() else {
        return false;
    };
    if let Some(p) = g.as_ref() {
        // We're between frames here, so the daemon is idle.
        if decide(p, true, None, std::time::Instant::now()) == Decision::RestartNow {
            *g = None;
            return true;
        }
    }
    false
}

fn agent_url() -> String {
    if let Ok(url) = std::env::var("FLEETY_AGENT_URL") {
        return url;
    }
    if let Some(url) = discover_via_mdns(std::time::Duration::from_secs(2)) {
        tracing::info!(%url, "discovered fleety server via mDNS");
        return url;
    }
    "ws://127.0.0.1:8787".to_string()
}

/// Browse the LAN for the fleety server. Mirror of fleety-cli's helper —
/// duplicated rather than shared so each daemon stays a small crate with the
/// dep wholly inside its own boundary.
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
    fleety_tools::device::device_id()
}

/// What ended one connected session.
enum Outcome {
    /// A clean shutdown signal (Ctrl+C / service Stop) — exit the process.
    Shutdown,
    /// The link dropped (disconnect/sleep) — the caller should reconnect.
    Disconnected,
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Outer reconnect loop: stay connected across transient drops and device
/// sleep. Exits only on a clean shutdown signal; everything else reconnects
/// with exponential backoff (reset after a successful connect). `shutdown` is an
/// optional external stop (set by the Windows service control handler); a clean
/// stop is also delivered by Ctrl+C and, on Unix, SIGTERM (so `systemctl stop`
/// is graceful).
async fn run(shutdown: Option<tokio::sync::watch::Receiver<bool>>) -> Result<()> {
    // Ensure device dependencies in the background (best-effort, non-blocking).
    tokio::spawn(ensure_dependencies());
    let mut bo = backoff::Backoff::new();
    loop {
        let url = agent_url();
        match connect_once(&url).await {
            Ok(ws) => {
                bo.reset();
                match serve(&url, ws, shutdown.clone()).await {
                    Outcome::Shutdown => return Ok(()),
                    Outcome::Disconnected => {
                        tracing::info!("fleetyd disconnected; will reconnect");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(), "connect failed; will retry");
            }
        }
        let delay =
            backoff::with_jitter(bo.next_base(), backoff::JITTER_FRAC, backoff::jitter_unit());
        tracing::info!(?delay, "waiting before reconnect");
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = wait_stop(shutdown.clone()) => {
                tracing::info!("stop signal received during backoff; shutting down fleetyd");
                return Ok(());
            }
        }
    }
}

/// Resolve when the process should stop cleanly: Ctrl+C on any platform, plus
/// SIGTERM on Unix (service stop) or the external `shutdown` watch on Windows
/// (SCM Stop). Used in every place the daemon waits.
async fn wait_stop(shutdown: Option<tokio::sync::watch::Receiver<bool>>) {
    #[cfg(unix)]
    {
        let _ = &shutdown;
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(t) => {
                        t.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        match shutdown {
            Some(mut rx) => {
                if *rx.borrow() {
                    return;
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    changed = rx.changed() => {
                        let _ = changed;
                    }
                }
            }
            None => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
}

async fn connect_once(url: &str) -> Result<WsStream> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    Ok(ws)
}

/// Run one connected session: send Hello, then serve frames until the link
/// drops or a shutdown signal arrives.
async fn serve(
    url: &str,
    ws: WsStream,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Outcome {
    let (mut tx, mut rx) = ws.split();

    // Token precedence: env override > on-disk persisted token > pairing flow.
    let token = std::env::var("FLEETY_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(read_saved_token);
    let pairing_code = std::env::var("FLEETY_PAIRING_CODE")
        .ok()
        .filter(|s| !s.is_empty());

    let registry = ondevice::build_local_registry(&ondevice::device_root());
    // Advertise the on-device tool set so the agent knows what device_exec can
    // invoke here — without this, the server has to guess (or hardcode).
    let local_tools_json = serde_json::to_string(&registry.specs()).ok();

    let hello = match serde_json::to_string(&ClientMsg::Hello {
        device_id: device_id(),
        protocol: PROTOCOL_VERSION,
        token,
        pairing_code,
        local_tools_json,
        hostname: fleety_tools::device::hostname(),
    }) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!(%e, "could not serialize hello; will reconnect");
            return Outcome::Disconnected;
        }
    };
    if let Err(e) = tx.send(WsMessage::Text(hello)).await {
        tracing::warn!(%e, "send hello failed; will reconnect");
        return Outcome::Disconnected;
    }
    tracing::info!(%url, "connected; holding connection");
    loop {
        // Idle frame boundary: carry out a deferred self-restart (auto-update)
        // here so it never interrupts a tool that's mid-execution.
        if restart_due_at_idle() {
            tracing::info!("applying deferred restart now (idle); restarting service");
            let _ = tx.close().await;
            if let Err(e) = service::restart() {
                tracing::warn!(report = ?e.report(), "could not restart service for update");
            }
            return Outcome::Shutdown;
        }
        let next = tokio::select! {
            frame = rx.next() => frame,
            _ = wait_stop(shutdown.clone()) => {
                tracing::info!("stop signal received; sending Close and shutting down fleetyd");
                let _ = tx.close().await;
                return Outcome::Shutdown;
            }
        };
        let Some(frame) = next else {
            return Outcome::Disconnected;
        };
        let frame = match frame {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(%e, "websocket read failed; will reconnect");
                return Outcome::Disconnected;
            }
        };
        if frame.is_close() {
            return Outcome::Disconnected;
        }
        if !frame.is_text() {
            continue;
        }
        let Ok(text) = frame.to_text() else { continue };
        let Ok(msg) = serde_json::from_str::<ServerMsg>(text) else {
            continue;
        };
        match msg {
            ServerMsg::Welcome {
                session_id, token, ..
            } => {
                if let Some(tok) = token {
                    if let Err(e) = write_saved_token(&tok) {
                        tracing::warn!(report = ?e.report(), "could not persist fleetyd token");
                    } else {
                        tracing::info!("fleetyd token persisted to ~/.fleety/fleetyd.token");
                    }
                }
                tracing::info!(%session_id, "registered with agent");
            }
            ServerMsg::Error { ref error } if error.kind == "unauthenticated" => {
                tracing::warn!(
                    "server rejected our token: {} — clearing saved token so the next \
                     connect can re-pair",
                    error.message
                );
                clear_saved_token();
                return Outcome::Disconnected;
            }
            ServerMsg::RunTool {
                call_id,
                tool,
                args_json,
            } => {
                let args: serde_json::Value =
                    serde_json::from_str(&args_json).unwrap_or_else(|_| serde_json::json!({}));
                tracing::info!(%tool, "running on-device tool");
                let reply = match registry.call(&tool, args).await {
                    Ok(value) => ClientMsg::ToolResult {
                        call_id,
                        result_json: value.to_string(),
                    },
                    Err(e) => {
                        let r = e.report();
                        ClientMsg::ToolError {
                            call_id,
                            error: WireError {
                                kind: r.kind,
                                message: r.message,
                                remediation: r.remediation,
                            },
                        }
                    }
                };
                let out = match serde_json::to_string(&reply) {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::warn!(%e, "could not serialize tool reply; dropping");
                        continue;
                    }
                };
                if let Err(e) = tx.send(WsMessage::Text(out)).await {
                    tracing::warn!(%e, "send tool reply failed; will reconnect");
                    return Outcome::Disconnected;
                }
            }
            _ => {}
        }
    }
}
