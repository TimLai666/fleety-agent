//! fleety-server background-service control.
//!
//! Like fleetyd, the server runs as a background OS service via the shared
//! [`fleety_tools::service`] module. Unlike fleetyd, **install defaults to boot
//! autostart on** (the server should "come back by itself"), and it offers
//! `up`/`down` convenience verbs in the spirit of `docker compose up -d`:
//! `up` = install + enable + start (one command, runs in the background, keeps
//! running after the terminal closes), `down` = stop.
//!
//! The high-level action → low-level verb mapping ([`plan`]) is pure and
//! unit-tested; the executor shells out via the shared module.

use agent_core::{CoreError, Result};
use fleety_tools::service::{self, ServiceSpec, Verb};

/// High-level server lifecycle actions exposed on the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
    Status,
    Up,
    Down,
}

/// fleety-server's service definition (runs in service mode via `run-service`).
pub fn spec() -> Result<ServiceSpec> {
    let exec = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?
        .to_string_lossy()
        .into_owned();
    Ok(ServiceSpec {
        name: "fleety-server".to_string(),
        label: "com.fleety.server".to_string(),
        description: "Fleety Agent server".to_string(),
        exec,
        args: vec!["run-service".to_string()],
    })
}

/// Pure mapping from a high-level action to the ordered manager verbs it runs.
/// `install` enables autostart by default; `up`/`down` are compose-style combos.
pub fn plan(action: Action) -> Vec<Verb> {
    match action {
        Action::Install => vec![Verb::Install, Verb::Enable],
        Action::Up => vec![Verb::Install, Verb::Enable, Verb::Start],
        Action::Down => vec![Verb::Stop],
        Action::Uninstall => vec![Verb::Uninstall],
        Action::Start => vec![Verb::Start],
        Action::Stop => vec![Verb::Stop],
        Action::Restart => vec![Verb::Restart],
        Action::Enable => vec![Verb::Enable],
        Action::Disable => vec![Verb::Disable],
        // Status is a query handled separately (not part of the plan).
        Action::Status => vec![Verb::Status],
    }
}

/// Execute a high-level action against the platform service manager.
pub fn run(action: Action) -> Result<()> {
    let spec = spec()?;
    if action == Action::Status {
        println!("{}", service::status_text(&spec));
        return Ok(());
    }
    let verbs = plan(action);
    // Pre-flight the elevation check once before touching the SCM. Every
    // non-status action is state-changing, so if we're not elevated on Windows we
    // abort here — before any `sc` runs — leaving no half-created service behind.
    if let Some(&first) = verbs.first() {
        service::ensure_elevated_for(first)?;
    }
    for verb in verbs {
        service::run_verb(&spec, verb)?;
    }
    match action {
        Action::Install => println!(
            "fleety-server installed and set to autostart at boot. Use `fleety-server start` to \
             run now, or `fleety-server up` to install+enable+start in one step."
        ),
        Action::Up => println!("fleety-server is up (installed, autostart on, running in the background)."),
        Action::Down => println!("fleety-server is down (stopped). It will still autostart at next boot unless you `disable` it."),
        Action::Uninstall => println!("fleety-server service removed."),
        _ => {}
    }
    Ok(())
}

/// What a `restart` invocation should do. Pure decision; [`restart`] does the I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartMode {
    /// Restart immediately through the service manager (forced, or nothing running
    /// to wait for).
    Immediate,
    /// Ask the running server to restart itself once idle (drop the marker).
    RequestDeferred,
}

/// Decide how a `restart` should proceed given `--force` and whether a live server
/// owns the pidfile. `--force` always restarts now; otherwise a live server is
/// asked to defer until idle, and with no running server there is no in-flight
/// work to wait for so we restart directly.
pub fn restart_mode(force: bool, server_alive: bool) -> RestartMode {
    if force || !server_alive {
        RestartMode::Immediate
    } else {
        RestartMode::RequestDeferred
    }
}

/// Carry out `fleety-server restart`. `--force` restarts immediately through the
/// manager (the old behavior); otherwise, against a *live* server, drop a
/// restart-request marker so that server restarts itself once idle instead of
/// being hard-killed mid-turn. With no server running, restart directly. If the
/// marker can't be written we fall back to an immediate manager restart (never
/// silently skip the restart).
pub fn restart(force: bool) -> Result<()> {
    let spec = spec()?;
    let pid = service::read_pid(&service::pidfile_path(&spec.name));
    let server_alive = pid.map(service::pid_alive).unwrap_or(false);
    match restart_mode(force, server_alive) {
        RestartMode::Immediate => {
            // A forced/immediate restart drives the SCM directly (sc stop + start),
            // so pre-flight the same elevation guard the other state-changing verbs
            // use — abort before touching the SCM when not elevated on Windows.
            service::ensure_elevated_for(Verb::Restart)?;
            service::run_verb(&spec, Verb::Restart)
        }
        RestartMode::RequestDeferred => match crate::restart_watch::request_restart(&spec.name) {
            Ok(()) => {
                println!(
                    "restart requested; the running server will restart once it is idle (no \
                     in-flight turn), or after the deferral deadline. Pass --force to restart now."
                );
                Ok(())
            }
            Err(e) => {
                eprintln!(
                    "could not record a deferred restart request ({}); restarting now instead.",
                    e.report().message
                );
                service::run_verb(&spec, Verb::Restart)
            }
        },
    }
}

/// Parse a CLI subcommand into an [`Action`]. Returns `None` for non-service
/// commands (so the caller can fall through to running the server).
pub fn action_from_arg(arg: &str) -> Option<Action> {
    Some(match arg {
        "install" => Action::Install,
        "uninstall" => Action::Uninstall,
        "start" => Action::Start,
        "stop" => Action::Stop,
        "restart" => Action::Restart,
        "enable" => Action::Enable,
        "disable" => Action::Disable,
        "status" => Action::Status,
        "up" => Action::Up,
        "down" => Action::Down,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_enables_autostart_by_default() {
        assert_eq!(plan(Action::Install), vec![Verb::Install, Verb::Enable]);
    }

    #[test]
    fn up_is_install_enable_start() {
        assert_eq!(
            plan(Action::Up),
            vec![Verb::Install, Verb::Enable, Verb::Start]
        );
    }

    #[test]
    fn down_is_just_stop() {
        assert_eq!(plan(Action::Down), vec![Verb::Stop]);
    }

    #[test]
    fn restart_mode_defers_only_for_a_live_unforced_restart() {
        // Forced → immediate, regardless of whether a server is up.
        assert_eq!(restart_mode(true, true), RestartMode::Immediate);
        assert_eq!(restart_mode(true, false), RestartMode::Immediate);
        // Unforced against a live server → ask it to defer until idle.
        assert_eq!(restart_mode(false, true), RestartMode::RequestDeferred);
        // Unforced with nothing running → nothing to wait for, restart directly.
        assert_eq!(restart_mode(false, false), RestartMode::Immediate);
    }

    #[test]
    fn arg_parsing_maps_known_verbs_only() {
        assert_eq!(action_from_arg("up"), Some(Action::Up));
        assert_eq!(action_from_arg("status"), Some(Action::Status));
        assert_eq!(action_from_arg("run-service"), None);
        assert_eq!(action_from_arg("bogus"), None);
    }

    #[test]
    fn server_spec_is_well_formed() {
        let s = spec().expect("spec");
        assert_eq!(s.name, "fleety-server");
        assert_eq!(s.args, vec!["run-service".to_string()]);
        assert!(!s.exec.is_empty());
    }
}
