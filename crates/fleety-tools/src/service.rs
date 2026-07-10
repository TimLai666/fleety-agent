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
use std::process::Command;

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
                    vec![s(&["launchctl", "bootstrap", domain]).plus(unit_path(os, spec))]
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
    let domain = launchd_domain();
    for argv in verb_argv(os, spec, verb, &domain) {
        let Some((program, args)) = argv.split_first() else {
            continue;
        };
        let status = Command::new(program).args(args).status().map_err(|e| {
            CoreError::Message(format!(
                "service command '{program}' failed to run: {e}{}",
                admin_hint(os, verb)
            ))
        })?;
        // Status verbs are queries; a non-zero just means "not active/enabled".
        if !status.success() && verb != Verb::Status {
            return Err(CoreError::Message(format!(
                "service command '{}' exited with failure{}",
                argv.join(" "),
                admin_hint(os, verb)
            )));
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

/// Whether a process with `pid` is currently alive. Uses the platform's own
/// query (no `unsafe`): `kill -0` on Unix, `tasklist` on Windows. A failure to
/// determine is treated as "not alive" so we never wrongly refuse to start.
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if cfg!(target_os = "windows") {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()
            .map(|o| {
                let out = String::from_utf8_lossy(&o.stdout);
                out.contains(&pid.to_string())
            })
            .unwrap_or(false)
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
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
    if let Some(pid) = read_pid(path) {
        if pid_alive(pid) {
            return Ok(Acquire::AlreadyRunning(pid));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create {}: {e}", parent.display())))?;
    }
    std::fs::write(path, std::process::id().to_string())
        .map_err(|e| CoreError::Message(format!("cannot write pidfile {}: {e}", path.display())))?;
    Ok(Acquire::Started(PidGuard {
        path: path.to_path_buf(),
    }))
}

/// Holds the pidfile for the process lifetime; removes it on drop.
#[derive(Debug)]
pub struct PidGuard {
    path: PathBuf,
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        std::fs::write(&dead, "4294967294").unwrap(); // u32::MAX-1, not a real pid
        assert!(matches!(acquire_at(&dead).unwrap(), Acquire::Started(_)));

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
