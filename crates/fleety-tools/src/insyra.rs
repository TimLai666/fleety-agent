//! The `insyra_exec` tool: data analysis via the Insyra DSL.
//!
//! It drives the `fleety-insyra` sidecar (a Go process wrapping
//! `github.com/HazelnutParadise/insyra/engine/dsl`) over a line-delimited JSON
//! protocol on stdin/stdout. One sidecar process is kept alive per tool
//! instance; DSL sessions are keyed by name and keep their state (variables,
//! loaded data) across calls. Environments persist on disk under
//! `<root>/.insyra`, so state also survives a sidecar restart.
//!
//! The sidecar binary is located via `FLEETY_INSYRA_BIN`, then next to the
//! current executable, then on `PATH`. If it is missing, the tool still
//! registers and returns an actionable error when called.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the `insyra_exec` tool rooted at `root` (its `.insyra` env store and
/// the sidecar's working directory live under `root`).
pub fn register_insyra(registry: &mut ToolRegistry, root: &Path) {
    registry.register(Box::new(InsyraExec {
        root: root.to_path_buf(),
        proc: Arc::new(Mutex::new(None)),
        seq: Arc::new(AtomicU64::new(0)),
    }));
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

struct InsyraExec {
    root: PathBuf,
    proc: Arc<Mutex<Option<Proc>>>,
    seq: Arc<AtomicU64>,
}

/// Resolve the sidecar binary: env override, then beside the current exe, then
/// the bare name for a `PATH` lookup at spawn time.
fn locate_binary() -> PathBuf {
    if let Ok(p) = std::env::var("FLEETY_INSYRA_BIN") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return pb;
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["fleety-insyra.exe"]
    } else {
        &["fleety-insyra"]
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from(names[0])
}

impl InsyraExec {
    fn spawn(&self) -> Result<Proc> {
        let bin = locate_binary();
        let env_root = self.root.join(".insyra");
        if let Err(e) = std::fs::create_dir_all(&env_root) {
            return Err(CoreError::Message(format!(
                "cannot create insyra env dir '{}': {e}",
                env_root.display()
            )));
        }
        let mut child = Command::new(&bin)
            .arg("-root")
            .arg(&env_root)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                CoreError::Message(format!(
                    "cannot start the fleety-insyra sidecar ('{}'): {e}. Install it (it ships with Fleety) or set FLEETY_INSYRA_BIN.",
                    bin.display()
                ))
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Message("insyra sidecar: no stdin pipe".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Message("insyra sidecar: no stdout pipe".to_string()))?;
        Ok(Proc {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn roundtrip(&self, req: &Value) -> Result<Value> {
        let mut guard = self
            .proc
            .lock()
            .map_err(|_| CoreError::Message("insyra sidecar lock poisoned".to_string()))?;
        if guard.is_none() {
            *guard = Some(self.spawn()?);
        }
        let proc = match guard.as_mut() {
            Some(p) => p,
            None => return Err(CoreError::Message("insyra sidecar unavailable".to_string())),
        };
        match exchange(proc, req) {
            Ok(line) => serde_json::from_str(&line)
                .map_err(|e| CoreError::Message(format!("bad insyra sidecar response: {e}"))),
            Err(e) => {
                // The pipe is likely broken; drop the process so the next call respawns.
                if let Some(mut dead) = guard.take() {
                    let _ = dead.child.kill();
                    let _ = dead.child.wait();
                }
                Err(CoreError::Message(format!("insyra sidecar I/O error: {e}")))
            }
        }
    }
}

/// One request line out, one response line in.
fn exchange(proc: &mut Proc, req: &Value) -> std::io::Result<String> {
    let mut line = serde_json::to_string(req).unwrap_or_default();
    line.push('\n');
    proc.stdin.write_all(line.as_bytes())?;
    proc.stdin.flush()?;
    let mut resp = String::new();
    if proc.stdout.read_line(&mut resp)? == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "sidecar closed the connection",
        ));
    }
    Ok(resp)
}

#[async_trait]
impl Tool for InsyraExec {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "insyra_exec".to_string(),
            description: "Data analysis via the Insyra DSL (.isr): load CSV/Parquet/Excel/SQL, transform (filter/groupby/scale/encode), run stats (mean/describe/ttest/anova/regression/kmeans), and plot. State persists within a `session` (variables + loaded data) across calls. Pass `script` for a multi-line .isr program or `command` for one line; `save <var> <file>` writes results into the workspace (read them back with read_file). Set `reset` to clear a session. Common DSL: newdl/load/read, filter/groupby/scale/encode, mean/median/describe/rank, ttest/ztest/anova/chisq/regression/kmeans, show/save/plot. Load the `fleety-use-insyra-dsl` skill for the full DSL reference.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "a single DSL line, e.g. \"mean x\"" },
                    "script": { "type": "string", "description": "a multi-line .isr program" },
                    "session": { "type": "string", "description": "session/env name (default 'default'); isolates persistent state" },
                    "reset": { "type": "boolean", "description": "clear the session's persisted state" }
                }
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let session = args
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let id = self.seq.fetch_add(1, Ordering::Relaxed).to_string();
        let req = if args.get("reset").and_then(Value::as_bool).unwrap_or(false) {
            json!({ "id": id, "op": "reset", "session": session })
        } else if let Some(script) = args.get("script").and_then(Value::as_str) {
            json!({ "id": id, "op": "exec_script", "session": session, "script": script })
        } else if let Some(command) = args.get("command").and_then(Value::as_str) {
            json!({ "id": id, "op": "exec", "session": session, "command": command })
        } else {
            return Err(CoreError::Message(
                "provide 'command' (one DSL line), 'script' (multi-line .isr), or 'reset'"
                    .to_string(),
            ));
        };

        let resp = self.roundtrip(&req)?;
        if !resp.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            let err = resp
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(CoreError::Message(format!("insyra: {err}")));
        }
        Ok(json!({
            "session": session,
            "output": resp.get("output").and_then(Value::as_str).unwrap_or(""),
        }))
    }
}
