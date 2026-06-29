use std::path::{Path, PathBuf};
use std::process::Command;

use agent_core::ToolRegistry;
use serde_json::json;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("fleety-insyra-smoke-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct EnvGuard(Option<String>);

impl EnvGuard {
    fn set_insyra_bin(path: &Path) -> Self {
        let old = std::env::var("FLEETY_INSYRA_BIN").ok();
        std::env::set_var("FLEETY_INSYRA_BIN", path);
        Self(old)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("FLEETY_INSYRA_BIN", value),
            None => std::env::remove_var("FLEETY_INSYRA_BIN"),
        }
    }
}

fn build_fake_sidecar(root: &Path) -> PathBuf {
    let src = root.join("fake_sidecar.rs");
    let exe = root.join(if cfg!(windows) {
        "fake-sidecar.exe"
    } else {
        "fake-sidecar"
    });
    std::fs::write(
        &src,
        r##"
use std::io::{self, BufRead};

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.unwrap_or_default();
        if line.contains("bad-json") {
            println!("not json");
        } else if line.contains("eof") {
            return;
        } else if line.contains("fail") {
            println!("{}", r#"{"ok":false,"error":"boom"}"#);
        } else {
            println!("{}", r#"{"ok":true,"output":"fake-ok"}"#);
        }
    }
}
"##,
    )
    .expect("write fake source");
    let output = Command::new("rustc")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("run rustc for fake sidecar");
    assert!(
        output.status.success(),
        "fake sidecar compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    exe
}

#[tokio::test]
async fn insyra_exec_uses_line_json_protocol_and_surfaces_sidecar_errors() {
    let _env_lock = ENV_LOCK.lock().await;
    let temp = TempDir::new("protocol");
    let fake = build_fake_sidecar(&temp.0);
    let _guard = EnvGuard::set_insyra_bin(&fake);

    let mut reg = ToolRegistry::new();
    fleety_tools::register_insyra(&mut reg, &temp.0);

    let command = reg
        .call(
            "insyra_exec",
            json!({ "command": "mean x", "session": "s" }),
        )
        .await
        .expect("command");
    assert_eq!(command["session"], json!("s"));
    assert_eq!(command["output"], json!("fake-ok"));

    let script = reg
        .call(
            "insyra_exec",
            json!({ "script": "newdl 1 2 as x\nmean x", "session": "s" }),
        )
        .await
        .expect("script");
    assert_eq!(script["output"], json!("fake-ok"));

    let reset = reg
        .call("insyra_exec", json!({ "reset": true, "session": "s" }))
        .await
        .expect("reset");
    assert_eq!(reset["output"], json!("fake-ok"));

    assert!(reg.call("insyra_exec", json!({})).await.is_err());
    assert!(reg
        .call("insyra_exec", json!({ "command": "fail", "session": "s" }))
        .await
        .is_err());
    assert!(reg
        .call(
            "insyra_exec",
            json!({ "command": "bad-json", "session": "s" })
        )
        .await
        .is_err());

    assert!(reg
        .call("insyra_exec", json!({ "command": "eof", "session": "s" }))
        .await
        .is_err());
    let after_eof = reg
        .call(
            "insyra_exec",
            json!({ "command": "mean x", "session": "s" }),
        )
        .await
        .expect("respawn after eof");
    assert_eq!(after_eof["output"], json!("fake-ok"));
}

#[tokio::test]
async fn insyra_exec_reports_missing_sidecar_as_actionable_error() {
    let _env_lock = ENV_LOCK.lock().await;
    let temp = TempDir::new("missing");
    let missing = temp.0.join("missing-sidecar");
    let _guard = EnvGuard::set_insyra_bin(&missing);

    let mut reg = ToolRegistry::new();
    fleety_tools::register_insyra(&mut reg, &temp.0);
    let err = reg
        .call("insyra_exec", json!({ "command": "mean x" }))
        .await
        .expect_err("missing sidecar should error");
    let message = err.report().message;
    assert!(message.contains("cannot start the fleety-insyra sidecar"));
}

#[tokio::test]
async fn insyra_exec_reports_env_dir_creation_errors() {
    let _env_lock = ENV_LOCK.lock().await;
    let temp = TempDir::new("bad-root");
    let fake = build_fake_sidecar(&temp.0);
    let _guard = EnvGuard::set_insyra_bin(&fake);
    let root_file = temp.0.join("not-a-dir");
    std::fs::write(&root_file, "file").expect("root file");

    let mut reg = ToolRegistry::new();
    fleety_tools::register_insyra(&mut reg, &root_file);
    let err = reg
        .call("insyra_exec", json!({ "command": "mean x" }))
        .await
        .expect_err("root file should block env dir creation");
    assert!(err
        .report()
        .message
        .contains("cannot create insyra env dir"));
}
