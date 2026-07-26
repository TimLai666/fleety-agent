use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};
use mdns_sd::{ServiceDaemon, ServiceInfo};
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
        server_fingerprint: Some("fingerprint-smoke".into()),
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

fn start_duplicate_welcome_server() -> (String, mpsc::Receiver<ClientMsg>) {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind duplicate Welcome server");
    let addr = listener.local_addr().expect("duplicate Welcome address");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept duplicate Welcome ws");
        let mut ws = accept(stream).expect("accept duplicate Welcome websocket");
        let frame = ws.read().expect("duplicate Welcome client frame");
        let hello = serde_json::from_str::<ClientMsg>(frame.to_text().expect("Hello text"))
            .expect("Hello message");
        tx.send(hello).expect("publish duplicate Welcome Hello");
        for token in [None, Some("second-token")] {
            let message = ServerMsg::Welcome {
                session_id: format!("duplicate-{}", token.unwrap_or("none")),
                conversation_id: "duplicate-welcome".into(),
                protocol: PROTOCOL_VERSION,
                server_version: String::new(),
                audio_input: false,
                config_protocol: 0,
                server_fingerprint: Some("fingerprint-duplicate".into()),
                loopback_trusted: false,
                token: token.map(str::to_string),
            };
            ws.send(Message::Text(
                serde_json::to_string(&message).expect("serialize duplicate Welcome"),
            ))
            .expect("send duplicate Welcome");
        }
        thread::sleep(Duration::from_millis(300));
        let _ = ws.close(None);
    });
    (format!("ws://{addr}"), rx)
}

fn start_delayed_welcome_presence_probe() -> (String, mpsc::Receiver<Option<ClientMsg>>) {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind pre-Welcome presence server");
    let addr = listener.local_addr().expect("pre-Welcome presence address");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept pre-Welcome presence ws");
        stream
            .set_read_timeout(Some(Duration::from_millis(400)))
            .expect("set pre-Welcome read timeout");
        let mut ws = accept(stream).expect("accept pre-Welcome presence websocket");
        let hello = ws.read().expect("presence probe Hello");
        assert!(matches!(
            serde_json::from_str::<ClientMsg>(hello.to_text().expect("Hello text")),
            Ok(ClientMsg::Hello { .. })
        ));
        let pre_welcome = ws.read().ok().and_then(|frame| {
            frame
                .to_text()
                .ok()
                .and_then(|text| serde_json::from_str::<ClientMsg>(text).ok())
        });
        tx.send(pre_welcome)
            .expect("publish pre-Welcome presence capture");
        ws.send(Message::Text(
            serde_json::to_string(&named_welcome(
                "presence-authenticated",
                Some("fingerprint-presence"),
            ))
            .expect("serialize presence Welcome"),
        ))
        .expect("send presence Welcome");
        let _ = ws.close(None);
    });
    (format!("ws://{addr}"), rx)
}

fn start_tcp_probe() -> (String, mpsc::Receiver<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe server");
    let addr = listener.local_addr().expect("probe address");
    listener
        .set_nonblocking(true)
        .expect("make probe listener nonblocking");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok(_) => {
                    let _ = tx.send(());
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    (format!("http://{addr}"), rx)
}

fn start_rogue_mdns_server(
    marker: PathBuf,
) -> (ServiceDaemon, mpsc::Receiver<Option<ClientMsg>>, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind rogue server");
    listener
        .set_nonblocking(true)
        .expect("make rogue listener nonblocking");
    let port = listener.local_addr().expect("rogue address").port();
    let (capture_tx, capture_rx) = mpsc::channel();
    thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let mut ws = accept(stream).expect("upgrade rogue websocket");
                    let hello = ws.read().ok().and_then(|frame| {
                        frame
                            .to_text()
                            .ok()
                            .and_then(|text| serde_json::from_str::<ClientMsg>(text).ok())
                    });
                    capture_tx.send(hello).expect("publish rogue capture");
                    let pre_marker = marker.with_extension("pre-welcome.txt");
                    ws.send(Message::Text(
                        serde_json::to_string(&ServerMsg::RunTool {
                            call_id: "rogue-pre-welcome".into(),
                            tool: "write_file".into(),
                            args_json: serde_json::json!({
                                "path": pre_marker,
                                "content": "rogue control before Welcome"
                            })
                            .to_string(),
                        })
                        .expect("serialize pre-Welcome rogue tool request"),
                    ))
                    .expect("send pre-Welcome rogue tool request");
                    ws.send(Message::Text(
                        serde_json::to_string(&welcome(Some("rogue-token")))
                            .expect("serialize rogue welcome"),
                    ))
                    .expect("send rogue welcome");
                    let post_marker = marker.with_extension("post-welcome.txt");
                    ws.send(Message::Text(
                        serde_json::to_string(&ServerMsg::RunTool {
                            call_id: "rogue-post-welcome".into(),
                            tool: "write_file".into(),
                            args_json: serde_json::json!({
                                "path": post_marker,
                                "content": "rogue control after Welcome"
                            })
                            .to_string(),
                        })
                        .expect("serialize rogue tool request"),
                    ))
                    .expect("send rogue tool request");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        return;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return,
            }
        }
    });

    let mdns = ServiceDaemon::new().expect("start rogue mDNS");
    let instance = format!("rogue-fleetyd-smoke-{}", std::process::id());
    let info = ServiceInfo::new(
        "_fleety._tcp.local.",
        &instance,
        "rogue-fleetyd-smoke.local.",
        "127.0.0.1",
        port,
        &[("fp", "copied-fingerprint")][..],
    )
    .expect("build rogue mDNS service");
    mdns.register(info).expect("advertise rogue mDNS service");
    (mdns, capture_rx, port)
}

fn rejecting_ws_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind rejecting server");
    let addr = listener.local_addr().expect("rejecting server address");
    thread::spawn(move || {
        let _ = listener.accept();
    });
    format!("ws://{addr}")
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

fn start_multi_held_ws_server(name: &'static str) -> (String, mpsc::Receiver<ClientMsg>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind multi held ws server");
    let addr = listener.local_addr().expect("multi held server address");
    let (hello_tx, hello_rx) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                return;
            };
            let hello_tx = hello_tx.clone();
            thread::spawn(move || {
                let mut ws = accept(stream).expect("upgrade multi held websocket");
                let frame = ws.read().expect("multi held server hello");
                let hello = serde_json::from_str::<ClientMsg>(frame.to_text().expect("hello text"))
                    .expect("parse multi held hello");
                hello_tx.send(hello).expect("publish multi held hello");
                ws.send(Message::Text(
                    serde_json::to_string(&named_welcome(
                        name,
                        Some(&format!("fingerprint-{name}")),
                    ))
                    .expect("serialize multi held welcome"),
                ))
                .expect("send multi held welcome");
                loop {
                    match ws.read() {
                        Ok(Message::Close(_)) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    (format!("ws://{addr}"), hello_rx)
}

fn start_rotating_token_ws_server(
    name: &'static str,
    minted_token: &'static str,
) -> (String, mpsc::Receiver<ClientMsg>) {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind rotating-token ws server");
    let addr = listener.local_addr().expect("rotating-token address");
    let (hello_tx, hello_rx) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                return;
            };
            let hello_tx = hello_tx.clone();
            thread::spawn(move || {
                let mut ws = accept(stream).expect("upgrade rotating-token websocket");
                let frame = ws.read().expect("rotating-token hello");
                let hello = serde_json::from_str::<ClientMsg>(frame.to_text().expect("hello text"))
                    .expect("parse rotating-token hello");
                hello_tx.send(hello).expect("publish rotating-token hello");
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
                        token: Some(minted_token.to_string()),
                    })
                    .expect("serialize rotating-token welcome"),
                ))
                .expect("send rotating-token welcome");
                loop {
                    match ws.read() {
                        Ok(Message::Close(_)) | Err(_) => return,
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    (format!("ws://{addr}"), hello_rx)
}

fn start_probeable_held_ws_server(
    name: &'static str,
) -> (String, mpsc::Receiver<ClientMsg>, mpsc::Receiver<bool>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probeable ws server");
    let addr = listener.local_addr().expect("probeable server address");
    let (hello_tx, hello_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut ws = loop {
            let (stream, _) = listener.accept().expect("accept probeable connection");
            match accept(stream) {
                Ok(ws) => break ws,
                Err(tokio_tungstenite::tungstenite::HandshakeError::Failure(
                    tokio_tungstenite::tungstenite::Error::Protocol(
                        tokio_tungstenite::tungstenite::error::ProtocolError::HandshakeIncomplete,
                    ),
                )) => continue,
                Err(error) => panic!("upgrade probeable websocket: {error:?}"),
            }
        };
        let frame = ws.read().expect("probeable server hello");
        let hello = serde_json::from_str::<ClientMsg>(frame.to_text().expect("hello text"))
            .expect("parse probeable hello");
        hello_tx.send(hello).expect("publish probeable hello");
        ws.send(Message::Text(
            serde_json::to_string(&named_welcome(name, Some(&format!("fingerprint-{name}"))))
                .expect("serialize probeable welcome"),
        ))
        .expect("send probeable welcome");
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

fn start_gated_ws_server(
    _name: &'static str,
) -> (
    String,
    mpsc::Receiver<ClientMsg>,
    mpsc::Sender<ServerMsg>,
    mpsc::Receiver<bool>,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind gated ws server");
    let addr = listener.local_addr().expect("gated server address");
    let (hello_tx, hello_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept gated websocket");
        let mut ws = accept(stream).expect("upgrade gated websocket");
        let frame = ws.read().expect("gated server hello");
        let hello = serde_json::from_str::<ClientMsg>(frame.to_text().expect("hello text"))
            .expect("parse gated hello");
        hello_tx.send(hello).expect("publish gated hello");
        let Ok(reply) = reply_rx.recv() else {
            return;
        };
        ws.send(Message::Text(
            serde_json::to_string(&reply).expect("serialize gated reply"),
        ))
        .expect("send gated reply");
        let closed = loop {
            match ws.read() {
                Ok(Message::Close(_)) | Err(_) => break true,
                Ok(_) => continue,
            }
        };
        let _ = closed_tx.send(closed);
    });
    (format!("ws://{addr}"), hello_rx, reply_tx, closed_rx)
}

fn named_welcome(name: &str, fingerprint: Option<&str>) -> ServerMsg {
    ServerMsg::Welcome {
        session_id: format!("session-{name}"),
        conversation_id: format!("conversation-{name}"),
        protocol: PROTOCOL_VERSION,
        server_version: String::new(),
        audio_input: false,
        config_protocol: 0,
        server_fingerprint: fingerprint.map(String::from),
        loopback_trusted: false,
        token: None,
    }
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

fn run_gated_reconnect(
    reply: Option<ServerMsg>,
    expected_fingerprint: Option<&str>,
    transport_token: Option<&str>,
) -> Output {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("gated-reconnect-{seq}"));
    let root = TempDir::new(&format!("gated-reconnect-root-{seq}"));
    let (url_a, hello_a, _closed_a) = start_held_ws_server("gated-a");
    let (url_b, hello_b, reply_b, _closed_b) = start_gated_ws_server("gated-b");
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
            fingerprint: expected_fingerprint.map(String::from),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed gated profiles");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_fleetyd"));
    daemon
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
        .stderr(Stdio::null());
    if let Some(token) = transport_token {
        daemon.env("FLEETY_TOKEN", token);
    }
    let child = daemon.spawn().expect("run fleetyd on gated profile A");
    let _child = ChildGuard(child);
    hello_a
        .recv_timeout(Duration::from_secs(15))
        .expect("gated profile A hello");
    let ready_path = home.0.join(".fleety").join("fleetyd.control-ready.json");
    wait_until(|| ready_path.exists());
    fleety_tools::connection::mutate_at(&conns_path, |live| {
        live.current = Some("B".into());
        Ok(())
    })
    .expect("switch gated profile to B");

    let (output_tx, output_rx) = mpsc::channel();
    let command_path = conns_path.clone();
    let command_home = home.0.clone();
    thread::spawn(move || {
        let output = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .args(["reconnect", "--profile", "B"])
            .env("HOME", &command_home)
            .env("USERPROFILE", &command_home)
            .env("FLEETY_CONNECTIONS", &command_path)
            .output()
            .expect("invoke gated reconnect");
        output_tx.send(output).expect("publish reconnect output");
    });
    let hello = hello_b
        .recv_timeout(Duration::from_secs(15))
        .expect("gated profile B hello");
    assert!(matches!(
        hello,
        ClientMsg::Hello { token, .. }
            if token.as_deref() == transport_token.or(Some("token-b"))
    ));
    assert!(
        output_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "reconnect must not settle before the selected Server replies"
    );
    if let Some(reply) = reply {
        reply_b.send(reply).expect("release gated Server reply");
    }
    output_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("reconnect terminal result")
}

fn run_unreachable_reconnect() -> Output {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("unreachable-reconnect-{seq}"));
    let root = TempDir::new(&format!("unreachable-reconnect-root-{seq}"));
    let (url_a, hello_a, _closed_a) = start_held_ws_server("unreachable-a");
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
            url: rejecting_ws_url(),
            token: Some("token-b".into()),
            fingerprint: Some("fingerprint-b".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed unreachable profiles");
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
    hello_a
        .recv_timeout(Duration::from_secs(15))
        .expect("profile A hello");
    let ready_path = home.0.join(".fleety").join("fleetyd.control-ready.json");
    wait_until(|| ready_path.exists());
    fleety_tools::connection::mutate_at(&conns_path, |live| {
        live.current = Some("B".into());
        Ok(())
    })
    .expect("switch unreachable profile to B");

    Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .args(["reconnect", "--profile", "B"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .output()
        .expect("invoke unreachable reconnect")
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

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
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
fn second_daemon_with_the_same_control_root_exits_before_a_second_hello() {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("single-control-owner-{seq}"));
    let root = TempDir::new(&format!("single-control-owner-root-{seq}"));
    let (url, hellos) = start_multi_held_ws_server("single-control-owner");

    let first = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start first fleetyd");
    let mut first = ChildGuard(first);
    assert!(matches!(
        hellos.recv_timeout(Duration::from_secs(5)),
        Ok(ClientMsg::Hello { .. })
    ));

    let second = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start second fleetyd");
    let mut second = ChildGuard(second);

    let status = wait_for_child_exit(&mut second.0, Duration::from_secs(3))
        .expect("second fleetyd must fail closed instead of joining the runtime");
    assert!(!status.success());
    assert!(
        hellos.recv_timeout(Duration::from_millis(300)).is_err(),
        "rejected fleetyd must send zero Hello frames"
    );
    assert!(
        first.0.try_wait().expect("poll first fleetyd").is_none(),
        "the original owner must remain connected"
    );
}

#[cfg(not(windows))]
#[test]
fn different_control_roots_cannot_share_the_service_pid_owner() {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("single-pid-owner-{seq}"));
    let root = TempDir::new(&format!("single-pid-owner-root-{seq}"));
    let connections_a = home.0.join("control-a").join("connections.toml");
    let connections_b = home.0.join("control-b").join("connections.toml");
    let (url, hellos) = start_multi_held_ws_server("single-pid-owner");

    let first = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .arg("run-service")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &connections_a)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start first service fleetyd");
    let mut first = ChildGuard(first);
    assert!(matches!(
        hellos.recv_timeout(Duration::from_secs(5)),
        Ok(ClientMsg::Hello { .. })
    ));

    let second = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .arg("run-service")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &connections_b)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start second service fleetyd");
    let mut second = ChildGuard(second);

    let status = wait_for_child_exit(&mut second.0, Duration::from_secs(3))
        .expect("second service must fail its shared pid ownership claim");
    assert!(!status.success());
    assert!(
        hellos.recv_timeout(Duration::from_millis(300)).is_err(),
        "rejected service must send zero Hello frames"
    );
    assert!(
        first.0.try_wait().expect("poll first service").is_none(),
        "the original service owner must remain connected"
    );
}

#[test]
fn unreadable_control_ready_exits_before_network_work() {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("unreadable-control-ready-{seq}"));
    let root = TempDir::new(&format!("unreadable-control-ready-root-{seq}"));
    let fleety_dir = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety_dir).expect("create Fleety home");
    let ready = fleety_dir.join("fleetyd.control-ready.json");
    std::fs::write(&ready, b"not-json").expect("seed unreadable ready owner");
    let (url, hellos) = start_multi_held_ws_server("unreadable-control-ready");

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fleetyd with unreadable ready owner");
    let mut child = ChildGuard(child);

    let status = wait_for_child_exit(&mut child.0, Duration::from_secs(3))
        .expect("unreadable ready owner must reject startup");
    assert!(!status.success());
    assert!(hellos.recv_timeout(Duration::from_millis(300)).is_err());
    assert_eq!(
        std::fs::read(&ready).expect("ready owner remains"),
        b"not-json",
        "uncertain ownership must not be replaced"
    );
}

#[cfg(unix)]
#[test]
fn permission_denied_service_pid_claim_exits_before_network_work() {
    use std::os::unix::fs::PermissionsExt;

    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("denied-service-pid-{seq}"));
    let root = TempDir::new(&format!("denied-service-pid-root-{seq}"));
    let fleety_dir = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety_dir).expect("create Fleety home");
    let pidfile = fleety_dir.join("fleetyd.pid");
    std::fs::write(&pidfile, b"4242").expect("seed service pid owner");
    std::fs::set_permissions(&pidfile, std::fs::Permissions::from_mode(0o000))
        .expect("deny service pidfile access");
    let (url, hellos) = start_multi_held_ws_server("denied-service-pid");

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .arg("run-service")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fleetyd with denied pid owner");
    let mut child = ChildGuard(child);

    let status = wait_for_child_exit(&mut child.0, Duration::from_secs(3))
        .expect("permission-denied service pid owner must reject startup");
    assert!(!status.success());
    assert!(hellos.recv_timeout(Duration::from_millis(300)).is_err());

    std::fs::set_permissions(&pidfile, std::fs::Permissions::from_mode(0o600))
        .expect("restore pidfile permissions");
    assert_eq!(
        std::fs::read_to_string(&pidfile).expect("pid owner remains"),
        "4242"
    );
}

#[cfg(unix)]
#[test]
fn unknown_service_pid_owner_exits_before_background_or_network_work() {
    use std::os::unix::fs::PermissionsExt;

    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("unknown-service-pid-{seq}"));
    let root = TempDir::new(&format!("unknown-service-pid-root-{seq}"));
    let fake_bin = home.0.join("fake-bin");
    let fleety_dir = home.0.join(".fleety");
    std::fs::create_dir_all(&fake_bin).expect("create fake probe bin");
    std::fs::create_dir_all(&fleety_dir).expect("create Fleety home");
    for command in ["kill", "ps"] {
        let path = fake_bin.join(command);
        std::fs::write(&path, "#!/bin/sh\nexit 2\n").expect("write uncertain pid probe");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make uncertain pid probe executable");
    }
    let pidfile = fleety_dir.join("fleetyd.pid");
    std::fs::write(&pidfile, b"4242").expect("seed uncertain service pid owner");
    let (url, hellos) = start_multi_held_ws_server("unknown-service-pid");
    let (update_origin, update_hits) = start_tcp_probe();
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let mut child_paths = vec![fake_bin];
    child_paths.extend(std::env::split_paths(&inherited_path));
    let path = std::env::join_paths(child_paths).expect("construct child PATH");

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .arg("run-service")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("PATH", path)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "1")
        .env(
            "FLEETY_UPDATE_MANIFEST",
            format!("{update_origin}/{{version}}.json"),
        )
        .env("FLEETY_AUTO_UPDATE", "notify")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start fleetyd with uncertain pid owner");
    let mut child = ChildGuard(child);

    let status = wait_for_child_exit(&mut child.0, Duration::from_secs(3))
        .expect("unknown service pid owner must reject startup");
    assert!(!status.success());
    assert!(hellos.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(
        update_hits
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "update poller must not start before service ownership succeeds"
    );
    assert_eq!(
        std::fs::read_to_string(&pidfile).expect("pid owner remains"),
        "4242"
    );
}

#[test]
fn automatic_mdns_never_opens_a_control_session_or_persists_rogue_credentials() {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("mdns-display-only-{seq}"));
    let root = TempDir::new(&format!("mdns-display-only-root-{seq}"));
    let marker = root.0.join("rogue-controlled.txt");
    let (mdns, capture, _port) = start_rogue_mdns_server(marker.clone());
    thread::sleep(Duration::from_millis(300));

    let mut child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_DEVICE_ID", "mdns-display-only")
        .env("FLEETY_TOKEN", "caller-secret")
        .env("FLEETY_PAIRING_CODE", "caller-pairing-code")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_MDNS_DISABLED")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run fresh fleetyd");
    thread::sleep(Duration::from_secs(4));
    let _ = child.kill();
    let output = child.wait_with_output().expect("collect fleetyd output");
    let _ = mdns.shutdown();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("automatic discovery is display-only"),
        "fleetyd must report that the LAN candidate needs explicit selection: {stderr}"
    );
    assert!(
        capture.try_recv().is_err(),
        "the rogue advertiser must not receive a WebSocket or Hello carrying \
         FLEETY_TOKEN/FLEETY_PAIRING_CODE"
    );
    assert!(
        !marker.with_extension("pre-welcome.txt").exists()
            && !marker.with_extension("post-welcome.txt").exists(),
        "pre- and post-Welcome RunTool frames must never execute"
    );
    let conns_path = home.0.join(".fleety").join("connections.toml");
    if conns_path.exists() {
        let conns =
            fleety_tools::connection::load_at(&conns_path).expect("load post-run connections");
        assert!(
            conns
                .profiles
                .values()
                .all(|profile| profile.token.as_deref() != Some("rogue-token")),
            "an unsolicited Welcome token must never be persisted"
        );
    }
}

#[test]
fn fresh_daemon_prefers_trusted_loopback_over_a_live_mdns_advertiser() {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("local-before-mdns-{seq}"));
    let root = TempDir::new(&format!("local-before-mdns-root-{seq}"));
    let marker = root.0.join("rogue-controlled.txt");
    let (mdns, rogue_capture, _rogue_port) = start_rogue_mdns_server(marker.clone());
    let (local_url, local_hello, _local_closed) = start_probeable_held_ws_server("trusted-local");
    let local_port = local_url
        .rsplit(':')
        .next()
        .expect("local port")
        .parse::<u16>()
        .expect("numeric local port");
    thread::sleep(Duration::from_millis(300));

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_DEVICE_ID", "local-before-mdns")
        .env("FLEETY_ADDR", format!("0.0.0.0:{local_port}"))
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .env_remove("FLEETY_PAIRING_CODE")
        .env_remove("FLEETY_MDNS_DISABLED")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run fresh fleetyd");
    let _child = ChildGuard(child);

    let hello = local_hello
        .recv_timeout(Duration::from_secs(10))
        .expect("fleetyd must connect to the trusted loopback server");
    assert!(matches!(hello, ClientMsg::Hello { .. }));
    thread::sleep(Duration::from_millis(300));
    assert!(
        rogue_capture.try_recv().is_err(),
        "mDNS must not run or connect when trusted loopback is available"
    );
    assert!(
        !marker.with_extension("pre-welcome.txt").exists()
            && !marker.with_extension("post-welcome.txt").exists(),
        "rogue control must remain unreachable"
    );
    let _ = mdns.shutdown();
}

#[test]
fn saved_current_profile_skips_live_mdns_and_connects_with_its_token() {
    let seq = COMMAND_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempDir::new(&format!("saved-before-mdns-{seq}"));
    let root = TempDir::new(&format!("saved-before-mdns-root-{seq}"));
    let marker = root.0.join("rogue-controlled.txt");
    let (mdns, rogue_capture, _rogue_port) = start_rogue_mdns_server(marker.clone());
    let (saved_url, saved_hello, _saved_closed) = start_held_ws_server("saved-current");
    let conns_path = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(conns_path.parent().expect("connections parent"))
        .expect("create Fleety directory");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "saved-before-mdns".into(),
        current: Some("saved".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "saved".into(),
        fleety_tools::connection::Profile {
            url: saved_url,
            token: Some("saved-token".into()),
            fingerprint: Some("fingerprint-saved-current".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed saved current profile");
    thread::sleep(Duration::from_millis(300));

    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_DEVICE_ID", "saved-before-mdns")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .env_remove("FLEETY_PAIRING_CODE")
        .env_remove("FLEETY_MDNS_DISABLED")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run fleetyd with saved current profile");
    let _child = ChildGuard(child);

    let hello = saved_hello
        .recv_timeout(Duration::from_secs(10))
        .expect("fleetyd must connect to the saved endpoint");
    assert!(matches!(
        hello,
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("saved-token")
    ));
    thread::sleep(Duration::from_millis(300));
    assert!(
        rogue_capture.try_recv().is_err(),
        "saved current endpoint must bypass discovery and the rogue advertiser"
    );
    let _ = mdns.shutdown();
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
fn daemon_session_persists_token_from_welcome_to_saved_current_profile() {
    let home = TempDir::new("session-current-profile-token");
    let root = TempDir::new("device-root");
    let (url, hello) = start_rotating_token_ws_server("current-profile", "server-token");
    let conns_path = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(conns_path.parent().expect("connections parent"))
        .expect("create fleety dir");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("default".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "default".into(),
        fleety_tools::connection::Profile {
            url,
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed current profile");

    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
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
            .expect("run fleetyd"),
    );
    let received = hello
        .recv_timeout(Duration::from_secs(15))
        .expect("saved current profile hello");

    assert!(matches!(
        received,
        ClientMsg::Hello {
            device_id,
            token: None,
            pairing_code: None,
            ..
        } if device_id == "daemon-smoke"
    ));
    // The minted token is persisted onto the exact saved current profile, so a
    // restart reconnects without re-pairing.
    wait_until(|| {
        fleety_tools::connection::load_at(&conns_path).is_ok_and(|saved| {
            saved.profiles.get("default").is_some_and(|profile| {
                profile.token.as_deref() == Some("server-token")
                    && profile.fingerprint.as_deref() == Some("fingerprint-current-profile")
            })
        })
    });
    drop(child);
}

#[test]
fn daemon_profile_welcome_commits_minted_token_and_identity_together() {
    let home = TempDir::new("session-profile-credentials");
    let root = TempDir::new("session-profile-credentials-root");
    let (url, hello) =
        start_rotating_token_ws_server("profile-credentials", "profile-minted-token");
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
            url,
            token: Some("profile-old-token".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed profile");

    let _daemon = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .env_remove("FLEETY_TOKEN")
            .env_remove("FLEETY_PAIRING_CODE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run fleetyd"),
    );
    let first = hello
        .recv_timeout(Duration::from_secs(15))
        .expect("profile hello");
    assert!(matches!(
        first,
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("profile-old-token")
    ));
    wait_until(|| {
        fleety_tools::connection::load_at(&conns_path).is_ok_and(|saved| {
            saved.profiles["A"].token.as_deref() == Some("profile-minted-token")
                && saved.profiles["A"].fingerprint.as_deref()
                    == Some("fingerprint-profile-credentials")
        })
    });
}

#[test]
fn duplicate_welcome_cannot_replace_the_authenticated_token() {
    let home = TempDir::new("duplicate-welcome");
    let root = TempDir::new("duplicate-welcome-root");
    let (url, hello) = start_duplicate_welcome_server();
    let conns_path = home.0.join(".fleety").join("connections.toml");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url,
            token: Some("old-token".into()),
            fingerprint: Some("fingerprint-duplicate".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed duplicate profile");

    let _daemon = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run duplicate Welcome fleetyd"),
    );
    assert!(matches!(
        hello
            .recv_timeout(Duration::from_secs(15))
            .expect("duplicate Welcome Hello"),
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("old-token")
    ));
    thread::sleep(Duration::from_millis(500));
    let saved =
        fleety_tools::connection::load_at(&conns_path).expect("load duplicate Welcome profile");
    assert_eq!(
        saved.profiles["A"].token.as_deref(),
        Some("old-token"),
        "a duplicate Welcome must not replace authenticated credentials"
    );
}

#[test]
fn presence_is_not_sent_before_authenticated_welcome() {
    let home = TempDir::new("pre-welcome-presence");
    let root = TempDir::new("pre-welcome-presence-root");
    let (url, pre_welcome) = start_delayed_welcome_presence_probe();
    let conns_path = home.0.join(".fleety").join("connections.toml");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url,
            token: Some("presence-token".into()),
            fingerprint: Some("fingerprint-presence".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed presence profile");

    let _daemon = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env("FLEETY_PRESENCE", "on")
            .env_remove("FLEETY_AGENT_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run presence fleetyd"),
    );
    assert!(
        pre_welcome
            .recv_timeout(Duration::from_secs(15))
            .expect("pre-Welcome presence capture")
            .is_none(),
        "presence metadata must wait for authenticated Welcome"
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
fn daemon_env_auth_rejection_explains_explicit_token_or_saved_profile_recovery() {
    let home = TempDir::new("env-auth-guidance");
    let root = TempDir::new("env-auth-guidance-root");
    let (url, rx) = start_ws_server(vec![vec![ServerMsg::Error {
        error: WireError {
            kind: "unauthenticated".into(),
            message: "bad token".into(),
            remediation: None,
        },
    }]]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_DEVICE_ID", "daemon-smoke")
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env_remove("FLEETY_TOKEN")
        .env_remove("FLEETY_PAIRING_CODE")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run fleetyd with transient environment target");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("capture transient Hello");
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello { token: None, .. })
    ));
    thread::sleep(Duration::from_millis(200));
    child.kill().expect("stop fleetyd");
    let output = child.wait_with_output().expect("collect fleetyd logs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("FLEETY_TOKEN"), "{stderr}");
    assert!(stderr.contains("unset FLEETY_AGENT_URL"), "{stderr}");
    assert!(!stderr.contains("clearing saved token"), "{stderr}");
}

#[test]
fn daemon_transient_endpoint_never_receives_a_pairing_code() {
    let home = TempDir::new("env-pairing-code-boundary");
    let root = TempDir::new("env-pairing-code-boundary-root");
    let (url, rx) = start_ws_server(vec![vec![ServerMsg::Error {
        error: WireError {
            kind: "unauthenticated".into(),
            message: "pairing rejected".into(),
            remediation: None,
        },
    }]]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env("FLEETY_PAIRING_CODE", "one-time-secret")
        .env_remove("FLEETY_TOKEN")
        .env("FLEETY_DEVICE_ID", "daemon-smoke")
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run fleetyd with a transient endpoint and pairing code");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("capture transient pairing Hello");
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            pairing_code: None,
            ..
        })
    ));
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn daemon_transient_endpoint_never_receives_server_scoped_config_token() {
    let home = TempDir::new("env-server-config-token-boundary");
    let root = TempDir::new("env-server-config-token-boundary-root");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create Fleety home");
    std::fs::write(
        fleety.join("config.toml"),
        "[server]\nFLEETY_TOKEN = \"bootstrap-admin-secret\"\n",
    )
    .expect("seed Server-owned bootstrap token");
    let (url, rx) = start_ws_server(vec![vec![ServerMsg::Error {
        error: WireError {
            kind: "unauthenticated".into(),
            message: "token absent".into(),
            remediation: None,
        },
    }]]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", &url)
        .env_remove("FLEETY_TOKEN")
        .env_remove("FLEETY_PAIRING_CODE")
        .env("FLEETY_DEVICE_ID", "daemon-smoke")
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_AUTO_INSTALL_DEPS", "0")
        .env_remove("FLEETY_UPDATE_MANIFEST")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run fleetyd with Server-owned config token");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("capture transient config-token Hello");
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello { token: None, .. })
    ));
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn daemon_env_override_never_borrows_or_mutates_same_url_profile_credentials() {
    for (case, current, explicit_token, expected_token) in [
        ("none", "B", None, None),
        ("explicit", "A", Some("caller-token"), Some("caller-token")),
    ] {
        let home = TempDir::new(&format!("same-url-env-provenance-{case}"));
        let root = TempDir::new(&format!("same-url-env-provenance-root-{case}"));
        let server_name = if case == "none" {
            "raw-env-none"
        } else {
            "raw-env-explicit"
        };
        let (url, hello_rx) = start_rotating_token_ws_server(server_name, "minted-raw-token");
        let conns_path = home.0.join(".fleety").join("connections.toml");
        std::fs::create_dir_all(conns_path.parent().expect("connections parent"))
            .expect("create Fleety home");
        std::fs::write(
            &conns_path,
            format!(
                "device_id = \"daemon-smoke\"\ncurrent = \"{current}\"\n\n\
                 [profiles.A]\nurl = \"{url}\"\ntoken = \"token-a\"\n\n\
                 [profiles.B]\nurl = \"{url}\"\ntoken = \"token-b\"\n"
            ),
        )
        .expect("seed daemon same-URL profiles");
        let before = std::fs::read(&conns_path).expect("read daemon profiles before");

        let mut command = Command::new(env!("CARGO_BIN_EXE_fleetyd"));
        command
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_DEVICE_ID", "daemon-smoke")
            .env("FLEETY_AGENT_URL", &url)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_TOKEN")
            .env_remove("FLEETY_PAIRING_CODE")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(token) = explicit_token {
            command.env("FLEETY_TOKEN", token);
        }
        let _child = ChildGuard(
            command
                .spawn()
                .expect("run fleetyd on a raw environment endpoint"),
        );
        let hello = hello_rx
            .recv_timeout(Duration::from_secs(15))
            .expect("capture fleetyd raw environment Hello");

        assert!(matches!(
            hello,
            ClientMsg::Hello { token, .. } if token.as_deref() == expected_token
        ));
        thread::sleep(Duration::from_millis(300));
        assert_eq!(
            std::fs::read(&conns_path).expect("read daemon profiles after Welcome"),
            before,
            "{case}: raw environment Welcome must not select or mutate a profile by URL"
        );
    }
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
        String::from_utf8_lossy(&reconnect.stdout).contains("authenticated the selected profile"),
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
fn reconnect_success_is_restart_ready_with_the_minted_token() {
    let home = TempDir::new("reconnect-restart-ready-token");
    let root = TempDir::new("reconnect-restart-ready-token-root");
    let (url_a, hello_a, _closed_a) = start_held_ws_server("restart-ready-a");
    let (url_b, hello_b) = start_rotating_token_ws_server("restart-ready-b", "token-b-minted");
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
            token: Some("token-b-old".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed A/B profiles");

    let spawn_daemon = || {
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
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
            .expect("spawn fleetyd")
    };
    let mut first_daemon = ChildGuard(spawn_daemon());
    hello_a
        .recv_timeout(Duration::from_secs(15))
        .expect("profile A hello");
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
        .expect("invoke daemon reconnect");
    assert!(
        reconnect.status.success(),
        "{}",
        String::from_utf8_lossy(&reconnect.stderr)
    );
    let first_b = hello_b
        .recv_timeout(Duration::from_secs(15))
        .expect("first profile B hello");
    assert!(matches!(
        first_b,
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("token-b-old")
    ));
    let persisted =
        fleety_tools::connection::load_at(&conns_path).expect("load success-visible credential");
    assert_eq!(
        persisted.profiles["B"].token.as_deref(),
        Some("token-b-minted"),
        "success is visible only after the minted token is persisted"
    );
    assert!(
        !conns_path.with_extension("toml.lock").exists(),
        "success cannot become observable before the credential owner lease is released"
    );

    first_daemon
        .0
        .kill()
        .expect("crash first daemon after success");
    first_daemon.0.wait().expect("reap first daemon");
    let _second_daemon = ChildGuard(spawn_daemon());
    let restarted = hello_b
        .recv_timeout(Duration::from_secs(15))
        .expect("restarted profile B hello");
    assert!(matches!(
        restarted,
        ClientMsg::Hello { token, .. } if token.as_deref() == Some("token-b-minted")
    ));
}

#[test]
fn daemon_reconnect_waits_for_authenticated_welcome_before_success() {
    let output = run_gated_reconnect(
        Some(named_welcome("gated-b", Some("fingerprint-gated-b"))),
        Some("fingerprint-gated-b"),
        None,
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("authenticated the selected profile"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn daemon_reconnect_keeps_explicit_transport_token_separate_from_disk_owner() {
    let output = run_gated_reconnect(
        Some(named_welcome("gated-b", Some("fingerprint-gated-b"))),
        Some("fingerprint-gated-b"),
        Some("caller-token"),
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn daemon_reconnect_rejects_authenticated_welcome_with_the_wrong_identity() {
    let output = run_gated_reconnect(
        Some(named_welcome("gated-b", Some("fingerprint-attacker"))),
        Some("fingerprint-gated-b"),
        None,
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("identity did not match"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn daemon_reconnect_settles_authentication_rejection_as_failure() {
    let output = run_gated_reconnect(
        Some(ServerMsg::Error {
            error: WireError {
                kind: "unauthenticated".into(),
                message: "token rejected".into(),
                remediation: None,
            },
        }),
        Some("fingerprint-gated-b"),
        None,
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not authenticate"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn daemon_reconnect_handshake_deadline_settles_failure_without_welcome() {
    let output = run_gated_reconnect(None, Some("fingerprint-gated-b"), None);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("handshake deadline"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn daemon_reconnect_settles_transport_connect_failure() {
    let output = run_unreachable_reconnect();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not connect"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--pairing-code <code>"),
        "fleetyd reconnect failure must direct explicit re-pair: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn daemon_reconnect_rejects_profile_switch_while_env_override_is_active() {
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
    let hello = hello_a
        .recv_timeout(Duration::from_secs(15))
        .expect("override A hello");
    assert!(matches!(hello, ClientMsg::Hello { token: None, .. }));
    let unchanged = fleety_tools::connection::load_at(&conns_path).expect("load profiles");
    assert!(unchanged.profiles["A"].fingerprint.is_none());
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
        "rejected switch must leave the raw override session intact"
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

#[test]
fn durable_profile_rejects_control_when_welcome_omits_identity() {
    let home = TempDir::new("missing-welcome-identity");
    let root = TempDir::new("missing-welcome-identity-root");
    let marker = root.0.join("must-not-run.txt");
    let (url, rx) = start_ws_server(vec![vec![
        ServerMsg::Welcome {
            session_id: "missing-identity".into(),
            conversation_id: "missing-identity".into(),
            protocol: PROTOCOL_VERSION,
            server_version: String::new(),
            audio_input: false,
            config_protocol: 0,
            server_fingerprint: None,
            loopback_trusted: false,
            token: None,
        },
        ServerMsg::RunTool {
            call_id: "must-not-run".into(),
            tool: "write_file".into(),
            args_json: serde_json::json!({
                "path": marker,
                "content": "identity bypass"
            })
            .to_string(),
        },
    ]]);
    let conns_path = home.0.join(".fleety").join("connections.toml");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url: url.clone(),
            fingerprint: Some("fingerprint-a".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed durable profile");

    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run fleetyd"),
    );
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("profile hello");
    assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
    thread::sleep(Duration::from_millis(200));
    assert!(!root.0.join("must-not-run.txt").exists());
    drop(child);
}

#[test]
fn durable_profile_rejects_control_when_welcome_identity_is_whitespace() {
    let home = TempDir::new("whitespace-welcome-identity");
    let root = TempDir::new("whitespace-welcome-identity-root");
    let marker = root.0.join("must-not-run.txt");
    let (url, rx) = start_ws_server(vec![vec![
        ServerMsg::Welcome {
            session_id: "whitespace-identity".into(),
            conversation_id: "whitespace-identity".into(),
            protocol: PROTOCOL_VERSION,
            server_version: String::new(),
            audio_input: false,
            config_protocol: 0,
            server_fingerprint: Some(" \t ".into()),
            loopback_trusted: false,
            token: None,
        },
        ServerMsg::RunTool {
            call_id: "must-not-run".into(),
            tool: "write_file".into(),
            args_json: serde_json::json!({
                "path": marker,
                "content": "whitespace identity bypass"
            })
            .to_string(),
        },
    ]]);
    let conns_path = home.0.join(".fleety").join("connections.toml");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url: url.clone(),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed durable profile");
    let before = std::fs::read(&conns_path).expect("read profile before handshake");

    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run fleetyd"),
    );
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("profile hello");
    assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
    thread::sleep(Duration::from_millis(200));
    assert!(
        !marker.exists(),
        "whitespace identity control must not execute"
    );
    assert_eq!(
        std::fs::read(&conns_path).expect("read profile after handshake"),
        before,
        "whitespace identity must not be persisted"
    );
    drop(child);
}

#[test]
fn durable_profile_rejects_control_before_authenticated_welcome() {
    let home = TempDir::new("pre-welcome-control");
    let root = TempDir::new("pre-welcome-control-root");
    let marker = root.0.join("must-not-run.txt");
    let (url, rx) = start_ws_server(vec![vec![
        ServerMsg::RunTool {
            call_id: "pre-welcome-control".into(),
            tool: "write_file".into(),
            args_json: serde_json::json!({
                "path": "must-not-run.txt",
                "content": "pre-Welcome identity bypass"
            })
            .to_string(),
        },
        named_welcome("authenticated", Some("fingerprint-a")),
    ]]);
    let conns_path = home.0.join(".fleety").join("connections.toml");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url: url.clone(),
            fingerprint: Some("fingerprint-a".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed durable profile");

    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run fleetyd"),
    );
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("profile hello");
    assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
    thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists(), "pre-Welcome control must not execute");
    drop(child);
}

#[test]
fn durable_profile_rejects_an_empty_minted_token() {
    let home = TempDir::new("empty-minted-token");
    let root = TempDir::new("empty-minted-token-root");
    let marker = root.0.join("must-not-run.txt");
    let (url, rx) = start_ws_server(vec![vec![
        ServerMsg::Welcome {
            session_id: "empty-token".into(),
            conversation_id: "empty-token".into(),
            protocol: PROTOCOL_VERSION,
            server_version: String::new(),
            audio_input: false,
            config_protocol: 0,
            server_fingerprint: Some("fingerprint-a".into()),
            loopback_trusted: false,
            token: Some(String::new()),
        },
        ServerMsg::RunTool {
            call_id: "empty-token-control".into(),
            tool: "write_file".into(),
            args_json: serde_json::json!({
                "path": "must-not-run.txt",
                "content": "empty token bypass"
            })
            .to_string(),
        },
    ]]);
    let conns_path = home.0.join(".fleety").join("connections.toml");
    let mut conns = fleety_tools::connection::Connections {
        device_id: "daemon-smoke".into(),
        current: Some("A".into()),
        ..Default::default()
    };
    conns.profiles.insert(
        "A".into(),
        fleety_tools::connection::Profile {
            url: url.clone(),
            token: Some("valid-token".into()),
            fingerprint: Some("fingerprint-a".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed durable profile");

    let child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .env("FLEETY_DEVICE_ROOT", &root.0)
            .env("FLEETY_MDNS_DISABLED", "1")
            .env_remove("FLEETY_AGENT_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run fleetyd"),
    );
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("profile hello");
    assert!(matches!(received.first(), Some(ClientMsg::Hello { .. })));
    thread::sleep(Duration::from_millis(200));
    let saved = fleety_tools::connection::load_at(&conns_path).expect("load durable profile");
    assert_eq!(
        saved.profiles["A"].token.as_deref(),
        Some("valid-token"),
        "empty minted token must not replace the valid credential"
    );
    assert!(!marker.exists(), "rejected Welcome cannot enable control");
    drop(child);
}

#[test]
fn daemon_delayed_consume_preserves_first_reconnect_and_rejects_second() {
    let home = TempDir::new("delayed-reconnect");
    let root = TempDir::new("delayed-reconnect-root");
    let blocking_command = if cfg!(windows) {
        "ping -n 9 127.0.0.1 >NUL"
    } else {
        "sleep 8"
    };
    let (url_a, _frames_a) = start_ws_server(vec![
        vec![
            named_welcome("delayed-a", Some("fingerprint-delayed-a")),
            ServerMsg::RunTool {
                call_id: "blocking-tool".into(),
                tool: "run_command".into(),
                args_json: serde_json::json!({
                    "command": blocking_command,
                    "timeout_secs": 10
                })
                .to_string(),
            },
        ],
        vec![],
    ]);
    let (url_b, hello_b, _closed_b) = start_held_ws_server("delayed-b");
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
            fingerprint: Some("fingerprint-delayed-a".into()),
            ..Default::default()
        },
    );
    conns.profiles.insert(
        "B".into(),
        fleety_tools::connection::Profile {
            url: url_b,
            fingerprint: Some("fingerprint-delayed-b".into()),
            ..Default::default()
        },
    );
    fleety_tools::connection::save_at(&conns_path, &conns).expect("seed delayed profiles");
    let child = Command::new(env!("CARGO_BIN_EXE_fleetyd"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_CONNECTIONS", &conns_path)
        .env("FLEETY_DEVICE_ID", "daemon-smoke")
        .env("FLEETY_DEVICE_ROOT", &root.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run delayed fleetyd");
    let _child = ChildGuard(child);
    let ready_path = home.0.join(".fleety").join("fleetyd.control-ready.json");
    wait_until(|| ready_path.exists());
    thread::sleep(Duration::from_millis(200));
    fleety_tools::connection::mutate_at(&conns_path, |live| {
        live.current = Some("B".into());
        Ok(())
    })
    .expect("switch delayed profile");

    let reconnect = || {
        Command::new(env!("CARGO_BIN_EXE_fleetyd"))
            .args(["reconnect", "--profile", "B"])
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_CONNECTIONS", &conns_path)
            .output()
            .expect("invoke delayed reconnect")
    };
    let first = reconnect();
    assert!(!first.status.success());
    assert!(
        String::from_utf8_lossy(&first.stderr).contains("remains durable"),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = reconnect();
    assert!(!second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("reconnect request"),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    hello_b
        .recv_timeout(Duration::from_secs(10))
        .expect("first durable request reaches profile B");
    let reconnect_journal = home
        .0
        .join(".fleety")
        .join("fleetyd.reconnect-journal.jsonl");
    wait_until(|| {
        std::fs::read_to_string(&reconnect_journal).is_ok_and(|journal| {
            journal.contains(r#""event":"settled""#) && journal.contains(r#""accepted":true"#)
        })
    });
    let settled = std::fs::read_to_string(&reconnect_journal).expect("read settled request");
    assert!(settled.contains(r#""expected_profile":"B""#), "{settled}");
}
