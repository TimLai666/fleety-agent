use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::{accept, Message};

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(args)
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
        .recv_timeout(Duration::from_secs(5))
        .expect("server received frames");
    (output, received)
}

#[test]
fn no_args_prints_top_level_help() {
    let output = run(&[]);

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fleety"));
    assert!(stdout.contains("fleety ask"));
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
    ] {
        let output = run(args);
        assert!(output.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("usage:"), "{args:?}: {stderr}");
    }
}

#[test]
fn init_and_pair_complete_handshake_and_save_config() {
    let home = TempHome::new("init-pair");

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

    let config_path = home.0.join(".fleety").join("config.json");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("config"))
            .expect("json");
    assert_eq!(config["agent_url"], init_url);

    let (pair_url, pair_rx) = start_ws_server(vec![vec![welcome(Some("pair-token"))]]);
    let (output, received) = run_against_server(&["pair", "PAIR-1"], &pair_url, &home, pair_rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("token saved"));
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-1"
    ));

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("config"))
            .expect("json");
    assert_eq!(config["agent_url"], pair_url);
    assert_eq!(config["token"], "pair-token");
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
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing backup"));
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::RollbackApply {
            device_id,
            backup_id
        }) if device_id == "cli-smoke" && backup_id == "b1"
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

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("server said no"));
    assert_eq!(received.len(), 2);
}
