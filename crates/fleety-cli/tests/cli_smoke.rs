use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::{accept, Message};

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Run the CLI in an isolated temp HOME so a command never reads or migrates the
/// developer's real `~/.fleety` (main() runs the one-time config.json migration).
fn run(args: &[&str]) -> std::process::Output {
    let n = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempHome::new(&format!("run-{n}"));
    Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(args)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("run fleety")
}

fn rejecting_ws_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind rejecting server");
    let addr = listener.local_addr().expect("rejecting addr");
    thread::spawn(move || {
        let _ = listener.accept();
    });
    format!("ws://{addr}")
}

fn run_with_rejecting_agent(args: &[&str]) -> std::process::Output {
    let home = TempHome::new("unreachable");
    let url = rejecting_ws_url();
    Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(args)
        .env("FLEETY_AGENT_URL", url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run fleety")
}

struct TempHome(std::path::PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("fleety-cli-smoke-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp home");
        Self(path)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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

fn run_against_server(
    args: &[&str],
    url: &str,
    home: &TempHome,
    rx: mpsc::Receiver<Vec<ClientMsg>>,
) -> (std::process::Output, Vec<ClientMsg>) {
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(args)
        .env("FLEETY_AGENT_URL", url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run fleety");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|e| {
            panic!(
                "server received frames ({e:?}); stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
    (output, received)
}

fn run_against_profile(
    args: &[&str],
    home: &TempHome,
    rx: mpsc::Receiver<Vec<ClientMsg>>,
) -> (std::process::Output, Vec<ClientMsg>) {
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(args)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run fleety");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|e| {
            panic!(
                "profile server received frames ({e:?}); stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
    (output, received)
}

#[test]
fn no_args_prints_top_level_help() {
    let output = run(&[]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("usage: fleety"));
    // The full command surface is listed, not just a teaser.
    for cmd in [
        "ask", "tui", "config", "audit", "rollback", "pair", "update",
    ] {
        assert!(stdout.contains(cmd), "help lists {cmd}");
    }

    // `help` / `--help` / `-h` print the same thing.
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("usage: fleety"));
}

#[test]
fn help_and_version_do_not_create_or_migrate_user_files() {
    for arg in ["--help", "--version"] {
        let home = TempHome::new(&format!("query-{}", arg.trim_start_matches('-')));
        let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .arg(arg)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .output()
            .expect("run query");
        assert!(output.status.success(), "{arg}");
        assert!(
            !home.0.join(".fleety").exists(),
            "{arg} must be side-effect free"
        );
    }
}

#[test]
fn unknown_command_errors_to_stderr_with_nonzero_exit() {
    let output = run(&["frobnicate"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command 'frobnicate'"), "{stderr}");
}

#[test]
fn usage_errors_return_before_network_work() {
    for args in [
        &["init"][..],
        &["ask"][..],
        &["resume"][..],
        &["audit"][..],
        &["audit", "show"][..],
        &["rollback"][..],
        &["rollback", "apply"][..],
        &["pair"][..],
        &["pair", "one", "extra"][..],
        &["pair-code", "extra"][..],
        &["status", "extra"][..],
        &["voice", "extra"][..],
        &["update", "extra"][..],
        &["conversations", "not-a-number"][..],
    ] {
        let output = run(args);
        // Usage mistakes exit 2 so scripts can tell them from runtime failures.
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("usage:"), "{args:?}: {stderr}");
    }
}

#[test]
fn config_owner_failures_never_fall_back_to_local_files() {
    let home = TempHome::new("config-owner-boundary");
    let fleety_dir = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety_dir).expect("create .fleety");
    let config_path = fleety_dir.join("config.toml");
    let providers_path = fleety_dir.join("providers.toml");
    std::fs::write(&config_path, "[cli]\nFLEETY_VOICE_AUDIO = \"auto\"\n").expect("seed config");
    std::fs::write(&providers_path, "# provider sentinel\n").expect("seed providers");
    let config_before = std::fs::read(&config_path).expect("config bytes");
    let providers_before = std::fs::read(&providers_path).expect("provider bytes");

    let run_owner = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .output()
            .expect("run config")
    };

    let local_provider = run_owner(&["config", "--target", "local", "provider", "edit"]);
    assert!(!local_provider.status.success());
    assert!(String::from_utf8_lossy(&local_provider.stderr).contains("owned by server"));

    for args in [
        &["config", "set", "FLEETY_TZ", "UTC"][..],
        &["config", "set", "FLEETY_ADDR", "127.0.0.1:9999"][..],
    ] {
        let output = run_owner(args);
        assert!(!output.status.success(), "{args:?}");
    }
    assert_eq!(
        std::fs::read(&config_path).expect("config unchanged"),
        config_before
    );
    assert_eq!(
        std::fs::read(&providers_path).expect("providers unchanged"),
        providers_before
    );

    let cli_write = run_owner(&["config", "set", "FLEETY_VOICE_AUDIO", "off"]);
    assert!(
        cli_write.status.success(),
        "{}",
        String::from_utf8_lossy(&cli_write.stderr)
    );
    let config_after = std::fs::read_to_string(&config_path).expect("read CLI config");
    assert!(config_after.contains("FLEETY_VOICE_AUDIO = \"off\""));
    assert_eq!(
        std::fs::read(&providers_path).expect("providers still unchanged"),
        providers_before
    );
}

#[test]
fn init_and_pair_write_the_connection_profile() {
    let home = TempHome::new("init-pair");
    let connections_path = home.0.join(".fleety").join("connections.toml");

    // `fleety init <url>` is sugar for `server add default <url> --use` + enroll:
    // it records the profile, makes it current, and completes the handshake.
    let (init_url, init_rx) = start_ws_server(vec![vec![welcome(None)]]);
    let (output, received) = run_against_server(&["init", &init_url], &init_url, &home, init_rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("registered device"));
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            pairing_code: None,
            ..
        })
    ));

    let conns = std::fs::read_to_string(&connections_path).expect("connections.toml");
    assert!(conns.contains(&init_url), "profile url persisted: {conns}");
    assert!(
        conns.contains("current = \"default\""),
        "made current: {conns}"
    );

    // `fleety pair <code>` requires a named profile and writes only that
    // profile. Point the verified profile at the pairing fixture first.
    let (pair_url, pair_rx) = start_ws_server(vec![vec![welcome(Some("pair-token"))]]);
    let set_url = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["server", "set-url", "default", pair_url.as_str()])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("set profile URL");
    assert!(set_url.status.success());
    let (output, received) = run_against_profile(&["pair", "PAIR-1"], &home, pair_rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("token saved"));
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-1"
    ));

    let conns = std::fs::read_to_string(&connections_path).expect("connections.toml");
    assert!(
        conns.contains("pair-token"),
        "pair token landed on the current profile: {conns}"
    );
}

#[test]
fn server_commands_manage_connection_profiles() {
    let home = TempHome::new("server-profiles");
    let run_srv = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .env_remove("FLEETY_CONNECTIONS")
            .output()
            .expect("run fleety")
    };

    // First `add` auto-selects; `current` prints the name + url.
    let out = run_srv(&["server", "add", "home", "ws://home:8787"]);
    assert!(
        out.status.success(),
        "add home: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cur = run_srv(&["server", "current"]);
    let cur_s = String::from_utf8_lossy(&cur.stdout);
    assert!(
        cur_s.contains("home") && cur_s.contains("ws://home:8787"),
        "current names the server: {cur_s}"
    );

    // A second server, without --use, leaves `home` current; `list` marks it.
    run_srv(&["server", "add", "work", "ws://work:8787"]);
    let list = run_srv(&["server", "list"]);
    let list_s = String::from_utf8_lossy(&list.stdout);
    assert!(
        list_s.contains("* home"),
        "list stars the current: {list_s}"
    );
    assert!(list_s.contains("work"), "list shows every server: {list_s}");

    // Removing the current server without --force is rejected (non-zero exit).
    let rm = run_srv(&["server", "remove", "home"]);
    assert!(!rm.status.success(), "remove current must need --force");
    assert!(String::from_utf8_lossy(&rm.stderr).contains("--force"));

    // After switching away, removing the former current succeeds.
    run_srv(&["server", "use", "work"]);
    let rm2 = run_srv(&["server", "remove", "home"]);
    assert!(
        rm2.status.success(),
        "remove non-current: {}",
        String::from_utf8_lossy(&rm2.stderr)
    );
}

#[test]
fn ask_sends_user_message_and_prints_assistant() {
    let home = TempHome::new("ask");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            ServerMsg::Assistant {
                conversation_id: "c1".into(),
                text: "answer text".into(),
                seq: 7,
                speech: None,
                attention: None,
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
    ]);

    let (output, received) = run_against_server(&["ask", "hello"], &url, &home, rx);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("answer text"));
    assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::UserMessage { text, origin, .. })
            if text == "hello" && origin.os.as_deref() == Some(std::env::consts::OS)
    ));
}

#[test]
fn ask_denies_approval_when_stdin_is_closed() {
    let home = TempHome::new("approval-deny");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ApprovalRequested {
            approval_id: "ap-1".into(),
            tool: "write_file".into(),
            summary: "write something".into(),
            risk: "mutate".into(),
        }],
        vec![ServerMsg::Done {
            conversation_id: "c1".into(),
        }],
    ]);

    let (output, received) = run_against_server(&["ask", "please write"], &url, &home, rx);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Approve tool"));
    assert!(matches!(
        received.get(2),
        Some(ClientMsg::Deny { approval_id }) if approval_id == "ap-1"
    ));
}

#[test]
fn resume_audit_and_rollback_render_server_results() {
    let home = TempHome::new("render-results");

    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            ServerMsg::Replay {
                conversation_id: "c1".into(),
                seq: 3,
                role: "assistant".into(),
                content: "old answer".into(),
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
    ]);
    let (output, received) = run_against_server(&["resume", "c1", "2"], &url, &home, rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("[3] assistant: old answer"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::Resume {
            conversation_id,
            after_seq
        }) if conversation_id == "c1" && *after_seq == 2
    ));

    let entries = serde_json::json!([
        {"index": 4, "kind": "tool_call", "tool": "read_file"},
        {"index": 5, "kind": "message"}
    ]);
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::AuditListResult {
            device_id: "cli-smoke".into(),
            entries_json: entries.to_string(),
        }],
    ]);
    let (output, received) = run_against_server(&["audit", "list", "2"], &url, &home, rx);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tool_call"));
    assert!(stdout.contains("read_file"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::AuditList {
            device_id,
            limit: Some(2),
            ..
        }) if device_id == "cli-smoke"
    ));

    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::AuditShowResult {
            device_id: "cli-smoke".into(),
            index: 4,
            event_json: r#"{"kind":"tool_result","ok":true}"#.into(),
        }],
    ]);
    let (output, received) = run_against_server(&["audit", "show", "4"], &url, &home, rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("tool_result"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::AuditShow {
            device_id,
            index: 4
        }) if device_id == "cli-smoke"
    ));

    let backups = serde_json::json!([
        {"id": "b1", "original_rel_path": "src/main.rs", "ts_secs": 10}
    ]);
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::RollbackListResult {
            device_id: "cli-smoke".into(),
            backups_json: backups.to_string(),
        }],
    ]);
    let (output, received) = run_against_server(&["rollback", "list"], &url, &home, rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("src/main.rs"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::RollbackList { device_id }) if device_id == "cli-smoke"
    ));

    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::RollbackResult {
            device_id: "cli-smoke".into(),
            backup_id: "b1".into(),
            ok: false,
            message: "missing backup".into(),
        }],
    ]);
    let (output, received) = run_against_server(&["rollback", "apply", "b1"], &url, &home, rx);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing backup"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::RollbackApply {
            device_id,
            backup_id
        }) if device_id == "cli-smoke" && backup_id == "b1"
    ));

    // `conversations` renders the listing (ids + previews) and sends a
    // ConversationList request carrying the parsed limit.
    let convs = serde_json::json!([
        {"conversation_id": "c-new", "last_ts_secs": 20, "events": 4, "preview": "newest topic"},
        {"conversation_id": "c-old", "last_ts_secs": 10, "events": 2, "preview": "older topic"}
    ]);
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConversationListResult {
            conversations_json: convs.to_string(),
        }],
    ]);
    let (output, received) = run_against_server(&["conversations", "5"], &url, &home, rx);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("c-new"));
    assert!(stdout.contains("newest topic"));
    assert!(stdout.contains("c-old"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::ConversationList { limit: Some(5) })
    ));
}

#[test]
fn commands_render_server_errors_without_panicking() {
    let home = TempHome::new("server-errors");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::Error {
            error: WireError {
                kind: "provider".into(),
                message: "server said no".into(),
                remediation: None,
            },
        }],
    ]);

    let (output, received) = run_against_server(&["audit", "list"], &url, &home, rx);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("server said no"));
    assert_eq!(received.len(), 2);
}

#[test]
fn network_commands_report_connection_errors_without_panicking() {
    for args in [
        &["ask", "hi"][..],
        &["resume", "c1"][..],
        &["audit", "list"][..],
        &["audit", "show", "1"][..],
        &["rollback", "list"][..],
        &["rollback", "apply", "b1"][..],
    ] {
        let output = run_with_rejecting_agent(args);
        // Connection failures exit non-zero so `fleety … && next` behaves.
        assert!(!output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("cannot connect") || stderr.contains("cannot open SSE"),
            "{args:?}: {stderr}"
        );
    }

    let output = run_with_rejecting_agent(&["pair", "PAIR-1"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("named server profile"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let url = rejecting_ws_url();
    let output = run(&["init", &url]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot connect") || stderr.contains("cannot open SSE"),
        "{stderr}"
    );
}

#[test]
fn init_pair_and_ask_report_unexpected_server_frames() {
    let home = TempHome::new("unexpected");
    let server_error = ServerMsg::Error {
        error: WireError {
            kind: "provider".into(),
            message: "wrong frame".into(),
            remediation: None,
        },
    };

    let (url, rx) = start_ws_server(vec![vec![server_error.clone()]]);
    let (output, _) = run_against_server(&["init", &url], &url, &home, rx);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("server rejected init") && stderr.contains("wrong frame"),
        "{stderr}"
    );

    let (url, rx) = start_ws_server(vec![vec![server_error]]);
    let (output, _) = run_against_server(&["ask", "hi"], &url, &home, rx);
    assert!(!output.status.success());
    // The handshake failure is reported readably (the server's message surfaced),
    // never a Debug dump of the internal frame.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wrong frame") && !stderr.contains("Error {"),
        "{stderr}"
    );

    let (url, rx) = start_ws_server(vec![vec![welcome(None)]]);
    let add = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["server", "add", "default", url.as_str(), "--use"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("add pair profile");
    assert!(add.status.success());
    let (output, _) = run_against_profile(&["pair", "PAIR-2"], &home, rx);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("server returned no token"));
}
