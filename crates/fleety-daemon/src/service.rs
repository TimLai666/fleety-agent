//! fleetyd background-service control.
//!
//! Delegates to the shared cross-platform [`fleety_tools::service`] module: the
//! verbs (install/uninstall/start/stop/restart/enable/disable/status) map to the
//! platform manager (systemd `--user` / launchd / Windows SCM). This file only
//! builds fleetyd's [`ServiceSpec`] and wires each verb; the command/file
//! mapping and its tests live in the shared module.

use agent_core::{CoreError, Result};
use fleety_tools::service::{self, ServiceSpec, Verb};

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
    service::ensure_elevated_for(Verb::Start)?;
    service::run_verb(&spec()?, Verb::Start)
}

pub fn stop() -> Result<()> {
    service::ensure_elevated_for(Verb::Stop)?;
    service::run_verb(&spec()?, Verb::Stop)
}

pub fn restart() -> Result<()> {
    service::ensure_elevated_for(Verb::Restart)?;
    service::run_verb(&spec()?, Verb::Restart)
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
