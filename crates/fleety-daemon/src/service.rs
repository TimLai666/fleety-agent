//! fleetyd background-service control.
//!
//! Delegates to the shared cross-platform [`fleety_tools::service`] module: the
//! verbs (install/uninstall/start/stop/restart/enable/disable/status) map to the
//! platform manager (systemd `--user` / launchd / Windows SCM). This file only
//! builds fleetyd's [`ServiceSpec`] and wires each verb; the command/file
//! mapping and its tests live in the shared module.

use std::time::Duration;

use agent_core::{CoreError, Result};
use fleety_tools::service::{self, ServiceSpec, Verb};

/// How long `start`/`restart` waits for the daemon process to actually come up
/// before reporting the launch failed (a healthy start writes its pidfile within
/// a second or two; the ceiling covers a slow boot).
const STARTUP_WAIT: Duration = Duration::from_secs(20);
/// Poll granularity while waiting for the pidfile owner to settle.
const WAIT_POLL: Duration = Duration::from_millis(400);

/// fleetyd's service definition, pointing at the current executable run in
/// service mode (`run-service`).
pub fn spec() -> Result<ServiceSpec> {
    let exec = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?
        .to_string_lossy()
        .into_owned();
    Ok(ServiceSpec {
        name: "fleetyd".to_string(),
        label: "com.fleety.fleetyd".to_string(),
        description: "Fleety device daemon".to_string(),
        exec,
        args: vec!["run-service".to_string()],
    })
}

pub fn install() -> Result<()> {
    service::ensure_elevated_for(Verb::Install)?;
    service::run_verb(&spec()?, Verb::Install)?;
    // Installed but not yet autostart-enabled; mirror the manager's own model.
    println!("fleetyd service installed. Use `fleetyd enable` for boot autostart and `fleetyd start` to run now.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    service::ensure_elevated_for(Verb::Uninstall)?;
    service::run_verb(&spec()?, Verb::Uninstall)?;
    println!("fleetyd service removed.");
    Ok(())
}

pub fn start() -> Result<()> {
    let spec = spec()?;
    service::ensure_elevated_for(Verb::Start)?;
    service::run_verb(&spec, Verb::Start)?;
    // Confirm the process actually came up — a manager "start" succeeds even when
    // the binary fails to launch, so don't report success on a dead start.
    confirm_up(&spec.name, None, "start")
}

pub fn stop() -> Result<()> {
    service::ensure_elevated_for(Verb::Stop)?;
    service::run_verb(&spec()?, Verb::Stop)
}

pub fn restart() -> Result<()> {
    let spec = spec()?;
    service::ensure_elevated_for(Verb::Restart)?;
    // The live pid we're cycling away from, so we wait for the *new* one to take over.
    let replacing =
        service::read_pid(&service::pidfile_path(&spec.name)).filter(|&p| service::pid_alive(p));
    service::run_verb(&spec, Verb::Restart)?;
    confirm_up(&spec.name, replacing, "restart")
}

/// Wait for a `start`/`restart` to complete — a live pid taking over the pidfile
/// (distinct from `replacing`, for a restart) — within [`STARTUP_WAIT`], and turn
/// a timeout into an actionable error so the command only "succeeds" once fleetyd
/// is genuinely up.
fn confirm_up(name: &str, replacing: Option<u32>, verb: &str) -> Result<()> {
    match service::wait_until_running(name, replacing, STARTUP_WAIT, WAIT_POLL) {
        Some(pid) => {
            println!("fleetyd {verb} complete (pid {pid}).");
            Ok(())
        }
        None => Err(CoreError::Message(format!(
            "fleetyd {verb} did not complete within {}s — no running process claimed the pidfile. \
             It likely failed to launch (e.g. a non-executable binary after an update) or crashed \
             on boot; check `fleetyd status` and the service logs.",
            STARTUP_WAIT.as_secs()
        ))),
    }
}

pub fn enable() -> Result<()> {
    service::ensure_elevated_for(Verb::Enable)?;
    service::run_verb(&spec()?, Verb::Enable)
}

pub fn disable() -> Result<()> {
    service::ensure_elevated_for(Verb::Disable)?;
    service::run_verb(&spec()?, Verb::Disable)
}

pub fn status() -> Result<()> {
    println!("{}", service::status_text(&spec()?));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleetyd_spec_is_well_formed() {
        // current_exe is available in the test runner; the spec must name fleetyd
        // and launch in service mode.
        let s = spec().expect("spec");
        assert_eq!(s.name, "fleetyd");
        assert_eq!(s.label, "com.fleety.fleetyd");
        assert_eq!(s.args, vec!["run-service".to_string()]);
        assert!(!s.exec.is_empty());
    }
}
