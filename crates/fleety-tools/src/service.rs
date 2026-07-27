//! Cross-platform background-service management, shared by fleetyd and
//! fleety-server.
//!
//! Both binaries run as a background OS service so they have no window, survive
//! the launching terminal closing, are single-instance, and can autostart — all
//! delegated to the platform service manager (so we never need `unsafe` to
//! daemonize): **systemd `--user`** on Linux, **launchd LaunchAgent** on macOS,
//! the **Service Control Manager** on Windows.
//!
//! The command/file mapping is pure (`unit_path`, `unit_content`, `verb_argv`)
//! and unit-tested; the thin executor ([`run_verb`]) writes the unit file and
//! shells out. Real install/start against a live manager is environment-specific
//! and verified manually.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use agent_core::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

pub fn current_os() -> Os {
    if cfg!(target_os = "windows") {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Macos
    } else {
        Os::Linux
    }
}

/// What a managed service is: its identifiers, the binary to run, and the args
/// the service mode is launched with (e.g. `["run-service"]`).
#[derive(Debug, Clone)]
pub struct ServiceSpec {
    /// systemd/SCM service name, e.g. `"fleetyd"`.
    pub name: String,
    /// launchd label / reverse-DNS, e.g. `"com.fleety.fleetyd"`.
    pub label: String,
    pub description: String,
    /// Absolute path to the executable.
    pub exec: String,
    /// Arguments the service-mode process is started with.
    pub args: Vec<String>,
}

/// Lifecycle verbs. `enable`/`disable` toggle boot/login autostart; start/stop/
/// restart act now; status reports running + autostart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Install,
    Uninstall,
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
    Status,
}

/// The unit/plist file path for managers that use one (systemd, launchd). `None`
/// for Windows (SCM keeps the definition itself).
pub fn unit_path(os: Os, spec: &ServiceSpec) -> Option<PathBuf> {
    match os {
        Os::Linux => Some(home_join(&format!(
            ".config/systemd/user/{}.service",
            spec.name
        ))),
        Os::Macos => Some(home_join(&format!(
            "Library/LaunchAgents/{}.plist",
            spec.label
        ))),
        Os::Windows => None,
    }
}

/// The unit/plist file content (systemd unit / launchd plist). `None` on Windows.
pub fn unit_content(os: Os, spec: &ServiceSpec) -> Option<String> {
    let argline = exec_line(&spec.exec, &spec.args);
    match os {
        Os::Linux => Some(format!(
            "[Unit]\nDescription={desc}\nAfter=network-online.target\n\n\
             [Service]\nExecStart={argline}\nRestart=on-failure\nRestartSec=2\n\n\
             [Install]\nWantedBy=default.target\n",
            desc = spec.description,
        )),
        Os::Macos => {
            let mut program_args = String::new();
            program_args.push_str(&format!(
                "    <string>{}</string>\n",
                xml_escape(&spec.exec)
            ));
            for a in &spec.args {
                program_args.push_str(&format!("    <string>{}</string>\n", xml_escape(a)));
            }
            Some(format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                 <plist version=\"1.0\">\n<dict>\n  \
                 <key>Label</key><string>{label}</string>\n  \
                 <key>ProgramArguments</key><array>\n{program_args}  </array>\n  \
                 <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n</dict>\n</plist>\n",
                label = xml_escape(&spec.label),
            ))
        }
        Os::Windows => None,
    }
}

/// The command(s) a verb runs, in order, as argv vectors. Pure (no I/O). `domain`
/// is the launchd domain target (`gui/<uid>`), supplied by the executor.
pub fn verb_argv(os: Os, spec: &ServiceSpec, verb: Verb, domain: &str) -> Vec<Vec<String>> {
    let n = &spec.name;
    let label = &spec.label;
    let s = |parts: &[&str]| parts.iter().map(|p| (*p).to_string()).collect::<Vec<_>>();
    match os {
        Os::Linux => match verb {
            // install writes the unit; daemon-reload picks it up.
            Verb::Install => vec![s(&["systemctl", "--user", "daemon-reload"])],
            Verb::Uninstall => vec![s(&["systemctl", "--user", "disable", "--now", n])],
            Verb::Start => vec![s(&["systemctl", "--user", "start", n])],
            Verb::Stop => vec![s(&["systemctl", "--user", "stop", n])],
            Verb::Restart => vec![s(&["systemctl", "--user", "restart", n])],
            Verb::Enable => vec![s(&["systemctl", "--user", "enable", n])],
            Verb::Disable => vec![s(&["systemctl", "--user", "disable", n])],
            Verb::Status => vec![
                s(&["systemctl", "--user", "is-active", n]),
                s(&["systemctl", "--user", "is-enabled", n]),
            ],
        },
        Os::Macos => {
            let target = format!("{domain}/{label}");
            match verb {
                Verb::Install => {
                    // bootout first (best-effort — see run_verb) so re-installing
                    // an already-loaded service reloads cleanly instead of
                    // failing bootstrap with EIO ("service already loaded").
                    vec![
                        s(&["launchctl", "bootout", &target]),
                        s(&["launchctl", "bootstrap", domain]).plus(unit_path(os, spec)),
                    ]
                }
                Verb::Uninstall => vec![s(&["launchctl", "bootout", &target])],
                Verb::Start => vec![s(&["launchctl", "kickstart", &target])],
                Verb::Stop => vec![s(&["launchctl", "bootout", &target])],
                Verb::Restart => vec![s(&["launchctl", "kickstart", "-k", &target])],
                Verb::Enable => vec![s(&["launchctl", "enable", &target])],
                Verb::Disable => vec![s(&["launchctl", "disable", &target])],
                Verb::Status => vec![s(&["launchctl", "print", &target])],
            }
        }
        Os::Windows => {
            // A real SCM service; `run-service` puts the binary into service mode.
            let bin_path = exec_line(&spec.exec, &spec.args);
            match verb {
                Verb::Install => vec![s(&[
                    "sc",
                    "create",
                    n,
                    "binPath=",
                    &bin_path,
                    "start=",
                    "auto",
                    "DisplayName=",
                    &spec.description,
                ])],
                Verb::Uninstall => vec![s(&["sc", "delete", n])],
                Verb::Start => vec![s(&["sc", "start", n])],
                Verb::Stop => vec![s(&["sc", "stop", n])],
                Verb::Restart => vec![s(&["sc", "stop", n]), s(&["sc", "start", n])],
                Verb::Enable => vec![s(&["sc", "config", n, "start=", "auto"])],
                Verb::Disable => vec![s(&["sc", "config", n, "start=", "demand"])],
                Verb::Status => vec![s(&["sc", "query", n])],
            }
        }
    }
}

/// Run a verb against the platform manager: write the unit file for install,
/// then execute its command sequence. Best-effort errors are actionable. The
/// caller owns higher-level policy (admin check, pidfile, etc.).
pub fn run_verb(spec: &ServiceSpec, verb: Verb) -> Result<()> {
    let os = current_os();
    if verb == Verb::Install {
        if let (Some(path), Some(content)) = (unit_path(os, spec), unit_content(os, spec)) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Message(format!("cannot create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(&path, content)
                .map_err(|e| CoreError::Message(format!("cannot write service file: {e}")))?;
        }
    }
    // A Windows restart of the service *we ourselves are* (an in-process
    // self-restart) must not wait for the old process to stop between `sc stop`
    // and `sc start` — that old process is us, and we can't wait for ourselves.
    // External restarts (a separate CLI process) own no such pidfile and wait.
    let self_restart = os == Os::Windows && verb == Verb::Restart && is_self_service(&spec.name);
    let domain = launchd_domain();
    for (i, argv) in verb_argv(os, spec, verb, &domain).into_iter().enumerate() {
        let Some((program, args)) = argv.split_first() else {
            continue;
        };
        // Some leading commands in a sequence are best-effort:
        // - macOS Install runs `[bootout, bootstrap]`; the leading bootout fails
        //   when nothing is loaded yet, and its "not loaded" stderr must not leak,
        //   so only the bootstrap has to succeed.
        // - Windows Restart runs `[sc stop, sc start]`; `sc stop` fails (1062)
        //   when the service is already stopped, but the restart should still
        //   start it — tolerate the stop and let the start be the one that matters.
        let win_restart_stop = os == Os::Windows && verb == Verb::Restart && i == 0;
        let best_effort = (os == Os::Macos && verb == Verb::Install && i == 0) || win_restart_stop;
        let mut cmd = Command::new(program);
        cmd.args(args);
        if best_effort {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
        let status = cmd.status().map_err(|e| {
            CoreError::Message(format!(
                "service command '{program}' failed to run: {e}{}",
                admin_hint(os, verb)
            ))
        })?;
        // Status verbs are queries; a non-zero just means "not active/enabled".
        if !status.success() && verb != Verb::Status && !best_effort {
            return Err(CoreError::Message(format!(
                "service command '{}' exited with failure{}",
                argv.join(" "),
                admin_hint(os, verb)
            )));
        }
        // `sc stop` returns at STOP_PENDING; wait for the old process to actually
        // exit before the following `sc start`, or the SCM rejects the start with
        // "an instance is already running". Best-effort: on timeout we start
        // anyway and let that error surface. Skipped for a self-restart (we are
        // the process being stopped — see `self_restart`).
        if win_restart_stop && !self_restart {
            wait_until_stopped_at(
                &pidfile_path(&spec.name),
                STOP_SETTLE_WAIT,
                STOP_SETTLE_POLL,
            );
        }
    }
    if verb == Verb::Uninstall {
        if let Some(path) = unit_path(os, spec) {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

/// Run the status query and return a short human summary (running/autostart).
/// Best-effort: a manager that isn't present yields a readable line, not a panic.
pub fn status_text(spec: &ServiceSpec) -> String {
    let os = current_os();
    let domain = launchd_domain();
    let mut lines = Vec::new();
    for argv in verb_argv(os, spec, Verb::Status, &domain) {
        let Some((program, args)) = argv.split_first() else {
            continue;
        };
        match Command::new(program).args(args).output() {
            Ok(o) => {
                let out = String::from_utf8_lossy(&o.stdout);
                let trimmed = out.trim();
                if trimmed.is_empty() {
                    lines.push(format!(
                        "{} (exit {})",
                        argv.join(" "),
                        o.status.code().unwrap_or(-1)
                    ));
                } else {
                    lines.push(trimmed.to_string());
                }
            }
            Err(e) => lines.push(format!("{}: {e}", argv.join(" "))),
        }
    }
    if lines.is_empty() {
        "unknown".to_string()
    } else {
        lines.join(" | ")
    }
}

/// Whether the current process is running elevated (administrator). On Windows,
/// probe with `net session`, which enumerates active sessions and is refused
/// without administrator rights: a zero exit means elevated. Any failure to run
/// the probe, or a non-zero exit, is treated as *not* elevated so we fail toward
/// asking the user to elevate rather than proceeding into a half-done SCM change.
/// Non-Windows managers (systemd `--user`, launchd LaunchAgent) run per-user and
/// never need elevation, so this is always `true` there. No `unsafe`, no new dep.
pub fn is_elevated() -> bool {
    if cfg!(target_os = "windows") {
        Command::new("net")
            .arg("session")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    } else {
        true
    }
}

/// Actionable message for a verb that was blocked by the elevation pre-flight.
/// Pure: state-changing verbs get an "administrator" instruction; the query verb
/// (status) needs no elevation and so carries no message (empty string).
pub fn elevation_required_message(verb: Verb) -> String {
    if verb == Verb::Status {
        String::new()
    } else {
        "changing a Windows service needs administrator rights; re-run this command from an \
         elevated (Administrator) terminal"
            .to_string()
    }
}

/// Pre-flight elevation guard for a lifecycle verb. On Windows, a state-changing
/// verb run without elevation returns an error *before* any `sc` is issued, so no
/// partial service state is left behind. Query verbs (status) and non-Windows
/// hosts always pass. Callers (fleety-server, fleetyd) invoke this before handing
/// the verb to the Service Control Manager.
pub fn ensure_elevated_for(verb: Verb) -> Result<()> {
    if verb == Verb::Status || is_elevated() {
        Ok(())
    } else {
        Err(CoreError::Message(elevation_required_message(verb)))
    }
}

/// Windows service install/uninstall needs admin; surface that in errors.
fn admin_hint(os: Os, verb: Verb) -> String {
    if os == Os::Windows && matches!(verb, Verb::Install | Verb::Uninstall) {
        " — installing/removing a Windows service needs administrator rights; run this once in an elevated (Administrator) terminal".to_string()
    } else {
        String::new()
    }
}

/// The launchd domain target `gui/<uid>` for the current user.
fn launchd_domain() -> String {
    let uid = std::env::var("UID").ok().unwrap_or_else(|| {
        // `id -u` is the portable way; fall back to 501 (typical first macOS user).
        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "501".to_string())
    });
    format!("gui/{uid}")
}

/// `exec` plus its args joined for a single ExecStart/binPath line.
fn exec_line(exec: &str, args: &[String]) -> String {
    if args.is_empty() {
        exec.to_string()
    } else {
        format!("{exec} {}", args.join(" "))
    }
}

fn home_join(rel: &str) -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(rel)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---- pidfile single-instance (defense-in-depth on top of the manager) ----

/// The pidfile path for a service, `~/.fleety/<name>.pid`.
pub fn pidfile_path(name: &str) -> PathBuf {
    home_join(&format!(".fleety/{name}.pid"))
}

/// The restart-request marker path for a service, `~/.fleety/<name>.restart-request`
/// (same runtime dir and convention as [`pidfile_path`]). A non-forced external
/// `restart` drops this file so the running service can restart itself once idle.
pub fn restart_request_path(name: &str) -> PathBuf {
    home_join(&format!(".fleety/{name}.restart-request"))
}

/// Parse a pid from a pidfile's contents. `None` if missing or unparsable.
pub fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// Pure single-instance decision: may we start, given the pid a live pidfile
/// holds? We may start only when nothing live owns it.
pub fn may_start(existing_live_pid: Option<u32>) -> bool {
    existing_live_pid.is_none()
}

/// Result of probing whether a process currently owns a PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidState {
    Alive,
    Dead,
    /// The probe failed, timed out, or was denied. This is deliberately not
    /// treated as dead because doing so could hand ownership to two processes.
    Unknown,
}

#[cfg(any(target_os = "windows", test))]
fn classify_probe_exit(code: Option<i32>) -> PidState {
    match code {
        Some(0) => PidState::Alive,
        Some(1) => PidState::Dead,
        _ => PidState::Unknown,
    }
}

#[cfg(target_os = "windows")]
const WINDOWS_NATIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(target_os = "windows")]
const WINDOWS_FALLBACK_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(target_os = "windows")]
fn csv_field(line: &str, wanted: usize) -> Option<String> {
    let mut field = String::new();
    let mut index = 0usize;
    let mut quoted = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                if index == wanted {
                    return Some(field);
                }
                field.clear();
                index += 1;
            }
            _ => field.push(ch),
        }
    }

    (index == wanted && !quoted).then_some(field)
}

#[cfg(target_os = "windows")]
fn tasklist_contains_pid(output: &[u8], pid: u32) -> bool {
    let expected = pid.to_string();
    String::from_utf8_lossy(output)
        .lines()
        .any(|line| csv_field(line.trim(), 1).as_deref() == Some(expected.as_str()))
}

#[cfg(not(target_os = "windows"))]
fn classify_unix_ps_probe(code: Option<i32>, stdout: &[u8], stderr: &[u8], pid: u32) -> PidState {
    let expected = pid.to_string();
    match code {
        Some(0)
            if String::from_utf8_lossy(stdout)
                .lines()
                .any(|line| line.trim() == expected) =>
        {
            PidState::Alive
        }
        Some(1) if stdout.is_empty() && stderr.is_empty() => PidState::Dead,
        _ => PidState::Unknown,
    }
}

/// Probe whether a process with `pid` is currently alive. Windows first uses
/// native `tasklist.exe` CSV output as a language-independent live-process fast
/// path. If that cannot confirm the PID, bounded PowerShell fallback
/// distinguishes a missing process from access denial or another uncertainty.
/// Command startup failures, access denial, unexpected errors, and timeouts
/// remain `Unknown`.
pub fn probe_pid(pid: u32) -> PidState {
    if pid == 0 {
        return PidState::Dead;
    }
    #[cfg(target_os = "windows")]
    {
        let native = Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .and_then(|child| child_output_with_timeout(child, WINDOWS_NATIVE_PROBE_TIMEOUT));
        if let Some(output) = native {
            if output.status.success() {
                return if tasklist_contains_pid(&output.stdout, pid) {
                    PidState::Alive
                } else {
                    PidState::Dead
                };
            }
        }

        // PowerShell and System.Diagnostics expose process IDs as signed
        // 32-bit integers. Values outside that domain cannot name a process.
        if pid > i32::MAX as u32 {
            return PidState::Unknown;
        }
        let script = format!(
            "$ErrorActionPreference='Stop'; try {{ Get-Process -Id {pid} | Out-Null; exit 0 }} catch {{ if ($_.FullyQualifiedErrorId -like 'NoProcessFoundForGivenId*') {{ exit 1 }} else {{ exit 2 }} }}"
        );
        let code = Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .and_then(|child| child_output_with_timeout(child, WINDOWS_FALLBACK_PROBE_TIMEOUT))
            .and_then(|output| output.status.code());
        classify_probe_exit(code)
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Unix process IDs use a signed `pid_t`; values outside that domain
        // cannot identify a live owner.
        if pid > i32::MAX as u32 {
            return PidState::Dead;
        }
        // `kill -0` success proves liveness. A non-zero result can also mean
        // permission denial, so confirm absence through `ps` before declaring
        // the owner dead; unexpected output remains Unknown and fails closed.
        let kill = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match kill {
            Ok(status) if status.success() => PidState::Alive,
            Ok(_) => Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "pid="])
                .output()
                .map(|output| {
                    classify_unix_ps_probe(
                        output.status.code(),
                        &output.stdout,
                        &output.stderr,
                        pid,
                    )
                })
                .unwrap_or(PidState::Unknown),
            Err(_) => PidState::Unknown,
        }
    }
}

/// Compatibility predicate for status displays. An indeterminate probe is
/// conservatively considered potentially alive; ownership decisions use
/// [`probe_pid`] directly and retain the full three-state result.
pub fn pid_alive(pid: u32) -> bool {
    probe_pid(pid) != PidState::Dead
}

#[cfg(target_os = "windows")]
fn child_output_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Option<std::process::Output> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Whether a freshly sampled pidfile owner means "the (re)started service is up":
/// the pidfile holds a pid, it is `alive`, and — for a restart that must cycle
/// the process — it is not the `replacing` pid we are moving away from. Returns
/// the confirmed pid, or `None` if this sample doesn't qualify. Pure.
pub fn sample_is_up(current: Option<u32>, state: PidState, replacing: Option<u32>) -> Option<u32> {
    match current {
        Some(pid) if state == PidState::Alive && replacing != Some(pid) => Some(pid),
        _ => None,
    }
}

/// Block until the `name` service's pidfile has a confirmed live owner, or
/// `timeout` elapses. "Confirmed" means the same qualifying pid is seen on two
/// consecutive polls, so a process that writes the pidfile then immediately dies
/// (e.g. a bad binary that crashes on boot) is not mistaken for a healthy start.
/// `replacing` (the pid a restart is cycling away from) makes the wait ignore the
/// still-present old process until the new one takes over. Returns the confirmed
/// pid, or `None` on timeout. Blocking (sleeps between polls); callers run it from
/// a one-shot CLI command.
pub fn wait_until_running(
    name: &str,
    replacing: Option<u32>,
    timeout: Duration,
    poll: Duration,
) -> Option<u32> {
    wait_until_running_at(&pidfile_path(name), replacing, timeout, poll)
}

/// Path-based core of [`wait_until_running`] (tests point it at an explicit
/// pidfile instead of resolving one from `$HOME`).
fn wait_until_running_at(
    path: &Path,
    replacing: Option<u32>,
    timeout: Duration,
    poll: Duration,
) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    let mut confirmed_once: Option<u32> = None;
    loop {
        let current = read_pid(path);
        let state = current.map(probe_pid).unwrap_or(PidState::Dead);
        match sample_is_up(current, state, replacing) {
            // The same live owner twice running → settled; report it up.
            Some(pid) if confirmed_once == Some(pid) => return Some(pid),
            Some(pid) => confirmed_once = Some(pid),
            None => confirmed_once = None,
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(poll);
    }
}

/// How long a Windows `restart` waits for the service to reach STOPPED (its
/// pidfile owner gone) after `sc stop` before issuing `sc start`. `sc stop`
/// returns at STOP_PENDING, so starting immediately races the SCM ("an instance
/// is already running"); waiting for the old process to exit avoids it.
const STOP_SETTLE_WAIT: Duration = Duration::from_secs(15);
/// Poll granularity while waiting for the old process to exit.
const STOP_SETTLE_POLL: Duration = Duration::from_millis(300);

/// True when the pidfile has no live owner — the service has fully stopped
/// (either the guard removed the file, or its pid is dead). Pure.
pub fn sample_is_stopped(current: Option<u32>, state: PidState) -> bool {
    match current {
        None | Some(0) => true,
        Some(_) => state == PidState::Dead,
    }
}

/// True when *this* process owns `name`'s pidfile — a "restart" of `name` is then
/// this process restarting **itself** (e.g. the running server applying its own
/// auto-update). Such a caller must not wait for the (re)start to complete: it is
/// the process being replaced, so waiting for itself to stop / a new pid to appear
/// would just self-deadlock. External restarts (a separate CLI process) own no
/// such pidfile, so they wait normally.
pub fn is_self_service(name: &str) -> bool {
    read_pid(&pidfile_path(name)) == Some(std::process::id())
}

/// Block until the `name` service's pidfile has no live owner, or `timeout`
/// elapses. Returns `true` once stopped, `false` on timeout. Used between a
/// Windows `sc stop` and `sc start` so the start doesn't race a still-stopping
/// service. Blocking.
fn wait_until_stopped_at(path: &Path, timeout: Duration, poll: Duration) -> bool {
    wait_until_stopped_at_with_probe(path, timeout, poll, probe_pid)
}

fn wait_until_stopped_at_with_probe<F>(
    path: &Path,
    timeout: Duration,
    poll: Duration,
    mut probe: F,
) -> bool
where
    F: FnMut(u32) -> PidState,
{
    let deadline = Instant::now() + timeout;
    loop {
        let current = read_pid(path);
        let state = current.map(&mut probe).unwrap_or(PidState::Dead);
        if sample_is_stopped(current, state) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// Outcome of trying to claim the single-instance pidfile.
#[derive(Debug)]
pub enum Acquire {
    /// We claimed it; hold the guard for the process lifetime (clears on drop).
    Started(PidGuard),
    /// A live instance already owns it (its pid).
    AlreadyRunning(u32),
}

/// Claim the single-instance pidfile for `name`.
pub fn acquire(name: &str) -> Result<Acquire> {
    acquire_at(&pidfile_path(name))
}

/// Claim a pidfile at an explicit path (testable form of [`acquire`]).
pub fn acquire_at(path: &Path) -> Result<Acquire> {
    acquire_at_with_probe(path, probe_pid)
}

fn acquire_at_with_probe<F>(path: &Path, mut probe: F) -> Result<Acquire>
where
    F: FnMut(u32) -> PidState,
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create {}: {e}", parent.display())))?;
    }
    let mut lock_name = path.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock_path = PathBuf::from(lock_name);
    let claim_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            CoreError::Message(format!(
                "cannot open ownership claim lock {}: {error}",
                lock_path.display()
            ))
        })?;
    fs4::FileExt::try_lock(&claim_lock).map_err(|error| {
        CoreError::Message(format!(
            "ownership claim is already locked at {}; refusing concurrent startup: {error}",
            lock_path.display()
        ))
    })?;
    let owner = std::process::id().to_string();
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                use std::io::Write;
                if let Err(error) = file.write_all(owner.as_bytes()) {
                    drop(file);
                    let _ = std::fs::remove_file(path);
                    return Err(CoreError::Message(format!(
                        "cannot write pidfile {}: {error}",
                        path.display()
                    )));
                }
                return Ok(Acquire::Started(PidGuard {
                    path: path.to_path_buf(),
                    owner,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let contents = std::fs::read_to_string(path).map_err(|read_error| {
                    CoreError::Message(format!(
                        "cannot read pid owner {}: {read_error}; refusing to replace its ownership file",
                        path.display()
                    ))
                })?;
                let pid = contents.trim().parse::<u32>().map_err(|parse_error| {
                    CoreError::Message(format!(
                        "cannot parse pid owner {}: {parse_error}; refusing to replace its ownership file",
                        path.display()
                    ))
                })?;
                match probe(pid) {
                    PidState::Alive => return Ok(Acquire::AlreadyRunning(pid)),
                    PidState::Unknown => {
                        return Err(CoreError::Message(format!(
                            "could not verify pid {pid} from {}; refusing to replace its ownership file",
                            path.display()
                        )));
                    }
                    PidState::Dead => {
                        if std::fs::read_to_string(path).ok().as_deref() != Some(contents.as_str())
                        {
                            continue;
                        }
                        match std::fs::remove_file(path) {
                            Ok(()) => continue,
                            Err(remove_error)
                                if remove_error.kind() == std::io::ErrorKind::NotFound =>
                            {
                                continue;
                            }
                            Err(remove_error) => {
                                return Err(CoreError::Message(format!(
                                    "cannot remove stale pid owner {}: {remove_error}",
                                    path.display()
                                )));
                            }
                        }
                    }
                }
            }
            Err(error) => {
                return Err(CoreError::Message(format!(
                    "cannot claim pidfile {}: {error}",
                    path.display()
                )));
            }
        }
    }
}

/// Holds the pidfile for the process lifetime; removes it on drop.
#[derive(Debug)]
pub struct PidGuard {
    path: PathBuf,
    owner: String,
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|owner| owner == self.owner) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessIdentityState {
    Held,
    Available,
}

#[derive(Debug)]
pub struct ProcessIdentityGuard {
    file: std::fs::File,
    path: PathBuf,
    identity: String,
}

impl ProcessIdentityGuard {
    pub fn identity(&self) -> &str {
        &self.identity
    }
}

pub fn probe_process_identity_at(path: &Path) -> Result<ProcessIdentityState> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProcessIdentityState::Available)
        }
        Err(error) => {
            return Err(CoreError::Message(format!(
                "cannot inspect process identity lock {}: {error}",
                path.display()
            )))
        }
    };
    match fs4::FileExt::try_lock(&file) {
        Ok(()) => {
            let _ = fs4::FileExt::unlock(&file);
            Ok(ProcessIdentityState::Available)
        }
        Err(fs4::TryLockError::WouldBlock) => Ok(ProcessIdentityState::Held),
        Err(fs4::TryLockError::Error(error)) => Err(CoreError::Message(format!(
            "cannot verify process identity lock {}: {error}",
            path.display()
        ))),
    }
}

pub fn claim_process_identity_at(path: &Path, identity: &str) -> Result<ProcessIdentityGuard> {
    if identity.is_empty() {
        return Err(CoreError::Message(
            "process identity must not be empty".to_string(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CoreError::Message(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let created = !path.exists();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| {
            CoreError::Message(format!(
                "cannot open process identity lock {}: {error}",
                path.display()
            ))
        })?;
    fs4::FileExt::try_lock(&file).map_err(|error| {
        CoreError::Message(format!(
            "another process holds identity lock {}: {error}",
            path.display()
        ))
    })?;
    use std::io::{Seek, Write};
    file.set_len(0)
        .and_then(|()| file.rewind())
        .and_then(|_| file.write_all(identity.as_bytes()))
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            CoreError::Message(format!(
                "cannot publish process identity lock {}: {error}",
                path.display()
            ))
        })?;
    #[cfg(unix)]
    if created {
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    CoreError::Message(format!(
                        "cannot make process identity directory durable {}: {error}",
                        parent.display()
                    ))
                })?;
        }
    }
    Ok(ProcessIdentityGuard {
        file,
        path: path.to_path_buf(),
        identity: identity.to_string(),
    })
}

fn release_process_identity_with<R, D, U>(
    identity: &str,
    mut read_identity: R,
    mut remove_path: D,
    mut unlock: U,
) where
    R: FnMut() -> std::io::Result<String>,
    D: FnMut() -> std::io::Result<()>,
    U: FnMut() -> std::io::Result<()>,
{
    if read_identity().is_ok_and(|current| current == identity) {
        let _ = remove_path();
    }
    let _ = unlock();
}

impl Drop for ProcessIdentityGuard {
    fn drop(&mut self) {
        release_process_identity_with(
            &self.identity,
            || std::fs::read_to_string(&self.path),
            || std::fs::remove_file(&self.path),
            || fs4::FileExt::unlock(&self.file),
        );
    }
}

/// Small helper to append the unit path (for launchd bootstrap) to an argv.
trait PushPath {
    fn plus(self, path: Option<PathBuf>) -> Vec<String>;
}
impl PushPath for Vec<String> {
    fn plus(mut self, path: Option<PathBuf>) -> Vec<String> {
        if let Some(p) = path {
            self.push(p.to_string_lossy().into_owned());
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_probe_exit_classification_distinguishes_alive_and_dead() {
        assert_eq!(classify_probe_exit(Some(0)), PidState::Alive);
        assert_eq!(classify_probe_exit(Some(1)), PidState::Dead);
    }

    #[test]
    fn access_denied_probe_is_unknown() {
        assert_eq!(classify_probe_exit(Some(2)), PidState::Unknown);
    }

    #[test]
    fn timed_out_or_unstarted_probe_is_unknown() {
        assert_eq!(classify_probe_exit(None), PidState::Unknown);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn child_output_timeout_terminates_a_stuck_probe() {
        let child = Command::new("powershell")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 5"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleeping probe");
        let started = Instant::now();
        assert!(child_output_with_timeout(child, Duration::from_millis(50)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn bounded_fallback_allows_a_two_second_shell_startup() {
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 2; exit 0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn delayed probe");

        let output = child_output_with_timeout(child, WINDOWS_FALLBACK_PROBE_TIMEOUT)
            .expect("two-second startup must fit inside the fallback bound");

        assert!(output.status.success());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn native_tasklist_csv_matching_ignores_image_name_and_localized_no_match_text() {
        let pid = 4242;
        assert!(tasklist_contains_pid(
            br#""weird,""name.exe","4242","Console","1","12,345 K""#,
            pid
        ));
        assert!(!tasklist_contains_pid(
            "INFO: No tasks are running which match the specified criteria.".as_bytes(),
            pid
        ));
        assert!(!tasklist_contains_pid(
            "INFORMATION: Keine Aufgaben entsprechen den angegebenen Kriterien.".as_bytes(),
            pid
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_probe_recognizes_a_live_owner_despite_shell_startup() {
        assert_eq!(probe_pid(std::process::id()), PidState::Alive);
    }

    #[cfg(target_os = "windows")]
    fn definitely_dead_pid() -> u32 {
        let mut child = Command::new("cmd.exe")
            .args(["/C", "exit", "0"])
            .spawn()
            .expect("spawn short-lived process");
        let pid = child.id();
        child.wait().expect("wait for short-lived process");
        pid
    }

    #[cfg(not(target_os = "windows"))]
    fn definitely_dead_pid() -> u32 {
        u32::MAX - 1
    }

    fn spec() -> ServiceSpec {
        ServiceSpec {
            name: "fleetyd".to_string(),
            label: "com.fleety.fleetyd".to_string(),
            description: "Fleety device daemon".to_string(),
            exec: "/opt/fleety/fleetyd".to_string(),
            args: vec!["run-service".to_string()],
        }
    }

    #[test]
    fn linux_unit_references_exec_and_verbs() {
        let s = spec();
        let content = unit_content(Os::Linux, &s).expect("unit");
        assert!(content.contains("ExecStart=/opt/fleety/fleetyd run-service"));
        assert!(content.contains("WantedBy=default.target"));
        assert!(unit_path(Os::Linux, &s)
            .unwrap()
            .to_string_lossy()
            .ends_with("systemd/user/fleetyd.service"));
        assert_eq!(
            verb_argv(Os::Linux, &s, Verb::Start, "gui/0"),
            vec![vec![
                "systemctl".to_string(),
                "--user".to_string(),
                "start".to_string(),
                "fleetyd".to_string()
            ]]
        );
        // status queries both active and enabled.
        assert_eq!(verb_argv(Os::Linux, &s, Verb::Status, "gui/0").len(), 2);
    }

    #[test]
    fn macos_plist_and_verbs() {
        let s = spec();
        let plist = unit_content(Os::Macos, &s).expect("plist");
        assert!(plist.contains("com.fleety.fleetyd"));
        assert!(plist.contains("<string>/opt/fleety/fleetyd</string>"));
        assert!(plist.contains("<string>run-service</string>"));
        assert!(plist.contains("RunAtLoad"));
        let restart = verb_argv(Os::Macos, &s, Verb::Restart, "gui/501");
        assert_eq!(
            restart,
            vec![vec![
                "launchctl".to_string(),
                "kickstart".to_string(),
                "-k".to_string(),
                "gui/501/com.fleety.fleetyd".to_string()
            ]]
        );
        // Install is idempotent: bootout (best-effort) then bootstrap, so a
        // re-run against an already-loaded service reloads instead of failing.
        let install = verb_argv(Os::Macos, &s, Verb::Install, "gui/501");
        assert_eq!(install.len(), 2);
        assert_eq!(install[0][1], "bootout");
        assert_eq!(install[0][2], "gui/501/com.fleety.fleetyd");
        assert_eq!(install[1][1], "bootstrap");
    }

    #[test]
    fn windows_uses_sc_no_unit_file() {
        let s = spec();
        assert!(unit_path(Os::Windows, &s).is_none());
        assert!(unit_content(Os::Windows, &s).is_none());
        let install = verb_argv(Os::Windows, &s, Verb::Install, "");
        let flat = install[0].join(" ");
        assert!(flat.starts_with("sc create fleetyd binPath="));
        assert!(flat.contains("run-service"));
        assert!(flat.contains("start= auto"));
        // enable/disable flip the start type; restart is stop+start.
        assert_eq!(
            verb_argv(Os::Windows, &s, Verb::Enable, "")[0].join(" "),
            "sc config fleetyd start= auto"
        );
        assert_eq!(
            verb_argv(Os::Windows, &s, Verb::Disable, "")[0].join(" "),
            "sc config fleetyd start= demand"
        );
        assert_eq!(verb_argv(Os::Windows, &s, Verb::Restart, "").len(), 2);
    }

    #[test]
    fn xml_escape_handles_specials() {
        assert_eq!(xml_escape("a&b<c>"), "a&amp;b&lt;c&gt;");
    }

    #[test]
    fn admin_hint_only_for_windows_install_uninstall() {
        assert!(admin_hint(Os::Windows, Verb::Install).contains("administrator"));
        assert!(admin_hint(Os::Windows, Verb::Uninstall).contains("administrator"));
        assert!(admin_hint(Os::Windows, Verb::Start).is_empty());
        assert!(admin_hint(Os::Linux, Verb::Install).is_empty());
        assert!(admin_hint(Os::Macos, Verb::Uninstall).is_empty());
    }

    #[test]
    fn elevation_required_message_only_for_state_changing_verbs() {
        // Every state-changing verb yields an actionable "administrator" message.
        for verb in [
            Verb::Install,
            Verb::Uninstall,
            Verb::Start,
            Verb::Stop,
            Verb::Restart,
            Verb::Enable,
            Verb::Disable,
        ] {
            assert!(
                elevation_required_message(verb).contains("administrator"),
                "{verb:?} should mention administrator"
            );
        }
        // A query verb needs no elevation, so it carries no message.
        assert!(elevation_required_message(Verb::Status).is_empty());
    }

    #[test]
    fn status_never_requires_elevation() {
        // Regardless of platform/elevation, a query verb passes the guard.
        assert!(ensure_elevated_for(Verb::Status).is_ok());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_never_requires_elevation() {
        // On systemd/launchd hosts the manager runs per-user; the guard is a no-op.
        assert!(ensure_elevated_for(Verb::Install).is_ok());
        assert!(is_elevated());
    }

    #[test]
    fn may_start_only_when_unowned() {
        assert!(may_start(None));
        assert!(!may_start(Some(1234)));
    }

    #[test]
    fn restart_request_path_sits_beside_the_pidfile() {
        let pid = pidfile_path("fleety-server");
        let req = restart_request_path("fleety-server");
        // Same runtime directory, distinct `.restart-request` suffix.
        assert_eq!(pid.parent(), req.parent());
        assert!(req
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with(".fleety/fleety-server.restart-request"));
    }

    #[test]
    fn read_pid_parses_or_none() {
        let dir = std::env::temp_dir().join(format!("fleety-pidtest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("a.pid");
        std::fs::write(&p, "  4321\n").unwrap();
        assert_eq!(read_pid(&p), Some(4321));
        std::fs::write(&p, "not-a-pid").unwrap();
        assert_eq!(read_pid(&p), None);
        assert_eq!(read_pid(&dir.join("missing.pid")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acquire_started_when_no_owner_and_already_running_when_live() {
        let dir = std::env::temp_dir().join(format!("fleety-acqtest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // No file → Started, and the pidfile now holds our own pid.
        let fresh = dir.join("fresh.pid");
        match acquire_at(&fresh).unwrap() {
            Acquire::Started(_g) => {
                assert_eq!(read_pid(&fresh), Some(std::process::id()));
            }
            Acquire::AlreadyRunning(_) => panic!("expected Started"),
        }

        // A live pid (ourselves) → AlreadyRunning.
        let live = dir.join("live.pid");
        std::fs::write(&live, std::process::id().to_string()).unwrap();
        match acquire_at(&live).unwrap() {
            Acquire::AlreadyRunning(pid) => assert_eq!(pid, std::process::id()),
            Acquire::Started(_) => panic!("expected AlreadyRunning for a live pid"),
        }

        // A dead pid → Started (we take over).
        let dead = dir.join("dead.pid");
        std::fs::write(&dead, definitely_dead_pid().to_string()).unwrap();
        assert!(matches!(acquire_at(&dead).unwrap(), Acquire::Started(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn process_identity_lock_distinguishes_live_owner_from_reused_pid() {
        let dir = std::env::temp_dir().join(format!(
            "fleety-process-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("tempdir");
        let path = dir.join("fleetyd.process-start.lock");

        assert_eq!(
            probe_process_identity_at(&path).expect("missing identity"),
            ProcessIdentityState::Available
        );
        let guard = claim_process_identity_at(&path, "start-a").expect("claim identity");
        assert_eq!(guard.identity(), "start-a");
        assert_eq!(
            probe_process_identity_at(&path).expect("held identity"),
            ProcessIdentityState::Held
        );
        drop(guard);
        assert_eq!(
            probe_process_identity_at(&path).expect("released identity"),
            ProcessIdentityState::Available
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn process_identity_release_removes_our_path_before_unlocking() {
        let events = std::cell::RefCell::new(Vec::new());
        release_process_identity_with(
            "start-a",
            || {
                events.borrow_mut().push("read");
                Ok("start-a".to_string())
            },
            || {
                events.borrow_mut().push("remove");
                Ok(())
            },
            || {
                events.borrow_mut().push("unlock");
                Ok(())
            },
        );

        assert_eq!(*events.borrow(), ["read", "remove", "unlock"]);
    }

    #[test]
    fn acquire_does_not_overwrite_owner_when_probe_is_unknown() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acq-unknown-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("owner.pid");
        std::fs::write(&path, "4242").unwrap();

        let error = acquire_at_with_probe(&path, |_| PidState::Unknown).unwrap_err();

        assert!(error.to_string().contains("could not verify pid 4242"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "4242");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acquire_rejects_malformed_owner_without_replacing_it() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acq-malformed-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("owner.pid");
        std::fs::write(&path, "not-a-pid").unwrap();

        let error = acquire_at(&path).unwrap_err();

        assert!(error.to_string().contains("cannot parse pid owner"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not-a-pid");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acquire_never_takes_over_a_dead_owner_while_claim_is_locked() {
        let dir =
            std::env::temp_dir().join(format!("fleety-acq-locked-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("owner.pid");
        let lock_path = dir.join("owner.pid.lock");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        fs4::FileExt::try_lock(&lock).unwrap();
        let dead = definitely_dead_pid().to_string();
        std::fs::write(&path, &dead).unwrap();

        let error = acquire_at(&path).unwrap_err();

        assert!(error
            .to_string()
            .contains("ownership claim is already locked"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), dead);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_guard_does_not_remove_a_replaced_owner() {
        let dir =
            std::env::temp_dir().join(format!("fleety-guard-owner-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("owner.pid");
        let guard = match acquire_at(&path).unwrap() {
            Acquire::Started(guard) => guard,
            Acquire::AlreadyRunning(pid) => panic!("unexpected live owner {pid}"),
        };
        let replacement = definitely_dead_pid().to_string();
        std::fs::write(&path, &replacement).unwrap();

        drop(guard);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), replacement);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_fallback_distinguishes_visible_denied_pid_from_a_missing_pid() {
        assert_eq!(
            classify_unix_ps_probe(Some(0), b" 4242\n", b"", 4242),
            PidState::Alive
        );
        assert_eq!(
            classify_unix_ps_probe(Some(1), b"", b"", 4242),
            PidState::Dead
        );
        assert_eq!(
            classify_unix_ps_probe(Some(1), b"", b"permission denied", 4242),
            PidState::Unknown
        );
        assert_eq!(
            classify_unix_ps_probe(Some(2), b"", b"", 4242),
            PidState::Unknown
        );
        assert_eq!(
            classify_unix_ps_probe(Some(0), b"", b"", 4242),
            PidState::Unknown
        );
    }

    #[test]
    fn sample_is_up_needs_live_and_changed() {
        // Live owner, nothing to replace → up.
        assert_eq!(sample_is_up(Some(42), PidState::Alive, None), Some(42));
        // Live owner that differs from the one we're replacing → up (new process).
        assert_eq!(sample_is_up(Some(43), PidState::Alive, Some(42)), Some(43));
        // The still-present old process (== replacing) is not "up" yet.
        assert_eq!(sample_is_up(Some(42), PidState::Alive, Some(42)), None);
        // A dead owner, or an empty pidfile, is never up.
        assert_eq!(sample_is_up(Some(42), PidState::Dead, None), None);
        assert_eq!(sample_is_up(Some(42), PidState::Unknown, None), None);
        assert_eq!(sample_is_up(None, PidState::Dead, None), None);
    }

    #[test]
    fn wait_until_running_confirms_a_live_owner_and_times_out_on_dead() {
        let dir = std::env::temp_dir().join(format!("fleety-waittest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // Our own pid is live → confirmed within the timeout.
        let live = dir.join("live.pid");
        std::fs::write(&live, std::process::id().to_string()).unwrap();
        let live_confirmation_timeout = if cfg!(target_os = "windows") {
            // Two bounded probes may both need the PowerShell fallback when
            // native tasklist access is denied. Production lifecycle callers
            // provide 20 seconds; keep this test below that real budget.
            Duration::from_secs(15)
        } else {
            Duration::from_secs(2)
        };
        assert_eq!(
            wait_until_running_at(
                &live,
                None,
                live_confirmation_timeout,
                Duration::from_millis(5)
            ),
            Some(std::process::id())
        );

        // Requiring a change away from our own (live) pid never settles → times out.
        assert_eq!(
            wait_until_running_at(
                &live,
                Some(std::process::id()),
                Duration::from_millis(40),
                Duration::from_millis(5)
            ),
            None
        );

        // A dead owner → times out (never confirmed).
        let dead = dir.join("dead.pid");
        std::fs::write(&dead, definitely_dead_pid().to_string()).unwrap();
        assert_eq!(
            wait_until_running_at(
                &dead,
                None,
                Duration::from_millis(40),
                Duration::from_millis(5)
            ),
            None
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_self_service_false_without_our_own_pidfile() {
        // A service whose pidfile we don't own is not a self-restart (no marker,
        // or someone else's pid). We never write a pidfile for this bogus name, so
        // it must read as "not self" — the guard then lets the restart wait.
        assert!(!is_self_service(
            "definitely-not-a-real-fleety-service-name-xyz"
        ));
    }

    #[test]
    fn sample_is_stopped_requires_confirmed_dead_owner() {
        assert!(sample_is_stopped(None, PidState::Unknown));
        assert!(sample_is_stopped(Some(42), PidState::Dead));
        assert!(sample_is_stopped(Some(0), PidState::Dead));
        assert!(!sample_is_stopped(Some(42), PidState::Alive));
        assert!(!sample_is_stopped(Some(42), PidState::Unknown));
    }

    #[test]
    fn wait_until_stopped_times_out_while_probe_is_unknown() {
        let dir =
            std::env::temp_dir().join(format!("fleety-stop-unknown-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("owner.pid");
        std::fs::write(&path, "4242").unwrap();

        assert!(!wait_until_stopped_at_with_probe(
            &path,
            Duration::from_millis(30),
            Duration::from_millis(5),
            |_| PidState::Unknown,
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_until_stopped_returns_fast_when_gone_and_times_out_when_live() {
        let dir = std::env::temp_dir().join(format!("fleety-stoptest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // Missing pidfile → already stopped.
        let missing = dir.join("missing.pid");
        assert!(wait_until_stopped_at(
            &missing,
            Duration::from_millis(40),
            Duration::from_millis(5)
        ));

        // A dead owner → stopped.
        let dead = dir.join("dead.pid");
        std::fs::write(&dead, definitely_dead_pid().to_string()).unwrap();
        assert!(wait_until_stopped_at(
            &dead,
            Duration::from_millis(40),
            Duration::from_millis(5)
        ));

        // Our own (live) pid → never stops within the window → times out (false).
        let live = dir.join("live.pid");
        std::fs::write(&live, std::process::id().to_string()).unwrap();
        assert!(!wait_until_stopped_at(
            &live,
            Duration::from_millis(40),
            Duration::from_millis(5)
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pid_guard_clears_on_drop() {
        let dir = std::env::temp_dir().join(format!("fleety-guardtest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("g.pid");
        {
            let _g = acquire_at(&p).unwrap();
            assert!(p.exists());
        }
        assert!(!p.exists(), "guard should remove the pidfile on drop");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
