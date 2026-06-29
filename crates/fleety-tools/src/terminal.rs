//! Interactive PTY terminal sessions.
//!
//! `run_command` / `ssh_exec` are one-shot (run to completion, return captured
//! output) so they can't drive an interactive TUI / REPL / installer prompt.
//! These tools open a **persistent** process under a PTY and let the agent drive
//! it turn by turn: [`terminal_open`] starts it, [`terminal_input`] sends a line
//! and reads the response, [`terminal_read`] fetches more, [`terminal_close`]
//! ends it. Each is an ordinary tool returning a single result — no protocol
//! change; the live PTY + child persist across calls in a process-global
//! registry keyed by session id (so the server process holds local/ssh sessions
//! and the daemon process holds device sessions).
//!
//! One implementation covers all three targets: the child is a local shell /
//! command, or `ssh -tt <host> <cmd>` when an `ssh_host` is given (ssh is just a
//! different argv). LLMs are turn-based, so this is *half-interactive*: each turn
//! reads until the output goes quiet (see [`should_stop_reading`]), not a true
//! real-time stream.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

// ---- tunables (env-overridable) ----

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn quiet_window() -> Duration {
    Duration::from_millis(env_u64("FLEETY_TERMINAL_QUIET_MS", 400))
}
fn max_window() -> Duration {
    Duration::from_millis(env_u64("FLEETY_TERMINAL_READ_MAX_MS", 8000))
}
fn max_sessions() -> usize {
    env_u64("FLEETY_TERMINAL_MAX_SESSIONS", 8) as usize
}
fn idle_ttl() -> Duration {
    Duration::from_secs(env_u64("FLEETY_TERMINAL_IDLE_TTL_SECS", 600))
}

// ---- pure helpers (unit-tested) ----

/// Stop reading this turn when the child has exited, the per-turn max window has
/// elapsed, or the output has gone quiet. `quiet_gap` is the time since the last
/// output byte — or since the turn started if none has arrived yet (the caller
/// computes `now - max(last_byte, turn_start)`), which gives output time to begin
/// so a turn doesn't return instantly empty right after input.
pub fn should_stop_reading(
    quiet_gap: Duration,
    total: Duration,
    child_exited: bool,
    quiet: Duration,
    max: Duration,
) -> bool {
    child_exited || total >= max || quiet_gap >= quiet
}

/// Strip ANSI/control escape sequences for a human/LLM-readable view.
pub fn strip_ansi(bytes: &[u8]) -> String {
    let cleaned = strip_ansi_escapes::strip(bytes);
    String::from_utf8_lossy(&cleaned).into_owned()
}

/// Build the `ssh` argv for a forced-PTY interactive session: `ssh -tt [opts]
/// host [command]`. Pure (mirrors `ssh_exec`'s option handling).
pub fn ssh_pty_argv(
    host: &str,
    user: Option<&str>,
    port: Option<u64>,
    identity: Option<&str>,
    command: &str,
) -> Vec<String> {
    let mut argv = vec![
        "ssh".to_string(),
        "-tt".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
    ];
    if let Some(p) = port {
        argv.push("-p".to_string());
        argv.push(p.to_string());
    }
    if let Some(id) = identity {
        argv.push("-i".to_string());
        argv.push(id.to_string());
    }
    argv.push(match user {
        Some(u) => format!("{u}@{host}"),
        None => host.to_string(),
    });
    if !command.trim().is_empty() {
        argv.push(command.to_string());
    }
    argv
}

/// Session ids (id, last_activity) whose idle time exceeds `ttl`. Pure.
pub fn idle_session_ids(now: Instant, ttl: Duration, items: &[(String, Instant)]) -> Vec<String> {
    items
        .iter()
        .filter(|(_, last)| now.saturating_duration_since(*last) > ttl)
        .map(|(id, _)| id.clone())
        .collect()
}

// ---- the live session + process-global registry ----

struct Shared {
    buf: Vec<u8>,
    last_byte: Instant,
    eof: bool,
}

struct PtySession {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    shared: Arc<Mutex<Shared>>,
    last_activity: Instant,
    exit_code: Option<i32>,
}

impl PtySession {
    /// Poll whether the child has exited, caching the exit code. Treats a wait
    /// error as exited so a turn never loops forever.
    fn poll_exit(&mut self) -> bool {
        if self.exit_code.is_some() {
            return true;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.exit_code = Some(status.exit_code() as i32);
                true
            }
            Ok(None) => false,
            Err(_) => {
                self.exit_code = Some(-1);
                true
            }
        }
    }
}

type Registry = Mutex<HashMap<String, PtySession>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<String, PtySession>> {
    // A poisoned lock (a thread panicked while holding it) shouldn't take down
    // the whole terminal subsystem — recover the guard and carry on.
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_shared(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// Spawn a process under a PTY and start a reader thread that accumulates its
/// output. Returns the session ready to register.
fn spawn_session(cmd: CommandBuilder) -> Result<PtySession> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| CoreError::Message(format!("cannot open pty: {e}")))?;
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| CoreError::Message(format!("cannot start terminal process: {e}")))?;
    // Close our handle to the slave so the reader sees EOF when the child exits.
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| CoreError::Message(format!("cannot read pty: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| CoreError::Message(format!("cannot write pty: {e}")))?;

    let shared = Arc::new(Mutex::new(Shared {
        buf: Vec::new(),
        last_byte: Instant::now(),
        eof: false,
    }));
    let shared_reader = Arc::clone(&shared);
    // Blocking reader thread: PTY reads are blocking; push bytes into the shared
    // buffer until EOF. Exits when the child dies (slave closed → EOF).
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut tmp = [0u8; 8192];
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => {
                    lock_shared(&shared_reader).eof = true;
                    break;
                }
                Ok(n) => {
                    let mut s = lock_shared(&shared_reader);
                    s.buf.extend_from_slice(&tmp[..n]);
                    s.last_byte = Instant::now();
                }
                Err(_) => {
                    lock_shared(&shared_reader).eof = true;
                    break;
                }
            }
        }
    });

    Ok(PtySession {
        child,
        writer,
        _master: pair.master,
        shared,
        last_activity: Instant::now(),
        exit_code: None,
    })
}

/// Read one turn from a registered session: poll until the output goes quiet /
/// the max window elapses / the child exits, then drain and return the bytes.
/// Locks the registry only briefly per poll (never across an await).
async fn read_turn(session_id: &str) -> Result<Value> {
    let quiet = quiet_window();
    let max = max_window();
    let turn_start = Instant::now();
    loop {
        let stop = {
            let mut reg = lock_registry();
            let sess = reg.get_mut(session_id).ok_or_else(|| {
                CoreError::Message(format!("no such terminal session '{session_id}'"))
            })?;
            let exited = sess.poll_exit();
            let now = Instant::now();
            let last_byte = lock_shared(&sess.shared).last_byte;
            let quiet_gap = now.saturating_duration_since(last_byte.max(turn_start));
            let total = now.saturating_duration_since(turn_start);
            should_stop_reading(quiet_gap, total, exited, quiet, max)
        };
        if stop {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Drain this turn's output + report status.
    let mut reg = lock_registry();
    let sess = reg
        .get_mut(session_id)
        .ok_or_else(|| CoreError::Message(format!("no such terminal session '{session_id}'")))?;
    sess.last_activity = Instant::now();
    let raw = {
        let mut s = lock_shared(&sess.shared);
        std::mem::take(&mut s.buf)
    };
    let running = sess.exit_code.is_none();
    Ok(json!({
        "session_id": session_id,
        "output": strip_ansi(&raw),
        "raw_output": String::from_utf8_lossy(&raw),
        "running": running,
        "exit_code": sess.exit_code,
    }))
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Register the interactive terminal tools.
pub fn register_terminal(registry: &mut ToolRegistry) {
    registry.register(Box::new(TerminalOpen));
    registry.register(Box::new(TerminalRead));
    registry.register(Box::new(TerminalInput));
    registry.register(Box::new(TerminalClose));
}

struct TerminalOpen;

#[async_trait]
impl Tool for TerminalOpen {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "terminal_open".to_string(),
            description: "Open an interactive terminal session under a PTY and read its first output. Drive it with terminal_input / terminal_read, then terminal_close. Unlike run_command this is for interactive programs (TUI / REPL / prompts). Half-interactive: each turn reads until the output goes quiet, not a live stream — call terminal_read for output that arrives later. Give `command` (empty = an interactive shell); set `ssh_host` (+ ssh_user/ssh_port/ssh_identity) to run it on a remote host via `ssh -tt`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "command to run (empty = interactive shell)" },
                    "cwd": { "type": "string", "description": "working directory (local sessions)" },
                    "ssh_host": { "type": "string", "description": "run on this host via ssh -tt" },
                    "ssh_user": { "type": "string" },
                    "ssh_port": { "type": "integer" },
                    "ssh_identity": { "type": "string", "description": "ssh private key path" }
                },
                "required": []
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let command = args.get("command").and_then(Value::as_str).unwrap_or("");
        let cmd = if let Some(host) = args.get("ssh_host").and_then(Value::as_str) {
            if host.is_empty() || host.starts_with('-') || host.split_whitespace().count() != 1 {
                return Err(CoreError::Message(format!(
                    "invalid ssh host '{host}': must be a single token not starting with '-'"
                )));
            }
            let argv = ssh_pty_argv(
                host,
                args.get("ssh_user").and_then(Value::as_str),
                args.get("ssh_port").and_then(Value::as_u64),
                args.get("ssh_identity").and_then(Value::as_str),
                command,
            );
            let mut c = CommandBuilder::new(&argv[0]);
            for a in &argv[1..] {
                c.arg(a);
            }
            c
        } else {
            let (shell, flag) = if cfg!(windows) {
                ("cmd", "/C")
            } else {
                ("sh", "-c")
            };
            let mut c = CommandBuilder::new(shell);
            if !command.is_empty() {
                c.arg(flag);
                c.arg(command);
            }
            if let Some(cwd) = args.get("cwd").and_then(Value::as_str) {
                c.cwd(cwd);
            }
            c
        };
        let mut cmd = cmd;
        cmd.env("TERM", "xterm-256color");

        let session = spawn_session(cmd)?;
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut reg = lock_registry();
            // Lazily reclaim idle sessions before enforcing the cap.
            let ttl = idle_ttl();
            let now = Instant::now();
            let items: Vec<(String, Instant)> = reg
                .iter()
                .map(|(k, v)| (k.clone(), v.last_activity))
                .collect();
            for stale in idle_session_ids(now, ttl, &items) {
                if let Some(mut s) = reg.remove(&stale) {
                    let _ = s.child.kill();
                }
            }
            let cap = max_sessions();
            if reg.len() >= cap {
                return Err(CoreError::Message(format!(
                    "too many terminal sessions ({} open, max {cap}); close one with terminal_close",
                    reg.len()
                )));
            }
            reg.insert(id.clone(), session);
        }
        read_turn(&id).await
    }
}

struct TerminalRead;

#[async_trait]
impl Tool for TerminalRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "terminal_read".to_string(),
            description: "Read more output from an open terminal session (for output that arrived after the previous turn). Returns the same shape as terminal_open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "session_id")?.to_string();
        {
            let reg = lock_registry();
            if !reg.contains_key(&id) {
                return Err(CoreError::Message(format!(
                    "no such terminal session '{id}'"
                )));
            }
        }
        read_turn(&id).await
    }
}

struct TerminalInput;

#[async_trait]
impl Tool for TerminalInput {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "terminal_input".to_string(),
            description: "Send input to an open terminal session (e.g. a line of text, a keystroke, \"y\\n\") and read the response. Include the trailing newline yourself when submitting a line. Returns the same shape as terminal_open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "data": { "type": "string", "description": "bytes to write to the terminal (include \\n to submit a line)" }
                },
                "required": ["session_id", "data"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "session_id")?.to_string();
        let data = require_str(&args, "data")?.to_string();
        {
            let mut reg = lock_registry();
            let sess = reg
                .get_mut(&id)
                .ok_or_else(|| CoreError::Message(format!("no such terminal session '{id}'")))?;
            sess.writer
                .write_all(data.as_bytes())
                .and_then(|()| sess.writer.flush())
                .map_err(|e| CoreError::Message(format!("cannot write to terminal: {e}")))?;
            sess.last_activity = Instant::now();
        }
        read_turn(&id).await
    }
}

struct TerminalClose;

#[async_trait]
impl Tool for TerminalClose {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "terminal_close".to_string(),
            description: "Close an open terminal session: terminate the process if still running and release it. Returns its exit code.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "session_id": { "type": "string" } },
                "required": ["session_id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "session_id")?.to_string();
        let mut sess = {
            let mut reg = lock_registry();
            reg.remove(&id)
                .ok_or_else(|| CoreError::Message(format!("no such terminal session '{id}'")))?
        };
        let exit_code = if sess.poll_exit() {
            sess.exit_code
        } else {
            let _ = sess.child.kill();
            let _ = sess.child.wait();
            sess.poll_exit();
            sess.exit_code
        };
        Ok(json!({ "session_id": id, "exit_code": exit_code }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_stop_reading_table() {
        let quiet = Duration::from_millis(400);
        let max = Duration::from_millis(8000);
        let within_q = Duration::from_millis(100);
        let past_q = Duration::from_millis(500);
        let within_m = Duration::from_millis(1000);
        let past_m = Duration::from_millis(9000);
        // gone quiet
        assert!(should_stop_reading(past_q, within_m, false, quiet, max));
        // hit max
        assert!(should_stop_reading(within_q, past_m, false, quiet, max));
        // child done
        assert!(should_stop_reading(within_q, within_m, true, quiet, max));
        // keep reading
        assert!(!should_stop_reading(within_q, within_m, false, quiet, max));
    }

    #[test]
    fn strip_ansi_removes_escapes() {
        let raw = b"\x1b[31mred\x1b[0m \x1b[1mbold\x1b[0m";
        assert_eq!(strip_ansi(raw), "red bold");
    }

    #[test]
    fn ssh_pty_argv_forces_tty_and_options() {
        let argv = ssh_pty_argv("pi", Some("admin"), Some(2222), Some("~/.ssh/id"), "uptime");
        assert_eq!(argv[0], "ssh");
        assert!(argv.contains(&"-tt".to_string()));
        assert!(argv.contains(&"admin@pi".to_string()));
        assert!(argv
            .windows(2)
            .any(|w| w == ["-p".to_string(), "2222".to_string()]));
        assert!(argv
            .windows(2)
            .any(|w| w == ["-i".to_string(), "~/.ssh/id".to_string()]));
        assert_eq!(argv.last(), Some(&"uptime".to_string()));
        // No command → no trailing arg (interactive remote shell).
        let plain = ssh_pty_argv("host", None, None, None, "");
        assert_eq!(plain.last(), Some(&"host".to_string()));
    }

    #[test]
    fn idle_session_ids_picks_stale() {
        let now = Instant::now();
        let items = vec![
            ("old".to_string(), now - Duration::from_secs(700)),
            ("fresh".to_string(), now - Duration::from_secs(10)),
        ];
        assert_eq!(
            idle_session_ids(now, Duration::from_secs(600), &items),
            vec!["old".to_string()]
        );
    }

    /// A command that stays alive ~5s without needing interactive input.
    fn slow_cmd() -> &'static str {
        if cfg!(windows) {
            "ping -n 6 127.0.0.1 >NUL"
        } else {
            "sleep 5"
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn open_reads_output_and_exits() {
        let res = TerminalOpen
            .call(json!({ "command": "echo hello-pty" }))
            .await
            .expect("open");
        assert!(res["output"]
            .as_str()
            .unwrap_or_default()
            .contains("hello-pty"));
        let id = res["session_id"].as_str().expect("id").to_string();
        // echo exits on its own; a follow-up read should see it done (poll a few times).
        let mut done = !res["running"].as_bool().unwrap_or(true);
        for _ in 0..5 {
            if done {
                break;
            }
            let r = TerminalRead
                .call(json!({ "session_id": id }))
                .await
                .expect("read");
            done = !r["running"].as_bool().unwrap_or(true);
        }
        assert!(done, "echo should have exited");
        let _ = TerminalClose.call(json!({ "session_id": id })).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn interactive_echo_round_trip() {
        // `cat` echoes stdin back through the PTY — a minimal interactive program.
        let opened = TerminalOpen
            .call(json!({ "command": "cat" }))
            .await
            .expect("open");
        let id = opened["session_id"].as_str().expect("id").to_string();
        assert!(
            opened["running"].as_bool().unwrap_or(false),
            "cat stays open"
        );
        let res = TerminalInput
            .call(json!({ "session_id": id, "data": "ping-123\n" }))
            .await
            .expect("input");
        assert!(
            res["output"]
                .as_str()
                .unwrap_or_default()
                .contains("ping-123"),
            "cat should echo the line back, got: {:?}",
            res["output"]
        );
        let closed = TerminalClose
            .call(json!({ "session_id": id }))
            .await
            .expect("close");
        assert_eq!(closed["session_id"], json!(id));
    }

    #[tokio::test]
    async fn unknown_session_errors() {
        assert!(TerminalRead
            .call(json!({ "session_id": "nope" }))
            .await
            .is_err());
        assert!(TerminalInput
            .call(json!({ "session_id": "nope", "data": "x\n" }))
            .await
            .is_err());
        assert!(TerminalClose
            .call(json!({ "session_id": "nope" }))
            .await
            .is_err());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn session_cap_is_enforced() {
        std::env::set_var("FLEETY_TERMINAL_MAX_SESSIONS", "1");
        // Drain any sessions left by other tests so the cap test is deterministic.
        let stale: Vec<String> = { lock_registry().keys().cloned().collect() };
        for id in stale {
            let _ = TerminalClose.call(json!({ "session_id": id })).await;
        }
        let first = TerminalOpen
            .call(json!({ "command": slow_cmd() }))
            .await
            .expect("first opens");
        let id1 = first["session_id"].as_str().expect("id").to_string();
        // Second open exceeds the cap of 1.
        let second = TerminalOpen.call(json!({ "command": slow_cmd() })).await;
        assert!(second.is_err(), "second open should hit the cap");
        let _ = TerminalClose.call(json!({ "session_id": id1 })).await;
        std::env::remove_var("FLEETY_TERMINAL_MAX_SESSIONS");
    }
}
