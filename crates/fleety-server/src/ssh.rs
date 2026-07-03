//! SSH connector: run commands on remote hosts via the system `ssh` client.
//!
//! No extra dependency — shells out to `ssh` in non-interactive `BatchMode`
//! (so it fails fast instead of prompting). This lets the agent operate hosts
//! that don't run fleetyd. The arg construction is pure and unit-tested; the
//! live connection follows the codebase's logic-tested/live-manual posture.

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the `ssh_exec` connector tool.
pub fn register(registry: &mut ToolRegistry) {
    registry.register(Box::new(SshExec));
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Build the `ssh` argument vector (everything after the program name).
/// When `control_dir` is set, enable connection multiplexing (ControlMaster) so
/// repeated calls to the same host reuse one persistent connection.
fn build_ssh_args(
    host: &str,
    command: &str,
    user: Option<&str>,
    port: Option<u64>,
    identity: Option<&str>,
    control_dir: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
    ];
    if let Some(dir) = control_dir {
        args.push("-o".to_string());
        args.push("ControlMaster=auto".to_string());
        args.push("-o".to_string());
        args.push(format!("ControlPath={dir}/%C"));
        args.push("-o".to_string());
        args.push("ControlPersist=60".to_string());
    }
    if let Some(p) = port {
        args.push("-p".to_string());
        args.push(p.to_string());
    }
    if let Some(id) = identity {
        args.push("-i".to_string());
        args.push(id.to_string());
    }
    let target = match user {
        Some(u) => format!("{u}@{host}"),
        None => host.to_string(),
    };
    args.push(target);
    args.push(command.to_string());
    args
}

struct SshExec;

#[async_trait]
impl Tool for SshExec {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ssh_exec".to_string(),
            description: "Run a command on a remote host over SSH (system ssh client, non-interactive BatchMode). Times out (default 120s; pass `timeout_secs`, 0 to disable). Non-interactive — it cannot answer prompts or drive a remote TUI.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "host": { "type": "string" },
                    "command": { "type": "string" },
                    "user": { "type": "string" },
                    "port": { "type": "integer" },
                    "identity": { "type": "string", "description": "path to a private key file" },
                    "timeout_secs": { "type": "integer", "description": "wall-clock limit; omit for the default, 0 to disable" }
                },
                "required": ["host", "command"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let host = require_str(&args, "host")?;
        if host.is_empty() || host.starts_with('-') || host.split_whitespace().count() != 1 {
            return Err(CoreError::Message(format!(
                "invalid ssh host '{host}': must be a single token not starting with '-'"
            )));
        }
        let command = require_str(&args, "command")?;
        // Irreversible remote commands are treated as critical: refuse them, the
        // same guard the local `run_command` applies (a remote `rm -rf /` is just
        // as catastrophic).
        if let Some(reason) = fleety_tools::critical_reason(command) {
            return Err(CoreError::Message(format!(
                "refusing critical remote command ({reason}); run it yourself on the host if you're sure"
            )));
        }
        let user = args.get("user").and_then(Value::as_str);
        let port = args.get("port").and_then(Value::as_u64);
        let identity = args.get("identity").and_then(Value::as_str);

        // Persistent connection multiplexing via ControlMaster (Unix only; the
        // Windows OpenSSH client doesn't support it). Repeated calls to the same
        // host then reuse one connection.
        let control_dir = if cfg!(unix) {
            let dir = std::env::temp_dir().join("fleety-ssh");
            let _ = std::fs::create_dir_all(&dir);
            Some(dir.to_string_lossy().to_string())
        } else {
            None
        };
        let ssh_args = build_ssh_args(host, command, user, port, identity, control_dir.as_deref());
        let timeout =
            fleety_tools::command_timeout(args.get("timeout_secs").and_then(Value::as_u64));
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.args(&ssh_args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let spawned = cmd.spawn().map_err(|e| {
            CoreError::Message(format!(
                "cannot run ssh (is the ssh client installed?): {e}"
            ))
        })?;
        let output = match timeout {
            Some(dur) => match tokio::time::timeout(dur, spawned.wait_with_output()).await {
                Ok(r) => r.map_err(|e| CoreError::Message(format!("ssh failed: {e}")))?,
                Err(_) => {
                    return Ok(json!({
                        "status": Value::Null,
                        "stdout": "",
                        "stderr": format!(
                            "ssh command timed out after {}s and was terminated (pass a larger timeout_secs, or 0 to disable)",
                            dur.as_secs()
                        ),
                        "timed_out": true,
                    }));
                }
            },
            None => spawned
                .wait_with_output()
                .await
                .map_err(|e| CoreError::Message(format!("ssh failed: {e}")))?,
        };
        Ok(json!({
            "status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_args_includes_target_and_options() {
        let args = build_ssh_args(
            "pi.local",
            "uptime",
            Some("admin"),
            Some(2222),
            Some("~/.ssh/id"),
            Some("/tmp/fleety-ssh"),
        );
        assert!(args.contains(&"admin@pi.local".to_string()));
        assert!(args.contains(&"uptime".to_string()));
        assert!(args
            .windows(2)
            .any(|w| w == ["-p".to_string(), "2222".to_string()]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-i".to_string(), "~/.ssh/id".to_string()]));
        assert!(args.contains(&"BatchMode=yes".to_string()));
        // Multiplexing options present when a control dir is given.
        assert!(args.contains(&"ControlMaster=auto".to_string()));
        assert!(args
            .iter()
            .any(|a| a.starts_with("ControlPath=/tmp/fleety-ssh/")));

        let plain = build_ssh_args("host", "ls", None, None, None, None);
        assert_eq!(plain.last(), Some(&"ls".to_string()));
        assert!(plain.contains(&"host".to_string()));
        assert!(!plain.contains(&"-p".to_string()));
        assert!(!plain.contains(&"ControlMaster=auto".to_string()));
    }

    #[tokio::test]
    async fn rejects_option_injection_host() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        assert!(registry
            .call(
                "ssh_exec",
                json!({ "host": "-oProxyCommand=evil", "command": "x" })
            )
            .await
            .is_err());
        assert!(registry
            .call("ssh_exec", json!({ "host": "a b", "command": "x" }))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn refuses_critical_remote_command() {
        let mut registry = ToolRegistry::new();
        register(&mut registry);
        // A catastrophic, irreversible command is refused before any ssh spawn —
        // the same guard as the local run_command.
        let err = registry
            .call("ssh_exec", json!({ "host": "host", "command": "rm -rf /" }))
            .await
            .expect_err("critical remote command must be refused");
        assert!(err.report().message.contains("critical"));
        // An ordinary command is not blocked by the guard (it fails later only if
        // the host is unreachable, not here).
        assert!(fleety_tools::critical_reason("ls -la").is_none());
    }
}
