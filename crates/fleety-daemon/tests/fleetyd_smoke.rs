use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::{accept, Message};

static COMMAND_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("fleetyd-smoke-{name}-{}", std::process::id()));
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

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn welcome(token: Option<&str>) -> ServerMsg {
    ServerMsg::Welcome {
        session_id: "s1".into(),
        conversation_id: "c1".into(),
        protocol: PROTOCOL_VERSION,
        server_version: String::new(),
        audio_input: false,
        config_protocol: 0,
        server_fingerprint: None,
        loopback_trusted: false,
        token: token.map(String::from),
    }
}

fn start_ws_server(steps: Vec<Vec<ServerMsg>>) -> (String, mpsc::Receiver<Vec<ClientMsg>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ws server");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept ws");
        let mut ws = accept(stream).expect("accept websocket");
        let mut received = Vec::new();
        for responses in steps {
            let frame = ws.read().expect("client frame");
            let text = frame.to_text().expect("text frame");
            received.push(serde_json::from_str::<ClientMsg>(text).expect("client msg"));
            for response in responses {
                ws.send(Message::Text(
                    serde_json::to_string(&response).expect("server msg"),
                ))
                .expect("send response");
            }
        }
        let _ = ws.close(None);
        tx.send(received).expect("send received frames");
    });
    (format!("ws://{addr}"), rx)
}

fn start_held_ws_server(
    name: &'static str,
) -> (String, mpsc::Receiver<ClientMsg>, mpsc::Receiver<bool>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind held ws server");
    let addr = listener.local_addr().expect("held server address");
    let (hello_tx, hello_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept held websocket");
        let mut ws = accept(stream).expect("upgrade held websocket");
        let frame = ws.read().expect("held server hello");
        let hello = serde_json::from_str::<ClientMsg>(frame.to_text().expect("hello text"))
            .expect("parse held hello");
        hello_tx.send(hello).expect("publish held hello");
        ws.send(Message::Text(
            serde_json::to_string(&ServerMsg::Welcome {
                session_id: format!("session-{name}"),
                conversation_id: format!("conversation-{name}"),
                protocol: PROTOCOL_VERSION,
                server_version: String::new(),
                audio_input: false,
                config_protocol: 0,
                server_fingerprint: Some(format!("fingerprint-{name}")),
                loopback_trusted: false,
                token: None,
            })
            .expect("serialize held welcome"),
        ))
        .expect("send held welcome");
        let closed = loop {
            match ws.read() {
                Ok(Message::Close(_)) | Err(_) => break true,
                Ok(_) => continue,
            }
        };
        let _ = closed_tx.send(closed);
    });
    (format!("ws://{addr}"), hello_rx, closed_rx)
}

fn run_connected(
    home: &TempDir,
    root: &TempDir,
    url: &str,
    rx: mpsc::Receiver<Vec<ClientMsg>>,
) -> (ChildGuard, Vec<ClientMsg>) {
    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_AGENT_URL", url)
            .env("FLEETY_DEVICE_ID", "daemon-smoke")
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env_remove("FLEETY_TOKEN")
            .env_remove("FLEETY_PAIRING_CODE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run fleetyd"),
    );
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("server frames");
    (child, received)
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for daemon state"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn run_command(args: &[&str]) -> Output {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("command-contract-{seq}"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(args)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", "ws://127.0.0.1:9")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run fleetyd command");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().expect("poll fleetyd").is_some() {
            return child.wait_with_output().expect("collect fleetyd output");
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fleetyd {args:?} started the daemon instead of exiting");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn help_exits_zero_without_starting_daemon() {
    for arg in ["--help", "-h"] {
        let output = run_command(&[arg]);
        assert!(output.status.success(), "fleetyd {arg} should succeed");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("Usage: fleetyd"),
            "fleetyd {arg} should print usage"
        );
    }
}

#[test]
fn version_aliases_exit_zero_without_starting_daemon() {
    for arg in ["--version", "-V", "-v", "version"] {
        let output = run_command(&[arg]);
        assert!(output.status.success(), "fleetyd {arg} should succeed");
        assert!(String::from_utf8_lossy(&output.stdout).contains("fleetyd"));
    }
}

#[test]
fn subgroup_help_is_generated_before_daemon_initialization() {
    let home = TempDir::new("subgroup-help");
    let output = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(["config", "--help"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("run fleetyd config help");

    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    assert!(output.stderr.is_empty());
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn config_help_word_preserves_legacy_files_byte_for_byte() {
    let home = TempDir::new("config-help-word");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create fleety home");
    let legacy = fleety.join("config.json");
    std::fs::write(&legacy, b"{\"server_url\":\"ws://legacy:8787\"}\n")
        .expect("seed legacy config");
    let before = std::fs::read(&legacy).expect("legacy before");

    let output = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(["config", "help"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("run fleetyd config help word");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(std::fs::read(&legacy).expect("legacy after"), before);
    assert_eq!(std::fs::read_dir(&fleety).expect("list fleety").count(), 1);
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
fn daemon_config_rejects_server_owned_commands() {
    for args in [
        &["config", "provider", "list"][..],
        &["config", "set", "FLEETY_ADDR", "127.0.0.1:9999"][..],
    ] {
        let output = run_command(args);
        assert!(!output.status.success(), "{args:?}");
    }
}

#[test]
fn direct_daemon_config_output_is_terminal_safe_and_redacts_urls() {
    let home = TempDir::new("direct-config-safe");
    let hostile =
        "wss://user:PASS@example.test/work?token=SECRET#tail\u{1b}]52;c;owned\u{7}\rforged";
    let output = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(["config", "list"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_TZ", hostile)
        .output()
        .expect("run fleetyd config list");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.contains(&0x1b), "{:?}", output.stdout);
    assert!(!output.stdout.contains(&0x07), "{:?}", output.stdout);
    assert!(!output.stdout.contains(&b'\r'), "{:?}", output.stdout);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("wss://example.test/work?token=<redacted>"),
        "{stdout}"
    );
    for secret in ["user", "PASS", "SECRET", "#tail"] {
        assert!(!stdout.contains(secret), "leaked {secret}: {stdout}");
    }
}

#[test]
fn unknown_and_extra_arguments_fail_without_starting_daemon() {
    for args in [
        &["statuz"][..],
        &["version", "unexpected"][..],
        &["run-service", "unexpected"][..],
    ] {
        let output = run_command(args);
        assert!(!output.status.success(), "fleetyd {args:?} should fail");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Usage: fleetyd"),
            "fleetyd {args:?} should explain the valid syntax"
        );
    }
}

#[test]
fn daemon_session_persists_token_from_welcome_to_connections() {
    let home = TempDir::new("session-token");
    let root = TempDir::new("device-root");
    let (url, rx) = start_ws_server(vec![vec![welcome(Some("server-token"))]]);

    let (_child, received) = run_connected(&home, &root, &url, rx);

    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            device_id,
            token: None,
            pairing_code: None,
            ..
        }) if device_id == "daemon-smoke"
    ));
    // The minted token is persisted onto the current profile in connections.toml
    // (not the legacy fleetyd.token), so a restart reconnects without re-pairing.
    let conns_path = home.0.join(".fleety").join("connections.toml");
    wait_until(
        || matches!(std::fs::read_to_string(&conns_path), Ok(s) if s.contains("server-token")),
    );
}

#[test]
fn daemon_env_override_does_not_send_or_clear_another_profiles_token() {
    let home = TempDir::new("unauth");
    let root = TempDir::new("device-root-unauth");
    // Seed a current profile that already holds a token (a previously-paired
    // device). The daemon reads its token from connections.toml now.
    let conns_path = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(conns_path.parent().expect("parent")).expect("fleety dir");
    std::fs::write(
        &conns_path,
        "device_id = \"daemon-smoke\"\ncurrent = \"default\"\n\n\
         [profiles.default]\nurl = \"ws://placeholder:8787\"\ntoken = \"old-token\"\n",
    )
    .expect("seed connections");
    let (url, rx) = start_ws_server(vec![vec![ServerMsg::Error {
        error: WireError {
            kind: "unauthenticated".into(),
            message: "bad token".into(),
            remediation: None,
        },
    }]]);

    let (_child, received) = run_connected(&home, &root, &url, rx);

    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello { token: None, .. })
    ));
    // Give the daemon time to process the rejection. The env-selected Server
    // does not own the persisted profile, so its rejection cannot clear A.
    thread::sleep(Duration::from_millis(200));
    assert!(std::fs::read_to_string(&conns_path)
        .expect("connections after rejection")
        .contains("old-token"));
}

#[test]
fn daemon_owner_reconnect_switches_live_session_from_a_to_b_immediately() {
    let home = TempDir::new("owner-reconnect");
    let root = TempDir::new("owner-reconnect-root");
    let (url_a, hello_a, closed_a) = start_held_ws_server("a");
    let (url_b, hello_b, _closed_b) = start_held_ws_server("b");
    let conns_path = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(conns_path.parent().expect("connections parent"))
        .expect("create fleety dir");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url: url_a,
            token: Some("token-a".into()),
            ..Default::default()
        },
    );
    conns.profiles.insert(
        "B".into(),
        fleety_tools::connection::Profile {
            url: url_b,
            token: Some("token-b".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed A/B profiles");

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .env("FLEETY_DEVICE_ID", "daemon-smoke")
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .env_remove("FLEETY_PAIRING_CODE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run fleetyd on profile A");
    let _child = ChildGuard(child);

    let first = hello_a
        .recv_timeout(Duration::from_secs(15))
        .expect("profile A hello");
    assert!(matches!(
        first,
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("token-a")
    ));
    let ready_path = home.0.join(".fleety").join("fleetyd.control-ready.json");
    wait_until(|| ready_path.exists());

    fleety_tools::connection::mutate_at(&conns_path, |live| {
        live.current = Some("B".into());
        Ok(())
    })
    .expect("switch persisted owner to B");
    let reconnect = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(["reconnect", "--profile", "B"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .output()
        .expect("invoke daemon owner reconnect");
    assert!(
        reconnect.status.success(),
        "{}",
        String::from_utf8_lossy(&reconnect.stderr)
    );
    assert!(
        String::from_utf8_lossy(&reconnect.stdout).contains("left the previous Server"),
        "{}",
        String::from_utf8_lossy(&reconnect.stdout)
    );
    assert!(
        closed_a
            .recv_timeout(Duration::from_secs(5))
            .expect("profile A close"),
        "daemon must close A before acknowledging the switch"
    );
    let second = hello_b
        .recv_timeout(Duration::from_secs(15))
        .expect("profile B hello");
    assert!(matches!(
        second,
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("token-b")
    ));
}

#[test]
fn daemon_owner_reconnect_rejects_profile_switch_while_env_override_is_active() {
    let home = TempDir::new("owner-reconnect-env-override");
    let root = TempDir::new("owner-reconnect-env-root");
    let (url_a, hello_a, closed_a) = start_held_ws_server("env-a");
    let conns_path = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(conns_path.parent().expect("connections parent"))
        .expect("create fleety dir");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url: url_a.clone(),
            token: Some("token-a".into()),
            ..Default::default()
        },
    );
    conns.profiles.insert(
        "B".into(),
        fleety_tools::connection::Profile {
            url: "ws://127.0.0.1:9".into(),
            token: Some("token-b".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed override profiles");

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .env("FLEETY_AGENT_URL", &url_a)
        .env("FLEETY_DEVICE_ID", "daemon-smoke")
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env_remove("FLEETY_TOKEN")
        .env_remove("FLEETY_PAIRING_CODE")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run fleetyd with owner override");
    let _child = ChildGuard(child);
    hello_a
        .recv_timeout(Duration::from_secs(15))
        .expect("override A hello");
    let ready_path = home.0.join(".fleety").join("fleetyd.control-ready.json");
    wait_until(|| ready_path.exists());
    fleety_tools::connection::mutate_at(&conns_path, |live| {
        live.current = Some("B".into());
        Ok(())
    })
    .expect("persist B selection");

    let reconnect = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(["reconnect", "--profile", "B"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .output()
        .expect("request blocked reconnect");
    assert!(!reconnect.status.success());
    let error = String::from_utf8_lossy(&reconnect.stderr);
    assert!(error.contains("FLEETY_AGENT_URL"), "{error}");
    assert!(
        closed_a.recv_timeout(Duration::from_millis(300)).is_err(),
        "rejected switch must leave the override-owned A session intact"
    );
}

#[test]
fn daemon_executes_run_tool_frames_and_reports_errors() {
    let home = TempDir::new("run-tool");
    let root = TempDir::new("device-root-run-tool");
    std::fs::write(root.0.join("note.txt"), "hello from device").expect("seed file");
    let (url, rx) = start_ws_server(vec![
        vec![
            welcome(None),
            ServerMsg::RunTool {
                call_id: "call-ok".into(),
                tool: "read_file".into(),
                args_json: r#"{"path":"note.txt"}"#.into(),
            },
        ],
        vec![ServerMsg::RunTool {
            call_id: "call-err".into(),
            tool: "missing_tool".into(),
            args_json: "{}".into(),
        }],
        vec![],
    ]);

    let (_child, received) = run_connected(&home, &root, &url, rx);

    assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
    match received.get(1) {
        Some(ClientMsg::ToolResult {
            call_id,
            result_json,
        }) => {
            assert_eq!(call_id, "call-ok");
            assert!(result_json.contains("hello from device"));
        }
        other => panic!("expected tool result, got {other:?}"),
    }
    match received.get(2) {
        Some(ClientMsg::ToolError { call_id, error }) => {
            assert_eq!(call_id, "call-err");
            assert!(error.message.contains("missing_tool"));
        }
        other => panic!("expected tool error, got {other:?}"),
    }
}
