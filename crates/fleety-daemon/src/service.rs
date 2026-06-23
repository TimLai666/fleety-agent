//! OS autostart service definitions for fleetyd.
//!
//! `install` writes the platform service file (systemd user unit / launchd
//! LaunchAgent) and prints the one command to enable it; Windows uses Task
//! Scheduler (no file). The definition generation is pure and unit-tested; the
//! privileged enable step is left to the user-run command for safety.

use std::path::PathBuf;

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

pub struct ServiceDef {
    pub manager: &'static str,
    /// `(path, content)` of a unit/plist file to write, if the manager uses one.
    pub file: Option<(String, String)>,
    /// Command the user runs to enable autostart.
    pub enable: String,
    /// Command the user runs to disable autostart.
    pub disable: String,
}

/// Build the autostart definition for `os`, pointing at the `exec` binary.
pub fn service_def(os: Os, exec: &str) -> ServiceDef {
    match os {
        Os::Linux => ServiceDef {
            manager: "systemd (user)",
            file: Some((
                "~/.config/systemd/user/fleetyd.service".to_string(),
                format!(
                    "[Unit]\nDescription=Fleety device daemon\nAfter=network-online.target\n\n\
                     [Service]\nExecStart={exec}\nRestart=on-failure\n\n\
                     [Install]\nWantedBy=default.target\n"
                ),
            )),
            enable: "systemctl --user daemon-reload && systemctl --user enable --now fleetyd"
                .to_string(),
            disable: "systemctl --user disable --now fleetyd".to_string(),
        },
        Os::Macos => ServiceDef {
            manager: "launchd (LaunchAgent)",
            file: Some((
                "~/Library/LaunchAgents/com.fleety.fleetyd.plist".to_string(),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\">\n<dict>\n  \
                     <key>Label</key><string>com.fleety.fleetyd</string>\n  \
                     <key>ProgramArguments</key><array><string>{exec}</string></array>\n  \
                     <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n</dict>\n</plist>\n"
                ),
            )),
            enable: "launchctl load ~/Library/LaunchAgents/com.fleety.fleetyd.plist".to_string(),
            disable: "launchctl unload ~/Library/LaunchAgents/com.fleety.fleetyd.plist".to_string(),
        },
        Os::Windows => ServiceDef {
            manager: "Windows Task Scheduler",
            file: None,
            enable: format!("schtasks /create /tn Fleetyd /tr \"{exec}\" /sc onlogon /rl highest"),
            disable: "schtasks /delete /tn Fleetyd /f".to_string(),
        },
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Write the service file (if any) and print the enable command.
pub fn install() -> Result<()> {
    let exec = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?
        .to_string_lossy()
        .to_string();
    let def = service_def(current_os(), &exec);
    if let Some((path, content)) = &def.file {
        let target = expand_home(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::Message(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        std::fs::write(&target, content)
            .map_err(|e| CoreError::Message(format!("cannot write service file: {e}")))?;
        println!("Wrote {} service file: {}", def.manager, target.display());
    } else {
        println!("{}: no file needed.", def.manager);
    }
    println!("To enable autostart, run:\n  {}", def.enable);
    Ok(())
}

/// Remove the service file (if any) and print the disable command.
pub fn uninstall() -> Result<()> {
    let def = service_def(current_os(), "");
    if let Some((path, _)) = &def.file {
        let target = expand_home(path);
        if target.exists() {
            std::fs::remove_file(&target)
                .map_err(|e| CoreError::Message(format!("cannot remove service file: {e}")))?;
            println!("Removed {}", target.display());
        }
    }
    println!("To disable autostart, run:\n  {}", def.disable);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defs_reference_the_exec_path() {
        let exec = "/opt/fleety/fleetyd";
        let linux = service_def(Os::Linux, exec);
        let (_, content) = linux.file.as_ref().expect("linux file");
        assert!(content.contains("ExecStart=/opt/fleety/fleetyd"));
        assert!(linux.enable.contains("systemctl"));

        let mac = service_def(Os::Macos, exec);
        let (_, plist) = mac.file.as_ref().expect("mac file");
        assert!(plist.contains(exec));
        assert!(plist.contains("com.fleety.fleetyd"));

        let win = service_def(Os::Windows, "C:\\fleety\\fleetyd.exe");
        assert!(win.file.is_none());
        assert!(win.enable.contains("schtasks"));
        assert!(win.enable.contains("fleetyd.exe"));
    }

    #[test]
    fn expand_home_uses_home_for_tilde_paths_only() {
        let old_home = std::env::var("HOME").ok();
        let old_profile = std::env::var("USERPROFILE").ok();
        let temp = std::env::temp_dir().join(format!("fleetyd-service-{}", std::process::id()));

        std::env::set_var("HOME", &temp);
        std::env::remove_var("USERPROFILE");
        assert_eq!(expand_home("~/x/y"), temp.join("x/y"));
        assert_eq!(
            expand_home("/absolute/path"),
            PathBuf::from("/absolute/path")
        );

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_profile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }
}
