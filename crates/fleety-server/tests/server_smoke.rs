use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

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
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("command-contract-home-{seq}"));
    let workspace = TempDir::new(&format!("command-contract-workspace-{seq}"));
    run_command_in(args, &home, &workspace)
}

fn run_command_in(args: &[&str], home: &TempDir, workspace: &TempDir) -> Output {
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
fn version_aliases_exit_zero_without_starting_server() {
    for arg in ["--version", "-V", "-v", "version"] {
        let output = run_command(&[arg]);
        assert!(
            output.status.success(),
            "fleety-server {arg} should succeed"
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("fleety-server"));
    }
}

#[test]
fn subgroup_help_is_generated_before_server_initialization() {
    for args in [["config", "--help"], ["backup", "--help"]] {
        let home = TempDir::new(&format!("subgroup-help-{}", args[0]));
        let agent_home = home.0.join("agent");
        let output = Command::new(env!("CARGO_BIN_EXE_fleety-server"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_AGENT_HOME", &agent_home)
            .output()
            .expect("run fleety-server subgroup help");

        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        assert!(output.stderr.is_empty());
        assert!(!agent_home.exists(), "{args:?} initialized server state");
        assert!(!home.0.join(".fleety").exists(), "{args:?} seeded config");
    }
}

#[test]
fn typo_is_a_usage_error_with_a_nearest_suggestion() {
    let output = run_command(&["statuz"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("status"));
}

#[test]
fn config_rejects_trailing_arguments_as_usage_errors() {
    let output = run_command(&["config", "list", "unexpected"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn server_config_rejects_daemon_owned_settings() {
    let output = run_command(&["config", "set", "FLEETY_PRESENCE", "on"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("daemon"));
}

#[test]
fn server_config_accepts_generated_options_first_provider_syntax() {
    let output = run_command(&["config", "provider", "add", "--type", "oauth:codex", "demo"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let help = run_command(&["config", "provider", "--help"]);
    assert!(help.status.success());
    assert!(
        !String::from_utf8_lossy(&help.stdout)
            .lines()
            .any(|line| line.trim_start().starts_with("edit")),
        "Server help must not advertise the CLI-only provider editor"
    );
}

#[test]
fn direct_server_provider_and_model_value_options_accept_separated_and_equals_forms() {
    let home = TempDir::new("direct-config-value-options-home");
    let workspace = TempDir::new("direct-config-value-options-workspace");
    let run_ok = |args: &[&str]| {
        let output = run_command_in(args, &home, &workspace);
        assert!(
            output.status.success(),
            "fleety-server {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run_ok(&[
        "config",
        "provider",
        "add",
        "separated",
        "--type",
        "api",
        "--url",
        "https://separated.example/v1",
        "--key",
        "separated-key",
    ]);
    run_ok(&[
        "config",
        "provider",
        "add",
        "equals",
        "--type=api",
        "--base-url=https://equals.example/v1",
        "--key=equals-key",
    ]);
    run_ok(&[
        "config",
        "provider",
        "set",
        "separated",
        "--type",
        "api",
        "--base-url",
        "https://updated-separated.example/v1",
        "--key",
        "updated-separated-key",
    ]);
    run_ok(&[
        "config",
        "provider",
        "set",
        "equals",
        "--type=api",
        "--url=https://updated-equals.example/v1",
        "--key=updated-equals-key",
    ]);

    run_ok(&[
        "config",
        "model",
        "set",
        "main",
        "--member",
        "separated/model-a",
        "--modalities",
        "text,image",
        "--effort",
        "high",
        "--strategy",
        "single",
    ]);
    run_ok(&[
        "config",
        "model",
        "set",
        "cheap",
        "--member=equals/model-b",
        "--modalities=text",
        "--effort=low",
        "--strategy=single",
    ]);

    run_ok(&[
        "config",
        "provider",
        "add",
        "--type=oauth:codex",
        "--",
        "after-terminator",
    ]);
    run_ok(&[
        "config",
        "model",
        "set",
        "--member=equals/model-c",
        "--strategy=single",
        "--",
        "main",
    ]);

    for args in [
        &[
            "config",
            "provider",
            "add",
            "duplicate-provider",
            "--type",
            "api",
            "--type=oauth:codex",
        ][..],
        &[
            "config",
            "model",
            "set",
            "duplicate-strategy",
            "--member=equals/model-d",
            "--strategy",
            "single",
            "--strategy=failover",
        ][..],
    ] {
        let output = run_command_in(args, &home, &workspace);
        assert_eq!(
            output.status.code(),
            Some(2),
            "duplicate option should be a usage error for {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn direct_server_provider_output_redacts_endpoint_secrets_and_controls() {
    let home = TempDir::new("provider-output-safe-home");
    let workspace = TempDir::new("provider-output-safe-workspace");
    let providers = home.0.join("providers.toml");
    std::fs::write(
        &providers,
        "[providers.hostile]\ntype = \"api\"\nbase_url = \"https://user:PASS@example.test/v1?token=SECRET#tail\"\nkey = \"provider-key\"\n",
    )
    .expect("seed providers");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety-server"))
        .args(["config", "provider", "list"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_HOME", &home.0)
        .env("FLEETY_WORKSPACE", &workspace.0)
        .env("FLEETY_PROVIDERS", &providers)
        .output()
        .expect("run provider list");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("https://example.test/v1?token=<redacted>"),
        "{stdout}"
    );
    for secret in ["user", "PASS", "SECRET", "#tail", "provider-key"] {
        assert!(!stdout.contains(secret), "leaked {secret}: {stdout}");
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
