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
mod colocation;
mod ondevice;
mod poll_updates;
mod provision;
mod service;
#[cfg(windows)]
mod winsvc;

use agent_core::{obs, Result};

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};
use fleety_tools::connection::{self, Target};

/// Persist a freshly-minted token onto the current profile in connections.toml
/// (replacing the legacy `~/.fleety/fleetyd.token`), so a restart reconnects
/// without re-pairing. If there is no current profile yet — e.g. a pure-env
/// (`FLEETY_AGENT_URL`) deployment — a `default` profile pointing at `url` is
/// created and made current so the token has a home.
fn persist_token(url: &str, token: &str) -> Result<()> {
    let mut conns = connection::load()?;
    let name = conns
        .current
        .clone()
        .unwrap_or_else(|| "default".to_string());
    if conns.device_id.is_empty() {
        conns.device_id = fleety_tools::device::device_id();
    }
    let profile = conns.profiles.entry(name.clone()).or_default();
    if profile.url.is_empty() {
        profile.url = url.to_string();
    }
    profile.token = Some(token.to_string());
    conns.current = Some(name);
    connection::save(&conns)
}

/// Clear the current profile's token after the server rejects it, so the next
/// connect re-pairs rather than retrying a dead token.
fn clear_current_token() -> Result<()> {
    let mut conns = connection::load()?;
    if let Some(name) = conns.current.clone() {
        if let Some(p) = conns.profiles.get_mut(&name) {
            p.token = None;
        }
        connection::save(&conns)?;
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    obs::init();
    // Seed env from ~/.fleety/config.toml before reading env (env still wins;
    // only unset keys are filled). Best-effort.
    fleety_tools::config::seed_env_from_config(&fleety_tools::config::load(
        &fleety_tools::config::config_path(),
    ));
    // One-time, idempotent migration of the legacy config.json / fleetyd.token
    // into connections.toml (best-effort; a fresh device has nothing to migrate).
    let _ = connection::migrate_from_config_json();
    let cmd = std::env::args().nth(1);

    // `config ...` inspects/edits this host's settings, then exits — no runtime
    // needed. Same command surface as `fleety config`.
    if cmd.as_deref() == Some("config") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        if let Err(e) = fleety_tools::config::run(&args) {
            let report = e.report();
            eprintln!("error: {}", report.message);
            if let Some(hint) = report.remediation {
                eprintln!("hint: {hint}");
            }
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }

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
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start tokio runtime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    rt.block_on(async_main(cmd))
}

/// Forward-only fleet convergence. Bring this host's fleety binaries to the
/// server's exact version when the server is newer; never downgrade. If this
/// device is ahead, only warn (the operator should upgrade the server). Skips
/// silently when no update manifest is configured.
async fn converge_to_server_version(server_version: &str) {
    if server_version.is_empty() || std::env::var("FLEETY_UPDATE_MANIFEST").is_err() {
        return; // older server, or no updater configured on this host
    }
    let me = agent_core::VERSION;
    if fleety_tools::update::is_newer(me, server_version) {
        tracing::warn!(
            device = me,
            server = server_version,
            "this device is newer than the server; upgrade the server so the fleet converges \
             (devices never auto-downgrade)"
        );
        return;
    }
    if !fleety_tools::update::is_newer(server_version, me) {
        return; // already matched
    }
    tracing::info!(
        device = me,
        server = server_version,
        "server is newer; converging this host"
    );

    // The resolution chain lives in fleety_tools::update::converge_to_version:
    // an env {version} template pins directly; otherwise the binary's latest
    // manifest either already matches or names the pinned manifest via its
    // versioned_manifest template. When neither applies, the error names both
    // remedies (publish versioned_manifest, or switch to a {version} template)
    // and this host just stays put — forward-only, never a wrong install.
    let self_updated = match fleety_tools::update::converge_self_to_version(server_version).await {
        Ok(updated) => updated,
        Err(e) => {
            tracing::warn!(report = ?e.report(), "could not self-update fleetyd to the server version");
            false
        }
    };
    // Bring sibling binaries on this host to the same version. fleety-server is a
    // service: a bare `restart` (no --force) asks the running server to restart
    // once it is idle (deferred until no in-flight turn), rather than hard-killing
    // it mid-turn; the fleety CLI just needs its binary swapped.
    for bin in ["fleety", "fleety-server"] {
        let Some(exe) = fleety_tools::update::sibling_exe(bin) else {
            continue;
        };
        match fleety_tools::update::converge_to_version(bin, &exe, server_version).await {
            Ok(true) if bin == "fleety-server" => {
                // Deferred restart: never add --force here so an update never
                // interrupts an in-flight turn before the deferral deadline.
                let _ = std::process::Command::new(&exe).arg("restart").status();
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(report = ?e.report(), bin, "could not update sibling to the server version")
            }
        }
    }
    if self_updated {
        // Restart at the next idle frame so the new fleetyd takes over.
        request_self_restart("converge-to-server-version");
    }
}

async fn async_main(cmd: Option<String>) -> std::process::ExitCode {
    // Lifecycle subcommands act on the OS service manager and exit; `run-service`
    // and no subcommand fall through to actually run the daemon.
    match cmd.as_deref() {
        Some("version") | Some("--version") | Some("-v") | Some("-V") => {
            println!("fleetyd {}", agent_core::VERSION);
            return std::process::ExitCode::SUCCESS;
        }
        Some("install") => {
            if let Err(e) = service::install() {
                return log_verb("install", Err(e));
            }
            // Provision the data-analysis sidecar (best-effort — the daemon
            // works without it, but say so on the console: otherwise the user
            // first learns at an `insyra_exec` failure much later).
            if let Err(e) = provision::ensure_insyra(false).await {
                eprintln!(
                    "note: could not provision the fleety-insyra sidecar ({}); data analysis \
                     (insyra_exec) will be unavailable on this device until `fleetyd update` \
                     succeeds",
                    e.report().message
                );
            }
            return std::process::ExitCode::SUCCESS;
        }
        Some("uninstall") => return log_verb("uninstall", service::uninstall()),
        Some("start") => return log_verb("start", service::start()),
        Some("stop") => return log_verb("stop", service::stop()),
        Some("restart") => return log_verb("restart", service::restart()),
        Some("enable") => return log_verb("enable", service::enable()),
        Some("disable") => return log_verb("disable", service::disable()),
        Some("status") => return log_verb("status", service::status()),
        Some("update") => {
            let mut code = std::process::ExitCode::SUCCESS;
            match fleety_tools::update::self_update().await {
                Ok(true) => {
                    // We swapped the binary; restart the installed service (best
                    // effort) so it runs the new exe. The manager stop is graceful
                    // (SIGTERM / SCM Stop handled between frames).
                    if let Err(e) = service::restart() {
                        eprintln!(
                            "updated, but could not restart the service automatically ({}) — \
                             restart fleetyd to apply",
                            e.report().message
                        );
                    }
                }
                Ok(false) => {}
                Err(e) => code = log_verb("update", Err(e)),
            }
            // Refresh the data-analysis sidecar alongside fleetyd (best-effort).
            if let Err(e) = provision::ensure_insyra(true).await {
                eprintln!(
                    "note: could not refresh the fleety-insyra sidecar ({}); insyra_exec may \
                     stay on the old version or be unavailable",
                    e.report().message
                );
            }
            // Host-wide: bring the sibling fleety binaries on this machine
            // along (gated on a {bin} template; fleety-server restarts
            // deferred). Best-effort — a sibling failure never fails the
            // daemon's own update.
            if let Err(e) = fleety_tools::update::update_siblings_to_latest(
                &fleety_tools::update::host_siblings_of("fleetyd"),
            )
            .await
            {
                eprintln!(
                    "note: sibling updates did not complete: {}",
                    e.report().message
                );
            }
            return code;
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
                return std::process::ExitCode::FAILURE;
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
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

/// Report the outcome of a one-shot lifecycle verb: a clean stderr line (plus
/// hint) on failure, and a non-zero exit code so users and scripts can tell.
fn log_verb(verb: &str, res: Result<()>) -> std::process::ExitCode {
    match res {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            let report = e.report();
            eprintln!("error: {verb} failed: {}", report.message);
            if let Some(hint) = report.remediation {
                eprintln!("hint: {hint}");
            }
            std::process::ExitCode::FAILURE
        }
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

/// Resolve which server (url + token) to connect to, via the shared resolver
/// over connections.toml. `FLEETY_AGENT_URL` stays a persistent unit-file source
/// for the daemon (transient env override in the resolver): env > current
/// profile > mDNS > localhost. The daemon has no per-invocation override — it
/// always follows `current`.
fn resolve_target() -> Result<connection::Resolved> {
    let conns = connection::load()?;
    let env_url = std::env::var("FLEETY_AGENT_URL").ok();
    let env_token = std::env::var("FLEETY_TOKEN").ok();
    connection::resolve(&conns, &Target::Current, env_url, env_token, || {
        discover_via_mdns(std::time::Duration::from_secs(2)).map(|url| {
            tracing::info!(%url, "discovered fleety server via mDNS");
            connection::Discovered {
                url,
                fingerprint: None,
            }
        })
    })
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
    connection::load()
        .map(|c| c.effective_device_id())
        .unwrap_or_else(|_| fleety_tools::device::device_id())
}

/// What ended one connected session.
enum Outcome {
    /// A clean shutdown signal (Ctrl+C / service Stop) — exit the process.
    Shutdown,
    /// The link dropped (disconnect/sleep) — the caller should reconnect.
    Disconnected,
}

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
        // Resolve the server (url + token) via the shared resolver over
        // connections.toml, honoring a persistent FLEETY_AGENT_URL env override.
        match resolve_target() {
            Ok(target) => {
                // WebSocket first, SSE+POST fallback (unless overridden by env) —
                // so a device behind a proxy that blocks the WS upgrade connects.
                match fleety_tools::transport::connect(&target.url, target.token.as_deref()).await {
                    Ok(conn) => {
                        bo.reset();
                        match serve(&target.url, conn, target.token, shutdown.clone()).await {
                            Outcome::Shutdown => return Ok(()),
                            Outcome::Disconnected => {
                                tracing::info!("fleetyd disconnected; will reconnect");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(report = ?e.report(), "connect failed; will retry");
                        // Sticky healing: the profile URL stopped answering — scan
                        // once for an advertiser with the SAME pinned fingerprint
                        // and adopt its new address (persisted; the next loop
                        // iteration resolves to it). Anything else is ignored.
                        if matches!(target.source, fleety_tools::connection::Source::Profile(_)) {
                            if let Some(new_url) =
                                fleety_tools::connection::heal_current_profile(&target.url)
                            {
                                tracing::info!(
                                    %new_url,
                                    "server moved (same identity fingerprint); reconnecting to \
                                     the new address"
                                );
                                bo.reset();
                                continue;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(report = ?e.report(),
                    "cannot resolve a server to connect to (is connections.toml valid?); will retry");
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

/// Run one connected session: send Hello, then serve frames until the link
/// drops or a shutdown signal arrives.
async fn serve(
    url: &str,
    mut conn: fleety_tools::transport::Connection,
    token: Option<String>,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Outcome {
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
    if let Err(e) = conn.send_text(hello).await {
        tracing::warn!(report = ?e.report(), "send hello failed; will reconnect");
        return Outcome::Disconnected;
    }
    tracing::info!(%url, "connected; holding connection");
    // Presence: when opted in (`FLEETY_PRESENCE=on`), periodically report this
    // device's co-location fingerprint so the server can infer its site. Absent
    // when disabled — nothing is computed or sent.
    let mut presence_tick = if colocation::presence_enabled() {
        let mut iv =
            tokio::time::interval(std::time::Duration::from_secs(colocation::interval_secs()));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Some(iv)
    } else {
        None
    };
    loop {
        // Idle frame boundary: carry out a deferred self-restart (auto-update)
        // here so it never interrupts a tool that's mid-execution.
        if restart_due_at_idle() {
            tracing::info!("applying deferred restart now (idle); restarting service");
            conn.close().await;
            if let Err(e) = service::restart() {
                tracing::warn!(report = ?e.report(), "could not restart service for update");
            }
            return Outcome::Shutdown;
        }
        let next = tokio::select! {
            frame = conn.recv_text() => Some(frame),
            _ = async {
                match presence_tick.as_mut() {
                    Some(iv) => { iv.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Presence report tick: send best-effort; a send failure just
                // means we retry next tick (or the link drops and we reconnect).
                if let Some(frame) = colocation::report_frame() {
                    if let Err(e) = conn.send_text(frame).await {
                        tracing::warn!(report = ?e.report(), "could not send co-location report");
                    }
                }
                None
            }
            _ = wait_stop(shutdown.clone()) => {
                tracing::info!("stop signal received; closing and shutting down fleetyd");
                conn.close().await;
                return Outcome::Shutdown;
            }
        };
        // `None` = a presence tick (handled above); loop for the next event.
        let Some(frame) = next else {
            continue;
        };
        // Inner `None` = link closed or went dead (SSE half-open timeout) → reconnect.
        let Some(text) = frame else {
            return Outcome::Disconnected;
        };
        let Ok(msg) = serde_json::from_str::<ServerMsg>(&text) else {
            continue;
        };
        match msg {
            ServerMsg::Welcome {
                session_id,
                token,
                server_version,
                server_fingerprint,
                ..
            } => {
                if let Some(tok) = token {
                    if let Err(e) = persist_token(url, &tok) {
                        tracing::warn!(report = ?e.report(), "could not persist fleetyd token");
                    } else {
                        tracing::info!(
                            "fleetyd token persisted to the current profile in connections.toml"
                        );
                    }
                }
                // Pin (or back-fill) the server's identity fingerprint so an IP
                // change can heal to this exact server; never overwrite a
                // different pin — that is an anomaly worth a warning.
                if let Some(fp) = server_fingerprint.as_deref().filter(|f| !f.is_empty()) {
                    match fleety_tools::connection::pin_current_fingerprint(fp) {
                        Ok(fleety_tools::connection::PinDecision::IdentityChanged) => {
                            tracing::warn!(
                                "the server's identity fingerprint changed since it was pinned; \
                                 keeping the old pin — re-pair this device if the server was \
                                 intentionally rebuilt"
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(report = ?e.report(), "could not pin the server fingerprint");
                        }
                    }
                }
                tracing::info!(%session_id, "registered with agent");
                // Forward-only fleet convergence: match this host to the server's
                // version when the server is newer (so a device that was offline
                // during a `fleety update` catches up on reconnect).
                converge_to_server_version(&server_version).await;
            }
            ServerMsg::Error { ref error } if error.kind == "unauthenticated" => {
                tracing::warn!(
                    "server rejected our token: {} — clearing saved token so the next \
                     connect can re-pair",
                    error.message
                );
                if let Err(e) = clear_current_token() {
                    tracing::warn!(report = ?e.report(), "could not clear the rejected token");
                }
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
                // The tool runs inline: this loop stops reading the socket, so
                // the WebSocket layer stops answering the server's keepalive
                // pings (ws-liveness). A tool that blocks past the liveness
                // deadline gets this connection reclaimed by the server — by
                // then the call has already failed device_exec's per-call
                // timeout; the tool still completes here (side effects intact),
                // the reply send below fails, and we reconnect via the normal
                // backoff path.
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
                if let Err(e) = conn.send_text(out).await {
                    tracing::warn!(report = ?e.report(), "send tool reply failed; will reconnect");
                    return Outcome::Disconnected;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
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
                std::env::temp_dir().join(format!("fleetyd-test-{name}-{}", std::process::id()));
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
    fn persist_and_clear_token_on_current_profile() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("token");

        // With no profile yet (a pure-env deployment), persist_token creates a
        // `default` profile pointing at the connected url and stores the token.
        persist_token("ws://srv:8787", "daemon-tok").expect("persist");
        let conns = connection::load().expect("load");
        assert_eq!(conns.current.as_deref(), Some("default"));
        let p = conns.current_profile().expect("profile");
        assert_eq!(p.url, "ws://srv:8787");
        assert_eq!(p.token.as_deref(), Some("daemon-tok"));

        // Clearing after a rejection drops the token but keeps the profile.
        clear_current_token().expect("clear");
        let conns = connection::load().expect("reload");
        assert!(conns
            .current_profile()
            .and_then(|p| p.token.as_deref())
            .is_none());
    }

    #[test]
    fn resolve_target_prefers_env_then_current_then_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("resolve");
        std::env::set_var("FLEETY_MDNS_DISABLED", "1"); // no LAN probe in the test

        // Nothing configured → the localhost default.
        assert_eq!(
            resolve_target().expect("resolve").url,
            connection::DEFAULT_URL
        );

        // No env + a current profile (a paired device) → its url + token.
        persist_token("ws://srv:8787", "tok").expect("persist");
        let r = resolve_target().expect("resolve");
        assert_eq!(r.url, "ws://srv:8787");
        assert_eq!(r.token.as_deref(), Some("tok"));

        // An old env deployment still connects: FLEETY_AGENT_URL overrides.
        std::env::set_var("FLEETY_AGENT_URL", "ws://env:8787");
        assert_eq!(resolve_target().expect("resolve").url, "ws://env:8787");
    }

    #[test]
    fn device_id_is_stable_and_nonempty() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("device-id");

        let first = device_id();
        assert!(!first.is_empty());
        assert_eq!(device_id(), first);
    }
}
