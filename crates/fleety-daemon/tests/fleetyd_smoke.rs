use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};
use tokio_tungstenite::tungstenite::{accept, Message};

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

#[test]
fn daemon_session_persists_token_from_welcome_across_disconnect() {
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
    let token_path = home.0.join(".fleety").join("fleetyd.token");
    wait_until(|| {
        matches!(
            std::fs::read_to_string(&token_path),
            Ok(token) if token == "server-token"
        )
    });
    let token = std::fs::read_to_string(token_path).expect("saved token");
    assert_eq!(token, "server-token");
}

#[test]
fn daemon_clears_saved_token_when_server_rejects_auth() {
    let home = TempDir::new("unauth");
    let root = TempDir::new("device-root-unauth");
    let token_path = home.0.join(".fleety").join("fleetyd.token");
    std::fs::create_dir_all(token_path.parent().expect("token parent")).expect("token dir");
    std::fs::write(&token_path, "old-token").expect("token");
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
        Some(ClientMsg::Hello {
            token: Some(token),
            ..
        }) if token == "old-token"
    ));
    wait_until(|| !token_path.exists());
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
