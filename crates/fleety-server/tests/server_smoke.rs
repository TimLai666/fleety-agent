use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("fleety-server-smoke-{name}-{}", std::process::id()));
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

fn run_command(args: &[&str]) -> Output {
    let home = TempDir::new("command-contract-home");
    let workspace = TempDir::new("command-contract-workspace");
    let mut child = Command::new(env!("CARGO_BIN_EXE_fleety-server"))
        .args(args)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_ADDR", "not-an-address")
        .env("FLEETY_AGENT_HOME", &home.0)
        .env("FLEETY_WORKSPACE", &workspace.0)
        .env_remove("FLEETY_MODEL_BASE_URL")
        .env_remove("FLEETY_MODEL")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run fleety-server command");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().expect("poll fleety-server").is_some() {
            return child
                .wait_with_output()
                .expect("collect fleety-server output");
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fleety-server {args:?} started the server instead of exiting");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn help_exits_zero_without_starting_server() {
    for arg in ["--help", "-h"] {
        let output = run_command(&[arg]);
        assert!(
            output.status.success(),
            "fleety-server {arg} should succeed"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Usage: fleety-server"),
            "fleety-server {arg} should print usage"
        );
    }
}

#[test]
fn unknown_and_extra_arguments_fail_without_starting_server() {
    for args in [
        &["statuz"][..],
        &["version", "unexpected"][..],
        &["run-service", "unexpected"][..],
    ] {
        let output = run_command(args);
        assert!(
            !output.status.success(),
            "fleety-server {args:?} should fail"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Usage: fleety-server"),
            "fleety-server {args:?} should explain the valid syntax"
        );
    }
}

#[test]
fn invalid_bind_exits_after_startup_setup() {
    let home = TempDir::new("home");
    let workspace = TempDir::new("workspace");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety-server"))
        .env("FLEETY_ADDR", "not-an-address")
        .env("FLEETY_AGENT_HOME", &home.0)
        .env("FLEETY_WORKSPACE", &workspace.0)
        .env_remove("FLEETY_MODEL_BASE_URL")
        .env_remove("FLEETY_MODEL")
        .output()
        .expect("run fleety-server");

    assert!(output.status.success());
    assert!(home
        .0
        .join("skills")
        .join("builtin")
        .join("fleety-use-insyra-dsl")
        .join("SKILL.md")
        .is_file());
}
