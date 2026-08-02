use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use fleety_protocol::{
    ClientMsg, ConfigEntry, ServerMsg, WireError, CONFIG_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use tokio_tungstenite::tungstenite::{accept, Message};

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Where `fleety acp install zed` writes settings when the fake home is `home`.
///
/// Mirrors `zed_settings_path()`: Windows resolves through `%APPDATA%`, not
/// `%USERPROFILE%`, so a test that overrides only HOME/USERPROFILE would read —
/// and on success write — the developer's real Zed settings. Every Zed test
/// must therefore also point APPDATA inside its temp home (see
/// [`zed_appdata_in`]) and assert against this path.
fn zed_settings_in(home: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) {
        home.join("AppData")
            .join("Roaming")
            .join("Zed")
            .join("settings.json")
    } else {
        home.join(".config").join("zed").join("settings.json")
    }
}

/// The APPDATA value that confines Zed-settings writes under `home`.
fn zed_appdata_in(home: &std::path::Path) -> std::path::PathBuf {
    home.join("AppData").join("Roaming")
}

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

#[test]
fn acp_install_accepts_editor_after_server_option_and_writes_the_validated_endpoint() {
    let home = TempHome::new("acp-install-order");
    let settings = zed_settings_in(&home.0);
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["acp", "install", "--server", "ws://127.0.0.1:8787", "zed"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("APPDATA", zed_appdata_in(&home.0))
        .output()
        .expect("run acp install");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(settings).expect("Zed settings written"))
            .expect("valid Zed settings");
    assert_eq!(
        value["agent_servers"]["Fleety"]["env"]["FLEETY_AGENT_URL"],
        "ws://127.0.0.1:8787"
    );
}

#[test]
fn acp_install_rejects_an_invalid_endpoint_without_touching_zed_settings() {
    let home = TempHome::new("acp-install-invalid");
    let settings = zed_settings_in(&home.0);
    std::fs::create_dir_all(settings.parent().expect("settings parent")).expect("settings dir");
    std::fs::write(&settings, b"{\"theme\":\"keep\"}").expect("settings fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["acp", "install", "zed", "--server", "not-a-websocket"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("APPDATA", zed_appdata_in(&home.0))
        .output()
        .expect("run invalid acp install");
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read(&settings).expect("unchanged settings"),
        b"{\"theme\":\"keep\"}"
    );
}

#[test]
fn acp_install_rejects_a_fragment_without_touching_zed_settings() {
    let home = TempHome::new("acp-install-fragment");
    let settings = zed_settings_in(&home.0);
    std::fs::create_dir_all(settings.parent().expect("settings parent")).expect("settings dir");
    std::fs::write(&settings, b"{\"theme\":\"keep\"}").expect("settings fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args([
            "acp",
            "install",
            "zed",
            "--server",
            "wss://example.test/ws#fragment",
        ])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("APPDATA", zed_appdata_in(&home.0))
        .output()
        .expect("run fragment acp install");
    assert!(!output.status.success());
    assert_eq!(
        std::fs::read(&settings).expect("unchanged settings"),
        b"{\"theme\":\"keep\"}"
    );
}

#[test]
fn acp_install_rejects_an_empty_settings_base() {
    let cwd = TempHome::new("acp-install-empty-home");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["acp", "install", "zed"])
        .current_dir(&cwd.0)
        .env("HOME", "")
        .env("USERPROFILE", "")
        .env("APPDATA", "")
        .output()
        .expect("run empty-home acp install");
    assert!(!output.status.success());
    assert!(!zed_settings_in(&cwd.0).exists());
}

#[test]
fn acp_install_unknown_editor_fails_with_the_generic_setup() {
    let home = TempHome::new("acp-install-generic");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["acp", "install", "neovim"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("APPDATA", zed_appdata_in(&home.0))
        .output()
        .expect("run generic acp install");
    // Naming an editor asks for an install. Nothing was installed, so exiting 0
    // told a script this had worked.
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no built-in auto-config"), "{stderr}");
    // The guidance a user needs is still right there.
    assert!(stderr.contains("point any ACP-capable editor"), "{stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).is_empty(),
        "a failure must not write to stdout"
    );
    assert!(!zed_settings_in(&home.0).exists());

    // Naming no editor is the documented "print the setup" path and still
    // succeeds on stdout.
    let printed = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["acp", "install"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("APPDATA", zed_appdata_in(&home.0))
        .output()
        .expect("run generic acp install");
    assert!(printed.status.success());
    assert!(String::from_utf8_lossy(&printed.stdout).contains("point any ACP-capable editor"));
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
    let n = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let home = TempHome::new(&format!("unreachable-{n}"));
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
        server_fingerprint: Some("fingerprint-smoke".into()),
        server_endpoints: Vec::new(),
        loopback_trusted: false,
        token: token.map(String::from),
    }
}

fn welcome_without_fingerprint(token: Option<&str>) -> ServerMsg {
    let mut reply = welcome(token);
    if let ServerMsg::Welcome {
        server_fingerprint, ..
    } = &mut reply
    {
        *server_fingerprint = None;
    }
    reply
}

fn welcome_with_fingerprint(fingerprint: &str) -> ServerMsg {
    welcome_with_fingerprint_and_token(fingerprint, None)
}

fn welcome_with_fingerprint_and_token(fingerprint: &str, token: Option<&str>) -> ServerMsg {
    ServerMsg::Welcome {
        session_id: "s1".into(),
        conversation_id: "c1".into(),
        protocol: PROTOCOL_VERSION,
        server_version: String::new(),
        audio_input: false,
        config_protocol: 0,
        server_fingerprint: Some(fingerprint.into()),
        server_endpoints: Vec::new(),
        loopback_trusted: false,
        token: token.map(String::from),
    }
}

fn doctor_welcome() -> ServerMsg {
    ServerMsg::Welcome {
        session_id: "doctor-session".into(),
        conversation_id: "doctor-conversation".into(),
        protocol: PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").into(),
        audio_input: false,
        config_protocol: CONFIG_PROTOCOL_VERSION,
        server_fingerprint: Some("doctor-server-id".into()),
        server_endpoints: Vec::new(),
        loopback_trusted: true,
        token: None,
    }
}

fn doctor_snapshot(providers_json: &str) -> ServerMsg {
    let mut providers: serde_json::Value =
        serde_json::from_str(providers_json).expect("valid Provider snapshot fixture");
    providers
        .as_object_mut()
        .expect("Provider snapshot fixture is an object")
        .insert("key_present".into(), serde_json::json!([]));
    ServerMsg::ConfigSnapshotResult {
        revision: "doctor-revision".into(),
        entries: vec![ConfigEntry {
            key: "FLEETY_MODEL".into(),
            scope: "server".into(),
            value: "gpt-5.4".into(),
            default: String::new(),
            description: "Active model".into(),
            secret: false,
            is_set: true,
            effect: None,
            choices: Vec::new(),
        }],
        providers_json: providers.to_string(),
    }
}

fn raw_provider_snapshot(providers_json: &str) -> ServerMsg {
    ServerMsg::ConfigSnapshotResult {
        revision: "provider-revision".into(),
        entries: Vec::new(),
        providers_json: providers_json.into(),
    }
}

type FakeWs = tokio_tungstenite::tungstenite::WebSocket<std::net::TcpStream>;

fn read_raw(ws: &mut FakeWs) -> Option<ClientMsg> {
    loop {
        let frame = ws.read().ok()?;
        if frame.is_close() {
            return None;
        }
        let Ok(text) = frame.to_text() else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        return serde_json::from_str::<ClientMsg>(text).ok();
    }
}

fn send_raw(ws: &mut FakeWs, msg: &ServerMsg) {
    let json = serde_json::to_string(msg).expect("server msg");
    let _ = ws.send(Message::Text(json));
}

/// Whether the client's handshake gets completed, and with which credential.
///
/// `Some((token, identity))` is a Server enrolled with this device: the
/// handshake succeeds only if the client derived the same key, which is exactly
/// what the roaming tests are asserting. `None` is a Server that understands the
/// frame but holds no credential for this device — it refuses, and the client is
/// expected to reconnect in the clear where that is still permitted.
type FakeSecure = Option<(String, String)>;

/// Outcome of the handshake phase on one fake connection.
enum FakeHandshake {
    /// Sealed from here on.
    Sealed(Box<fleety_tools::secure::SecureSession>),
    /// The client did not offer a handshake; this frame is already a control
    /// frame and belongs to the first step.
    Cleartext(Box<ClientMsg>),
    /// Refused, or the client went away. This connection carries no steps.
    Ended,
}

fn fake_handshake(ws: &mut FakeWs, secure: &FakeSecure) -> FakeHandshake {
    match read_raw(ws) {
        Some(ClientMsg::SecureHandshake { version, msg, .. }) => {
            let Some((token, fingerprint)) = secure else {
                // An older Server cannot parse the frame at all, so it just
                // drops the link; the client then reconnects in the clear.
                let _ = ws.close(None);
                return FakeHandshake::Ended;
            };
            let (responder, reply) =
                fleety_tools::secure::Responder::accept(version, token, fingerprint, &msg)
                    .expect("the fake Server holds this device's token");
            send_raw(
                ws,
                &ServerMsg::SecureAccept {
                    version,
                    msg: reply,
                },
            );
            FakeHandshake::Sealed(Box::new(
                responder.finish().expect("fake secure channel established"),
            ))
        }
        Some(other) => FakeHandshake::Cleartext(Box::new(other)),
        None => FakeHandshake::Ended,
    }
}

/// Drive one fake-Server connection through `steps`, transparently handling the
/// paired secure channel so assertions read the same either way: the frames
/// returned are always the client's control frames, unsealed.
///
/// Returns `None` when the connection ended before any step ran, which is how a
/// refused handshake reports "the client will be back in the clear".
fn serve_ws_connection(
    ws: &mut FakeWs,
    secure: &FakeSecure,
    steps: Vec<Vec<ServerMsg>>,
) -> Option<Vec<ClientMsg>> {
    let (mut session, mut pending) = match fake_handshake(ws, secure) {
        FakeHandshake::Sealed(session) => (Some(*session), None),
        FakeHandshake::Cleartext(first) => (None, Some(*first)),
        FakeHandshake::Ended => return None,
    };

    let mut received = Vec::new();
    for responses in steps {
        let message = match pending.take() {
            Some(message) => message,
            None => match read_raw(ws) {
                Some(ClientMsg::SecureFrame { payload }) => {
                    let session = session.as_mut().expect("sealed frame on a cleartext link");
                    let opened = session.open(&payload).expect("open the client's frame");
                    serde_json::from_str::<ClientMsg>(&opened).expect("client msg")
                }
                Some(message) => message,
                None => break,
            },
        };
        received.push(message);
        for response in responses {
            match session.as_mut() {
                Some(session) => {
                    let json = serde_json::to_string(&response).expect("server msg");
                    let payload = session.seal(&json).expect("seal");
                    send_raw(ws, &ServerMsg::SecureFrame { payload });
                }
                None => send_raw(ws, &response),
            }
        }
    }
    let _ = ws.close(None);
    Some(received)
}

/// A fake Server that cannot open a secure channel — an older build, or one that
/// no longer holds this device's credential. A client permitted to fall back
/// reconnects in the clear, so this accepts until a connection carries steps.
fn start_ws_server(steps: Vec<Vec<ServerMsg>>) -> (String, mpsc::Receiver<Vec<ClientMsg>>) {
    start_ws_server_with(steps, None)
}

fn start_ws_server_with(
    steps: Vec<Vec<ServerMsg>>,
    secure: FakeSecure,
) -> (String, mpsc::Receiver<Vec<ClientMsg>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ws server");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut steps = Some(steps);
        let received = loop {
            let Ok((stream, _)) = listener.accept() else {
                break Vec::new();
            };
            let Ok(mut ws) = accept(stream) else {
                break Vec::new();
            };
            let Some(pending) = steps.take() else {
                break Vec::new();
            };
            match serve_ws_connection(&mut ws, &secure, pending.clone()) {
                Some(received) => break received,
                // The handshake was refused: the client reconnects in the clear.
                None => steps = Some(pending),
            }
        };
        tx.send(received).expect("send received frames");
    });
    (format!("ws://{addr}"), rx)
}

/// A fake Server on a real, non-loopback interface — the shape a learned
/// endpoint has. `secure` is mandatory here because a learned endpoint is never
/// allowed the cleartext fallback: it has to prove the credential or be dropped.
fn start_non_loopback_ws_server(
    steps: Vec<Vec<ServerMsg>>,
    secure: FakeSecure,
) -> (String, mpsc::Receiver<Vec<ClientMsg>>) {
    let ip = if_addrs::get_if_addrs()
        .expect("enumerate test interfaces")
        .into_iter()
        .find_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
            _ => None,
        })
        .expect("a non-loopback IPv4 interface is required for endpoint roaming smoke tests");
    let listener = std::net::TcpListener::bind((ip, 0)).expect("bind non-loopback ws server");
    let addr = listener.local_addr().expect("non-loopback local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept non-loopback ws");
        let mut ws = accept(stream).expect("accept non-loopback websocket");
        let received = serve_ws_connection(&mut ws, &secure, steps).unwrap_or_default();
        tx.send(received)
            .expect("send non-loopback received frames");
    });
    (format!("ws://{addr}"), rx)
}

fn start_ws_multi_server(
    connections: Vec<Vec<Vec<ServerMsg>>>,
) -> (String, mpsc::Receiver<Vec<Vec<ClientMsg>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind multi ws server");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut all_received = Vec::new();
        for steps in connections {
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
            all_received.push(received);
        }
        tx.send(all_received).expect("send received frames");
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
    let home = TempHome::new("bare-help");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("run bare fleety");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.to_ascii_lowercase().contains("usage: fleety"));
    // The full command surface is listed, not just a teaser.
    for cmd in [
        "ask", "tui", "config", "audit", "rollback", "pair", "update",
    ] {
        assert!(stdout.contains(cmd), "help lists {cmd}");
    }
    assert!(!home.0.join(".fleety").exists());

    // `help` / `--help` / `-h` print the same thing.
    let output = run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains("usage: fleety"));
}

#[test]
fn help_and_version_do_not_create_or_migrate_user_files() {
    for arg in ["--help", "--version", "-v", "-V", "version"] {
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
fn generated_help_preserves_existing_config_bytes() {
    let home = TempHome::new("help-byte-identity");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create fleety home");
    let legacy = fleety.join("config.json");
    let config = fleety.join("config.toml");
    let connections = fleety.join("connections.toml");
    std::fs::write(&legacy, b"{\"legacy\":true}\n").expect("seed legacy");
    std::fs::write(&config, b"# config sentinel\n").expect("seed config");
    std::fs::write(&connections, b"# connections sentinel\n").expect("seed connections");
    let before = [
        std::fs::read(&legacy).expect("legacy before"),
        std::fs::read(&config).expect("config before"),
        std::fs::read(&connections).expect("connections before"),
    ];

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["provider", "status", "--help"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("run generated help");

    assert!(output.status.success());
    assert_eq!(std::fs::read(&legacy).expect("legacy after"), before[0]);
    assert_eq!(std::fs::read(&config).expect("config after"), before[1]);
    assert_eq!(
        std::fs::read(&connections).expect("connections after"),
        before[2]
    );
    assert_eq!(
        std::fs::read_dir(&fleety).expect("list fleety").count(),
        3,
        "help must not create migration or credential files"
    );
}

#[test]
fn every_command_node_supports_generated_help_without_side_effects() {
    // A leaf gets `--help` / `-h` after its path and the word form through the
    // generated `help <path...>` command. This avoids stealing a legitimate
    // positional value such as `fleety ask help` from the user.
    let command_paths: &[&[&str]] = &[
        &[],
        &["init"],
        &["ask"],
        &["resume"],
        &["tui"],
        &["chat"],
        &["conversations"],
        &["conversations", "list"],
        &["conversations", "resume"],
        &["audit"],
        &["audit", "list"],
        &["audit", "show"],
        &["rollback"],
        &["rollback", "list"],
        &["rollback", "apply"],
        &["server"],
        &["connection"],
        &["server", "add"],
        &["server", "use"],
        &["server", "list"],
        &["server", "show"],
        &["server", "current"],
        &["server", "rename"],
        &["server", "remove"],
        &["server", "set-url"],
        &["connection", "add"],
        &["connection", "use"],
        &["connection", "list"],
        &["connection", "show"],
        &["connection", "current"],
        &["connection", "rename"],
        &["connection", "remove"],
        &["connection", "set-url"],
        &["status"],
        &["doctor"],
        &["completion"],
        &["voice"],
        &["config"],
        &["config", "list"],
        &["config", "get"],
        &["config", "set"],
        &["config", "unset"],
        &["config", "open"],
        &["config", "edit"],
        &["config", "provider"],
        &["config", "model"],
        &["provider"],
        &["provider", "add"],
        &["provider", "edit"],
        &["provider", "remove"],
        &["provider", "list"],
        &["provider", "login"],
        &["provider", "logout"],
        &["provider", "status"],
        &["model"],
        &["model", "catalog"],
        &["model", "list"],
        &["model", "set"],
        &["model", "show"],
        &["model", "unset"],
        &["auth"],
        &["auth", "login"],
        &["auth", "logout"],
        &["auth", "status"],
        &["daemon"],
        &["update"],
        &["acp"],
        &["acp", "install"],
        &["pair"],
        &["pair-code"],
    ];

    for path in command_paths {
        let spellings = [
            {
                let mut args = path.to_vec();
                args.push("--help");
                args
            },
            {
                let mut args = path.to_vec();
                args.push("-h");
                args
            },
            {
                let mut args = vec!["help"];
                args.extend_from_slice(path);
                args
            },
        ];
        for args in spellings {
            let slug = if args.is_empty() {
                "root".to_string()
            } else {
                args.join("-").replace('-', "_")
            };
            let home = TempHome::new(&format!("help-{slug}"));
            let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
                .args(&args)
                .env("HOME", &home.0)
                .env("USERPROFILE", &home.0)
                .env_remove("FLEETY_AGENT_URL")
                .output()
                .expect("run help spelling");
            assert_eq!(
                output.status.code(),
                Some(0),
                "help {args:?}: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .to_ascii_lowercase()
                    .contains("usage"),
                "help {args:?} must render usage on stdout"
            );
            assert!(
                output.stderr.is_empty(),
                "help {args:?} must not write stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !home.0.join(".fleety").exists(),
                "help {args:?} must not create or migrate user files"
            );
        }
    }
}

#[test]
fn config_open_is_canonical_and_edit_is_a_side_effect_free_alias_off_tty() {
    for spelling in ["open", "edit"] {
        let help = run(&["config", spelling, "--help"]);
        assert!(
            help.status.success(),
            "{spelling}: {}",
            String::from_utf8_lossy(&help.stderr)
        );
        let stdout = String::from_utf8_lossy(&help.stdout);
        assert!(
            stdout.contains("Open the shared Settings workspace"),
            "{stdout}"
        );
    }

    let home = TempHome::new("config-open-alias-no-tty");
    let config_path = home.0.join(".fleety").join("config.toml");
    std::fs::create_dir_all(config_path.parent().expect("config parent")).expect("fleety home");
    std::fs::write(&config_path, "# sentinel\n").expect("seed config");
    let before = std::fs::read(&config_path).expect("config before");

    let mut diagnostics = Vec::new();
    for spelling in ["open", "edit"] {
        let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(["config", spelling])
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .output()
            .expect("run config workspace off TTY");
        assert_eq!(output.status.code(), Some(1));
        diagnostics.push(String::from_utf8_lossy(&output.stderr).into_owned());
        assert_eq!(std::fs::read(&config_path).expect("config after"), before);
    }
    assert_eq!(diagnostics[0], diagnostics[1]);
    assert!(diagnostics[0].contains("interactive terminal"));
    assert!(diagnostics[0].contains("shared Settings"));
}

#[test]
fn non_positional_leaves_accept_trailing_help_without_side_effects() {
    for (case, args) in [
        &["status", "help"][..],
        &["connection", "list", "help"],
        &["server", "current", "help"],
        &["config", "--owner", "server", "list", "help"],
        &["daemon", "status", "help"],
        &["-s", "ws://127.0.0.1:1", "status", "help"],
        &["-s=ws://127.0.0.1:1", "status", "help"],
        &["-sws://127.0.0.1:1", "status", "help"],
    ]
    .into_iter()
    .enumerate()
    {
        let home = TempHome::new(&format!("trailing-help-{case}"));
        let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .output()
            .expect("run trailing help");
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        assert!(output.stderr.is_empty());
        assert!(!home.0.join(".fleety").exists());
    }
}

#[test]
fn completion_powershell_is_stdout_only_and_side_effect_free() {
    let home = TempHome::new("completion-powershell");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create fleety home");
    let legacy = fleety.join("config.json");
    let config = fleety.join("config.toml");
    let connections = fleety.join("connections.toml");
    std::fs::write(&legacy, b"{\"legacy\":true}\n").expect("seed legacy");
    std::fs::write(&config, b"# config sentinel\n").expect("seed config");
    std::fs::write(&connections, b"# connections sentinel\n").expect("seed connections");
    let before = [
        std::fs::read(&legacy).expect("legacy before"),
        std::fs::read(&config).expect("config before"),
        std::fs::read(&connections).expect("connections before"),
    ];

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["completion", "powershell"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("generate PowerShell completion");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty(), "completion stderr must be empty");
    let source = String::from_utf8(output.stdout).expect("UTF-8 completion source");
    assert!(source.contains("Register-ArgumentCompleter"), "{source}");
    assert!(source.contains("fleety"), "{source}");
    assert_eq!(std::fs::read(&legacy).expect("legacy after"), before[0]);
    assert_eq!(std::fs::read(&config).expect("config after"), before[1]);
    assert_eq!(
        std::fs::read(&connections).expect("connections after"),
        before[2]
    );
    assert_eq!(std::fs::read_dir(&fleety).expect("list fleety").count(), 3);
}

#[test]
fn completion_rejects_json_without_mixing_completion_source() {
    let output = run(&["completion", "bash", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], false);
    assert!(value["errors"][0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("completion")));
}

#[test]
fn completion_supports_every_documented_shell_with_clean_streams() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = run(&["completion", shell]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty(), "{shell} completion is empty");
        assert!(output.stderr.is_empty(), "{shell} wrote diagnostics");
    }
}

#[test]
fn completion_accepts_the_standard_option_terminator() {
    let output = run(&["completion", "--", "bash"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn doctor_healthy_environment_reports_structured_passes_without_writes() {
    let providers = serde_json::json!({
        "providers": {
            "tingzhen-codex": { "type": "oauth:codex" }
        },
        "models": {
            "main": {
                "strategy": "single",
                "members": [{ "provider": "tingzhen-codex", "model": "gpt-5.4" }]
            }
        }
    })
    .to_string();
    let (url, rx) = start_ws_server(vec![
        vec![doctor_welcome()],
        vec![doctor_snapshot(&providers)],
        vec![ServerMsg::CredentialStatusResult {
            present: true,
            expires_at_secs: None,
            detail: Some("signed in".into()),
            error: None,
        }],
        vec![doctor_snapshot("{}")],
    ]);
    let home = TempHome::new("doctor-healthy");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create fleety home");
    let sentinel = fleety.join("config.json");
    let pidfile = fleety.join("fleetyd.pid");
    std::fs::write(&sentinel, b"{\"legacy\":true}\n").expect("seed legacy config");
    std::fs::write(&pidfile, std::process::id().to_string()).expect("seed daemon pid");
    let before = [
        std::fs::read(&sentinel).expect("sentinel before"),
        std::fs::read(&pidfile).expect("pid before"),
    ];

    let (output, frames) = run_against_server(&["doctor", "--json"], &url, &home, rx);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("doctor JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["context"]["owner"], "server");
    let checks = value["data"]["checks"].as_array().expect("checks array");
    for name in [
        "CLI",
        "Profile",
        "Server",
        "Config protocol",
        "Providers",
        "OAuth",
        "Active model",
        "Daemon installation",
        "Daemon connection",
    ] {
        assert!(
            checks.iter().any(|check| check["name"] == name),
            "missing {name}: {checks:?}"
        );
    }
    assert!(checks.iter().all(|check| check["status"] != "FAIL"));
    assert_eq!(frames.len(), 4);
    assert_eq!(std::fs::read(&sentinel).expect("sentinel after"), before[0]);
    assert_eq!(std::fs::read(&pidfile).expect("pid after"), before[1]);
    assert!(!fleety.join("config.json.migrated").exists());
}

#[test]
fn doctor_partial_environment_keeps_server_success_and_actionable_warnings() {
    let (url, rx) = start_ws_server(vec![
        vec![doctor_welcome()],
        vec![doctor_snapshot("{}")],
        vec![ServerMsg::Error {
            error: WireError {
                kind: "daemon_unavailable".into(),
                message: "device is offline".into(),
                remediation: Some("fleety daemon start".into()),
            },
        }],
    ]);
    let home = TempHome::new("doctor-partial");
    let (output, _) = run_against_server(&["doctor"], &url, &home, rx);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PASS  Server"), "{stdout}");
    assert!(stdout.contains("WARN  Providers"), "{stdout}");
    assert!(stdout.contains("WARN  OAuth"), "{stdout}");
    assert!(stdout.contains("WARN  Active model"), "{stdout}");
    assert!(stdout.contains("WARN  Daemon connection"), "{stdout}");
    assert!(stdout.contains("fleety daemon start"), "{stdout}");
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn doctor_offline_environment_fails_with_remediation_and_no_writes() {
    let home = TempHome::new("doctor-offline");
    let url = rejecting_ws_url();
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["doctor", "--json"])
        .env("FLEETY_AGENT_URL", url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run offline doctor");

    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("doctor JSON");
    assert_eq!(value["ok"], false);
    let server = value["data"]["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == "Server")
        .expect("server check");
    assert_eq!(server["status"], "FAIL");
    assert!(server["remediation"]
        .as_str()
        .is_some_and(|message| message.contains("fleety connection")));
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn doctor_keeps_a_pre_generation_current_profile_byte_identical() {
    let home = TempHome::new("doctor-legacy-current");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create fleety home");
    let connections = fleety.join("connections.toml");
    let url = rejecting_ws_url();
    let legacy =
        format!("current = \"home\"\n\n[profiles.home]\nurl = \"{url}\"\ntoken = \"saved\"\n");
    std::fs::write(&connections, legacy.as_bytes()).expect("seed pre-generation profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["doctor", "--json"])
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run doctor against legacy profile");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::read(&connections).expect("read unchanged profile"),
        legacy.as_bytes()
    );
}

#[test]
fn doctor_rejects_endpoint_credentials_before_network_without_echoing_them() {
    let home = TempHome::new("doctor-secret-endpoint");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args([
            "--server",
            "ws://user:SUPERSECRET@127.0.0.1:1/?token=QUERYSECRET&view=full",
            "doctor",
            "--json",
        ])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run doctor with credential-bearing URL");

    assert_eq!(output.status.code(), Some(2));
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!rendered.contains("SUPERSECRET"), "{rendered}");
    assert!(!rendered.contains("QUERYSECRET"), "{rendered}");
    assert!(!rendered.contains("user@"), "{rendered}");
    assert!(rendered.contains("valid ws:// or wss:// URL"), "{rendered}");
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn fragment_endpoint_is_rejected_before_doctor() {
    let home = TempHome::new("doctor-fragment-endpoint");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args([
            "--server",
            "wss://example.test/ws#fragment-secret",
            "doctor",
        ])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("run doctor with fragment URL");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("fragment-secret"), "{stderr}");
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn invalid_env_endpoint_is_rejected_before_legacy_migration() {
    let home = TempHome::new("invalid-env-before-migration");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("legacy fleety dir");
    let legacy = fleety.join("config.json");
    std::fs::write(
        &legacy,
        br#"{"agent_url":"ws://legacy.test:8787","token":"legacy-token"}"#,
    )
    .expect("legacy config");
    let before = std::fs::read(&legacy).expect("legacy bytes");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .arg("status")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", "wss://example.test/ws#fragment")
        .output()
        .expect("run invalid env status");

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read(&legacy).expect("legacy preserved"), before);
    assert!(!fleety.join("config.json.migrated").exists());
    assert!(!fleety.join("connections.toml").exists());
}

#[test]
fn invalid_control_bearing_endpoint_is_rejected_before_doctor() {
    let home = TempHome::new("doctor-control-endpoint");
    let raw = "ws://127.0.0.1:1/path\nforged\t\u{1b}[31m";
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["--server", raw, "doctor"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run doctor with control-bearing URL");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 usage error");
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    assert!(!stderr.contains("forged"), "{stderr:?}");
    assert!(stderr.contains("valid ws:// or wss:// URL"), "{stderr}");
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn init_invalid_url_error_redacts_secrets_and_terminal_controls_before_io() {
    let home = TempHome::new("init-hostile-url");
    let raw =
        "https://user:pass@example.test/path?token=SECRET#tail\u{1b}]52;c;STEAL\u{7}\r\nforged";
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", raw])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("run init with hostile URL");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 usage error");
    for forbidden in ["pass", "SECRET", "#tail", "\u{1b}", "\u{7}", "\r"] {
        assert!(
            !stderr.contains(forbidden),
            "leaked {forbidden:?}: {stderr:?}"
        );
    }
    assert!(stderr.contains("token=<redacted>"), "{stderr}");
    assert!(!home.0.join(".fleety").exists());

    let userinfo = "ws://user:password@example.test:8787/path?token=SECOND";
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", userinfo])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("run init with credential-bearing WebSocket URL");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 usage error");
    for forbidden in ["password", "SECOND"] {
        assert!(!stderr.contains(forbidden), "leaked {forbidden}: {stderr}");
    }
    assert!(stderr.contains("cannot contain credentials"), "{stderr}");
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn invalid_endpoint_cannot_bypass_json_secret_redaction() {
    let output = run(&[
        "--server",
        "not-a-url?token=BYPASSSECRET",
        "doctor",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let rendered = String::from_utf8(output.stdout).expect("UTF-8 usage envelope");
    assert!(!rendered.contains("BYPASSSECRET"), "{rendered}");
    let value: serde_json::Value = serde_json::from_str(&rendered).expect("usage envelope");
    assert_eq!(value["ok"], false);
    assert_eq!(value["errors"][0]["kind"], "usage");
}

#[test]
fn doctor_endpoint_keeps_unicode_path_and_redacted_query_keys() {
    let home = TempHome::new("doctor-unicode-endpoint");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args([
            "--server",
            "ws://127.0.0.1:1/路徑?tenant=一般&token=秘密",
            "doctor",
            "--json",
        ])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run doctor with Unicode endpoint");
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor JSON envelope");
    assert_eq!(
        value["context"]["endpoint"],
        "ws://127.0.0.1:1/路徑?tenant=<redacted>&token=<redacted>"
    );
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(!rendered.contains("一般"), "{rendered}");
    assert!(!rendered.contains("秘密"), "{rendered}");
}

#[test]
fn doctor_transport_errors_redact_parenthesized_and_ipv6_query_values() {
    for (name, endpoint, secrets, expected) in [
        (
            "parenthesized",
            "ws://127.0.0.1:1/path(foo)?token=SSESECRET",
            ["SSESECRET"],
            "ws://127.0.0.1:1/path(foo)?token=<redacted>",
        ),
        (
            "ipv6",
            "ws://[::1]:1/?token=SSEV6",
            ["SSEV6"],
            "ws://[::1]:1/?token=<redacted>",
        ),
    ] {
        let home = TempHome::new(&format!("doctor-{name}"));
        let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(["--server", endpoint, "doctor", "--json"])
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run doctor endpoint redaction case");
        assert_eq!(output.status.code(), Some(1), "{name}");
        let rendered = String::from_utf8(output.stdout).expect("UTF-8 doctor JSON");
        for secret in secrets {
            assert!(!rendered.contains(secret), "{name}: {rendered}");
        }
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("doctor envelope");
        assert_eq!(value["context"]["endpoint"], expected, "{name}");
    }
}

#[test]
fn unknown_command_errors_to_stderr_with_nonzero_exit() {
    let output = run(&["frobnicate"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("frobnicate"), "{stderr}");
    assert!(stderr.to_ascii_lowercase().contains("usage:"), "{stderr}");
}

#[test]
fn hostile_argv_cannot_inject_terminal_controls_through_clap_errors() {
    let hostile = "bad\u{1b}[31mFORGED\u{7}\r\nnext";
    let output = run(&[hostile]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stderr.contains(&0x1b), "{:?}", output.stderr);
    assert!(!output.stderr.contains(&0x07), "{:?}", output.stderr);
    assert!(!output.stderr.contains(&b'\r'), "{:?}", output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\\u{1b}[31mFORGED\\u{7}\\r"), "{stderr}");
}

#[test]
fn command_typo_suggests_the_canonical_command_without_side_effects() {
    let home = TempHome::new("command-suggestion");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["conection", "list"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("run typo");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connection"),
        "suggestion missing: {stderr}"
    );
    assert!(!home.0.join(".fleety").exists());
}

#[test]
fn provider_and_model_requirements_fail_before_owner_io() {
    for args in [
        &["provider", "add", "demo"][..],
        &["model", "set", "main"][..],
        &["config", "provider", "add", "demo"][..],
        &["config", "model", "set", "main"][..],
    ] {
        let home = TempHome::new(&format!("required-{}", args.join("-")));
        let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_AGENT_URL", "ws://127.0.0.1:9")
            .output()
            .expect("run incomplete provider/model command");

        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .to_ascii_lowercase()
                .contains("usage:"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!home.0.join(".fleety").exists(), "{args:?} performed I/O");
    }
}

#[test]
fn provider_and_model_mutations_report_next_connection_effect_but_queries_do_not() {
    let empty = r#"{"providers":{},"models":{},"key_present":[]}"#;
    let home = TempHome::new("provider-effect-human");
    let (url, rx) = start_ws_server(vec![
        vec![doctor_welcome()],
        vec![doctor_snapshot(empty)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: String::new(),
            effect: None,
            error: None,
        }],
    ]);
    let (human, _) = run_against_server(
        &[
            "provider",
            "add",
            "demo",
            "--type",
            "api",
            "--base-url",
            "https://example.invalid/v1",
        ],
        &url,
        &home,
        rx,
    );
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("Added Provider 'demo'"), "{stdout}");
    assert!(
        stdout.contains("takes effect on the next connection"),
        "{stdout}"
    );

    let configured = r#"{
        "providers":{"demo":{"type":"api","base_url":"https://example.invalid/v1"}},
        "models":{},
        "key_present":[]
    }"#;
    let home = TempHome::new("model-effect-json");
    let (url, rx) = start_ws_server(vec![
        vec![doctor_welcome()],
        vec![doctor_snapshot(configured)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: String::new(),
            effect: None,
            error: None,
        }],
    ]);
    let (json, _) = run_against_server(
        &[
            "--json",
            "model",
            "set",
            "main",
            "--member",
            "demo/gpt-test",
        ],
        &url,
        &home,
        rx,
    );
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("model JSON");
    assert_eq!(value["data"]["effect"], "next_connection");

    let home = TempHome::new("model-query-no-effect");
    let (url, rx) = start_ws_server(vec![
        vec![doctor_welcome()],
        vec![doctor_snapshot(configured)],
    ]);
    let (query, _) = run_against_server(&["--json", "model", "list"], &url, &home, rx);
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&query.stdout).expect("query JSON");
    assert!(
        value["data"].get("effect").is_none(),
        "query must omit effect: {value}"
    );
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
        assert!(
            stderr.to_ascii_lowercase().contains("usage:"),
            "{args:?}: {stderr}"
        );
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
fn config_owner_mismatch_fails_before_any_initialization_io() {
    let home = TempHome::new("owner-preflight");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let legacy = fleety.join("config.json");
    let legacy_bytes = br#"{"agent_url":"ws://legacy:8787"}"#;
    std::fs::write(&legacy, legacy_bytes).expect("legacy config");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["config", "--owner", "cli", "set", "FLEETY_TZ", "UTC"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("run owner mismatch");

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("owned by daemon"));
    assert_eq!(
        std::fs::read(&legacy).expect("legacy remains"),
        legacy_bytes
    );
    assert!(!fleety.join("connections.toml").exists());
    assert!(!fleety.join("config.json.migrated").exists());
}

#[test]
fn init_and_pair_write_the_connection_profile() {
    let home = TempHome::new("init-pair");
    let connections_path = home.0.join(".fleety").join("connections.toml");

    // `fleety init <url>` is sugar for `server add default <url> --use` + enroll:
    // it records the profile, makes it current, and completes the handshake.
    let (init_url, init_rx) =
        start_ws_server(vec![vec![welcome_with_fingerprint("fp-enrollment")]]);
    let (output, received) = run_against_server(&["init", &init_url], &init_url, &home, init_rx);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("registered device"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("server identity: fp-enrollment"));
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
    let pair_welcome = welcome_with_fingerprint_and_token("fp-enrollment", Some("pair-token"));
    let (pair_url, pair_rx) = start_ws_server(vec![vec![pair_welcome]]);
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
    assert!(String::from_utf8_lossy(&output.stdout).contains("server identity: fp-enrollment"));
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
fn pair_rejects_blank_tokens_and_missing_server_identity_without_mutation() {
    for (case, reply) in [
        (
            "blank-token",
            welcome_with_fingerprint_and_token("pair-fingerprint", Some(" \t ")),
        ),
        (
            "missing-fingerprint",
            welcome_without_fingerprint(Some("minted-token")),
        ),
    ] {
        let home = TempHome::new(&format!("pair-invalid-welcome-{case}"));
        let (url, rx) = start_ws_server(vec![vec![reply]]);
        let connections = home.0.join(".fleety").join("connections.toml");
        std::fs::create_dir_all(connections.parent().expect("connections parent"))
            .expect("create Fleety home");
        std::fs::write(
            &connections,
            format!(
                "current = \"office\"\n\n\
                 [profiles.office]\nurl = \"{url}\"\ntoken = \"old-token\"\nfingerprint = \"pair-fingerprint\"\ngeneration = \"pair-generation\"\n"
            ),
        )
        .expect("seed paired profile");
        let before = std::fs::read(&connections).expect("read profile before pair");

        let (output, _) = run_against_profile(&["pair", "PAIR-CODE"], &home, rx);
        assert_eq!(output.status.code(), Some(1), "{case}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("no credential was saved"),
            "{case}: {stderr}"
        );
        assert_eq!(
            std::fs::read(&connections).expect("read profile after rejected pair"),
            before,
            "{case} must not mutate the durable profile"
        );
    }
}

#[test]
fn init_with_pairing_code_requires_a_minted_token_before_persisting() {
    let home = TempHome::new("init-pairing-without-token");
    let connections_path = home.0.join(".fleety").join("connections.toml");
    let (url, rx) = start_ws_server(vec![vec![welcome_with_fingerprint("rogue-fingerprint")]]);

    let (output, received) =
        run_against_server(&["init", &url, "--pairing-code", "PAIR-1"], &url, &home, rx);

    assert_eq!(output.status.code(), Some(1));
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-1"
    ));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("did not complete pairing"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !connections_path.exists(),
        "a Welcome without a minted token must not persist the selected endpoint"
    );
}

#[test]
fn init_with_pairing_code_requires_server_identity_before_persisting() {
    let home = TempHome::new("init-pairing-without-identity");
    let connections_path = home.0.join(".fleety").join("connections.toml");
    let (url, rx) = start_ws_server(vec![vec![welcome_without_fingerprint(Some(
        "minted-token",
    ))]]);

    let (output, _) =
        run_against_server(&["init", &url, "--pairing-code", "PAIR-1"], &url, &home, rx);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("identity"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !connections_path.exists(),
        "a paired Welcome without Server identity must not persist the endpoint"
    );
}

#[test]
fn init_endpoint_change_requires_repair_and_never_sends_the_old_token() {
    let home = TempHome::new("init-endpoint-change");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        "current = \"office\"\n\n[profiles.office]\nurl = \"ws://old.test:8787\"\ntoken = \"old-secret\"\nfingerprint = \"old-pin\"\n",
    )
    .expect("write paired profile");
    let before = std::fs::read(&connections_path).expect("read baseline");

    let rejected = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", "ws://new.test:8787", "--name", "office"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("run unsafe endpoint change");
    assert_eq!(rejected.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("--pairing-code <code>"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(std::fs::read(&connections_path).unwrap(), before);

    let reply = welcome_with_fingerprint_and_token("new-pin", Some("new-token"));
    let (new_url, rx) = start_ws_server(vec![vec![reply]]);
    let (output, received) = run_against_server(
        &[
            "init",
            &new_url,
            "--name",
            "office",
            "--pairing-code",
            "PAIR-NEW",
        ],
        &new_url,
        &home,
        rx,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-NEW"
    ));
    let saved = std::fs::read_to_string(&connections_path).unwrap();
    assert!(!saved.contains("old-secret"), "{saved}");
    assert!(!saved.contains("old-pin"), "{saved}");
    assert!(saved.contains("new-token"), "{saved}");
    assert!(saved.contains("new-pin"), "{saved}");
}

#[test]
fn profile_override_can_repair_a_non_current_reselected_endpoint() {
    let home = TempHome::new("pair-non-current");
    let reply = welcome_with_fingerprint_and_token("spare-pin", Some("spare-token"));
    let (spare_url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        format!(
            "current = \"office\"\n\n[profiles.office]\nurl = \"ws://office.test:8787\"\ntoken = \"office-token\"\n\n[profiles.spare]\nurl = \"{spare_url}\"\n"
        ),
    )
    .expect("write profiles");

    let (output, received) =
        run_against_profile(&["--profile", "spare", "pair", "PAIR-SPARE"], &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-SPARE"
    ));
    let saved = std::fs::read_to_string(connections_path).unwrap();
    assert!(saved.contains("current = \"office\""), "{saved}");
    assert!(saved.contains("office-token"), "{saved}");
    assert!(saved.contains("spare-token"), "{saved}");
    assert!(saved.contains("spare-pin"), "{saved}");
}

#[test]
fn pair_same_url_replaces_old_token_and_identity_without_sending_them() {
    let home = TempHome::new("pair-same-url-rebuilt");
    let reply = welcome_with_fingerprint_and_token("new-pin", Some("new-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        format!(
            "current = \"office\"\n\n[profiles.office]\nurl = \"{url}\"\ntoken = \"old-token\"\nfingerprint = \"old-pin\"\n"
        ),
    )
    .expect("write old pairing");

    let (output, received) = run_against_profile(&["pair", "PAIR-REBUILD"], &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-REBUILD"
    ));
    let saved = std::fs::read_to_string(connections_path).expect("read repaired profile");
    assert!(!saved.contains("old-token"), "{saved}");
    assert!(!saved.contains("old-pin"), "{saved}");
    assert!(saved.contains("new-token"), "{saved}");
    assert!(saved.contains("new-pin"), "{saved}");
}

#[test]
fn named_profile_pairing_overrides_an_environment_url() {
    let home = TempHome::new("pair-named-over-env");
    let reply = welcome_with_fingerprint_and_token("office-pin", Some("office-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    std::fs::write(
        fleety.join("connections.toml"),
        format!(
            "current = \"other\"\n\n[profiles.office]\nurl = \"{url}\"\n\n\
             [profiles.other]\nurl = \"ws://127.0.0.1:9\"\n"
        ),
    )
    .expect("write profiles");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["--profile", "office", "pair", "PAIR-OFFICE"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env("FLEETY_AGENT_URL", "ws://127.0.0.1:1")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("pair named profile over env");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|error| {
            panic!(
                "named profile server received frames ({error:?}); stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-OFFICE"
    ));
}

#[test]
fn named_profile_pairing_ignores_a_malformed_environment_url() {
    let home = TempHome::new("pair-named-over-malformed-env");
    let reply = welcome_with_fingerprint_and_token("office-pin", Some("office-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    std::fs::write(
        fleety.join("connections.toml"),
        format!("current = \"office\"\n\n[profiles.office]\nurl = \"{url}\"\n"),
    )
    .expect("write profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["--profile", "office", "pair", "PAIR-OFFICE"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env("FLEETY_AGENT_URL", "not-a-websocket-url")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("pair named profile over malformed env");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or_else(|error| {
            panic!(
                "named profile server received frames ({error:?}); stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-OFFICE"
    ));
}

#[test]
fn pair_migrates_a_legacy_config_before_redeeming_the_code() {
    let home = TempHome::new("pair-migrates-legacy-config");
    let reply = welcome_with_fingerprint_and_token("new-pin", Some("new-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    std::fs::write(
        fleety.join("config.json"),
        serde_json::json!({
            "agent_url": url,
            "token": "legacy-token",
            "device_id": "legacy-device"
        })
        .to_string(),
    )
    .expect("write legacy config");

    let (output, received) = run_against_profile(&["pair", "PAIR-MIGRATED"], &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-MIGRATED"
    ));
    assert!(fleety.join("config.json.migrated").exists());
    let saved = std::fs::read_to_string(fleety.join("connections.toml"))
        .expect("read migrated connection profile");
    assert!(saved.contains("new-token"), "{saved}");
    assert!(!saved.contains("legacy-token"), "{saved}");
}

#[test]
fn pair_repairs_an_old_serializer_field_drop_without_sending_the_old_token() {
    let home = TempHome::new("pair-repairs-versioned-generation-mismatch");
    let reply = welcome_with_fingerprint_and_token("new-pin", Some("new-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        format!(
            "current = \"office\"\n\n[profiles.office]\nurl = \"{url}\"\n\
             token = \"old-token\"\nfingerprint = \"old-pin\"\n\
             configured_url = \"{url}\"\nsecure = true\n\
             generation = \"fleety-profile-v1:7:legacy-office-generation\"\n"
        ),
    )
    .expect("simulate an old serializer dropping roaming fields");

    let (output, received) = run_against_profile(&["pair", "PAIR-REPAIR"], &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-REPAIR"
    ));
    let saved =
        fleety_tools::connection::load_at(&connections_path).expect("load repaired profile");
    let office = &saved.profiles["office"];
    assert_eq!(office.token.as_deref(), Some("new-token"));
    assert_eq!(office.fingerprint.as_deref(), Some("new-pin"));
    assert_eq!(
        office.generation,
        "fleety-profile-v1:0:legacy-office-generation"
    );
}

#[test]
fn explicit_init_repairs_a_lost_configured_endpoint_without_sending_the_old_token() {
    let home = TempHome::new("init-repairs-lost-configured-endpoint");
    let reply = welcome_with_fingerprint_and_token("new-pin", Some("new-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        "current = \"other\"\n\n\
         [profiles.office]\nurl = \"ws://127.0.0.1:9\"\ntoken = \"old-token\"\n\
         fingerprint = \"old-pin\"\nsecure = true\n\
         generation = \"fleety-profile-v1:7:office-generation\"\n\n\
         [profiles.other]\nurl = \"ws://other:8787\"\ngeneration = \"other-generation\"\n",
    )
    .expect("simulate loss of the configured endpoint after roaming");

    let args = [
        "init",
        url.as_str(),
        "--name",
        "office",
        "--pairing-code",
        "PAIR-REPAIR",
    ];
    let (output, received) = run_against_profile(&args, &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-REPAIR"
    ));
    let saved =
        fleety_tools::connection::load_at(&connections_path).expect("load repaired profile");
    assert_eq!(saved.current.as_deref(), Some("office"));
    let office = &saved.profiles["office"];
    assert_eq!(office.url, url);
    assert_eq!(office.token.as_deref(), Some("new-token"));
    assert_eq!(office.fingerprint.as_deref(), Some("new-pin"));
    assert_eq!(office.generation, "fleety-profile-v1:0:office-generation");
}

#[test]
fn cli_rejects_a_downgraded_selected_profile_before_transport_use() {
    let home = TempHome::new("cli-rejects-versioned-generation-mismatch");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        "current = \"office\"\n\n[profiles.office]\nurl = \"ws://127.0.0.1:9\"\n\
         token = \"old-token\"\nsecure = true\n\
         generation = \"fleety-profile-v1:7:legacy-office-generation\"\n",
    )
    .expect("simulate an old serializer dropping roaming fields");
    let before = std::fs::read(&connections_path).expect("read seed");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .arg("status")
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .output()
        .expect("run CLI");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("profile \"office\""), "{stderr}");
    assert!(stderr.contains("update every Fleety binary"), "{stderr}");
    assert!(
        stderr.contains("fleety init <ws-url> --name <profile> --pairing-code <code>"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("fleety --profile <name> pair <code>"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read(&connections_path).expect("read unchanged file"),
        before
    );
}

#[test]
fn init_same_url_with_pairing_code_replaces_old_identity_without_sending_token() {
    let home = TempHome::new("init-same-url-rebuilt");
    let reply = welcome_with_fingerprint_and_token("new-pin", Some("new-token"));
    let (url, rx) = start_ws_server(vec![vec![reply]]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    std::fs::write(
        &connections_path,
        format!(
            "current = \"office\"\n\n[profiles.office]\nurl = \"{url}\"\ntoken = \"old-token\"\nfingerprint = \"old-pin\"\n"
        ),
    )
    .expect("write old pairing");

    let (output, received) = run_against_server(
        &[
            "init",
            &url,
            "--name",
            "office",
            "--pairing-code",
            "PAIR-REBUILD",
        ],
        &url,
        &home,
        rx,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello {
            token: None,
            pairing_code: Some(code),
            ..
        }) if code == "PAIR-REBUILD"
    ));
    let saved = std::fs::read_to_string(connections_path).expect("read repaired profile");
    assert!(!saved.contains("old-token"), "{saved}");
    assert!(!saved.contains("old-pin"), "{saved}");
    assert!(saved.contains("new-token"), "{saved}");
    assert!(saved.contains("new-pin"), "{saved}");
}

#[test]
fn init_reusing_a_paired_profile_proves_the_server_before_sending_hello() {
    let home = TempHome::new("init-existing-profile-secure-proof");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind secure init server");
    let url = format!(
        "ws://{}",
        listener.local_addr().expect("secure init address")
    );
    let (observed_tx, observed_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| -> Result<ClientMsg, String> {
            let (stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let mut ws = accept(stream).map_err(|error| error.to_string())?;
            let first = read_raw(&mut ws).ok_or_else(|| "missing first frame".to_string())?;
            let ClientMsg::SecureHandshake { version, msg, .. } = first else {
                return Err(format!(
                    "saved credential reached the transport before secure proof: {first:?}"
                ));
            };
            let (responder, reply) =
                fleety_tools::secure::Responder::accept(version, "saved-token", "server-a", &msg)
                    .map_err(|error| error.report().message)?;
            send_raw(
                &mut ws,
                &ServerMsg::SecureAccept {
                    version,
                    msg: reply,
                },
            );
            let mut session = responder.finish().map_err(|error| error.report().message)?;
            let frame = read_raw(&mut ws).ok_or_else(|| "missing sealed Hello".to_string())?;
            let ClientMsg::SecureFrame { payload } = frame else {
                return Err(format!("Hello was not sealed: {frame:?}"));
            };
            let opened = session
                .open(&payload)
                .map_err(|error| error.report().message)?;
            let hello =
                serde_json::from_str::<ClientMsg>(&opened).map_err(|error| error.to_string())?;
            let response = serde_json::to_string(&welcome_with_fingerprint("server-a"))
                .map_err(|error| error.to_string())?;
            let payload = session
                .seal(&response)
                .map_err(|error| error.report().message)?;
            send_raw(&mut ws, &ServerMsg::SecureFrame { payload });
            let _ = ws.close(None);
            Ok(hello)
        })();
        let _ = observed_tx.send(result);
    });
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create Fleety home");
    let connections = fleety.join("connections.toml");
    std::fs::write(
        &connections,
        format!(
            "device_id = \"init-secure\"\ncurrent = \"office\"\n\n\
             [profiles.office]\nurl = \"{url}\"\ntoken = \"saved-token\"\n\
             fingerprint = \"server-a\"\n\
             generation = \"fleety-profile-v1:0:init-secure-generation\"\n"
        ),
    )
    .expect("seed paired profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", &url, "--name", "office"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "init-secure")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run init against secure Server");
    let observed = observed_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("secure Server result");

    assert!(
        output.status.success(),
        "stdout={} stderr={} server={observed:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        observed,
        Ok(ClientMsg::Hello {
            token: Some(ref token),
            pairing_code: None,
            ..
        }) if token == "saved-token"
    ));
    let saved = fleety_tools::connection::load_at(&connections).expect("load secured profile");
    assert!(
        saved.profiles["office"].secure,
        "successful secure init must latch the encrypted channel"
    );
}

#[test]
fn saved_profile_failure_never_heals_or_mutates_and_directs_explicit_repair() {
    let home = TempHome::new("saved-profile-no-heal");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let connections_path = fleety.join("connections.toml");
    let unreachable = rejecting_ws_url();
    std::fs::write(
        &connections_path,
        format!(
            "current = \"office\"\n\n[profiles.office]\nurl = \"{unreachable}\"\ntoken = \"office-secret\"\nfingerprint = \"public-copyable-hint\"\ngeneration = \"fleety-profile-v1:0:failure-generation\"\n"
        ),
    )
    .expect("write paired profile");
    let before = std::fs::read(&connections_path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["status"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_MDNS_DISABLED")
        .output()
        .expect("run saved profile command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--pairing-code <code>"), "{stderr}");
    assert!(
        stderr.contains("will not send the stored token or change the URL"),
        "{stderr}"
    );
    assert_eq!(std::fs::read(&connections_path).unwrap(), before);
}

#[test]
fn init_success_sanitizes_hostile_profile_name() {
    let home = TempHome::new("init-safe-name");
    let hostile = "prod\u{1b}]52;c;owned\u{7}\r\nforged";
    let (url, rx) = start_ws_server(vec![vec![welcome_with_fingerprint("fp-safe-name")]]);
    let (output, _) = run_against_server(&["init", &url, "--name", hostile], &url, &home, rx);
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
        stdout.contains("prod\\u{1b}]52;c;owned\\u{7}\\r\\nforged"),
        "{stdout}"
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

    // Removing the current server is rejected, even with --force.
    let rm = run_srv(&["server", "remove", "home"]);
    assert!(!rm.status.success(), "remove current must require a switch");
    assert!(String::from_utf8_lossy(&rm.stderr).contains("explicitly switch"));
    let forced = run_srv(&["server", "remove", "home", "--force"]);
    assert!(
        !forced.status.success(),
        "--force must not choose a replacement"
    );
    assert!(String::from_utf8_lossy(&forced.stderr).contains("never chooses"));

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
fn connection_human_and_json_outputs_redact_legacy_secrets_and_controls() {
    let home = TempHome::new("connection-output-redaction");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create fleety home");
    std::fs::write(
        fleety.join("connections.toml"),
        "current = \"legacy\"\n\n[profiles.legacy]\nurl = \"wss://user:password@example.test/ws?token=secret#fragment\"\nlabel = \"line\\n\\u001b[2Jclear\"\nfingerprint = \"fp\\r\\nvalue\"\n",
    )
    .expect("seed legacy profile");

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .output()
            .expect("run connection output")
    };
    for args in [
        vec!["connection", "list"],
        vec!["connection", "show", "legacy"],
        vec!["--json", "connection", "show", "legacy"],
    ] {
        let output = run(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 output");
        for secret in ["user:password", "password", "token=", "secret", "fragment"] {
            assert!(
                !stdout.contains(secret),
                "{secret} leaked for {args:?}: {stdout:?}"
            );
        }
        assert!(
            !stdout.contains('\u{1b}'),
            "ESC leaked for {args:?}: {stdout:?}"
        );
        assert!(
            stdout.contains("wss://example.test/ws"),
            "redacted endpoint for {args:?}: {stdout:?}"
        );
    }
}

#[test]
fn connection_is_the_canonical_alias_of_server() {
    let home = TempHome::new("connection-alias");
    let run_profile = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .env_remove("FLEETY_CONNECTIONS")
            .output()
            .expect("run profile command")
    };

    let add = run_profile(&["connection", "add", "office", "ws://office:8787"]);
    assert!(
        add.status.success(),
        "canonical add: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let canonical = run_profile(&["connection", "list"]);
    let legacy = run_profile(&["server", "list"]);
    assert_eq!(canonical.status.code(), legacy.status.code());
    assert_eq!(canonical.stdout, legacy.stdout);
    assert_eq!(canonical.stderr, legacy.stderr);

    let switched = run_profile(&["--json", "connection", "use", "office"]);
    assert!(
        switched.status.success(),
        "json switch: {}",
        String::from_utf8_lossy(&switched.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&switched.stdout).expect("one valid JSON document");
    assert_eq!(
        payload.get("ok").and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let output = payload
        .pointer("/data/output")
        .and_then(serde_json::Value::as_str)
        .expect("connection use output");
    assert!(output.contains("now using profile 'office'"), "{output}");
    assert!(
        output.contains("no running fleetyd owns this connection store")
            || output.contains("fleetyd accepted profile"),
        "daemon owner status must be explicit: {output}"
    );
}

#[test]
fn profile_override_queries_b_without_changing_current_a() {
    let home = TempHome::new("profile-override");
    let (url, rx) = start_ws_server(vec![
        vec![welcome_with_fingerprint("server-b-fingerprint")],
        vec![ServerMsg::ServerStatusResult {
            version: "test-server".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "current = \"A\"\n\n[profiles.A]\nurl = \"ws://127.0.0.1:9\"\n\n[profiles.B]\nurl = \"{url}\"\n"
        ),
    )
    .expect("seed profiles");
    let (output, received) = run_against_profile(&["--profile", "B", "status"], &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("profile 'B'"), "{stdout}");
    assert!(stdout.contains("owner: Server"), "{stdout}");
    assert!(
        stdout.contains("server identity: server-b-fingerprint"),
        "{stdout}"
    );
    assert!(matches!(received.get(1), Some(ClientMsg::ServerStatus)));
    let after = std::fs::read_to_string(&connections).expect("connections after");
    assert!(after.contains("current = \"A\""), "{after}");
    let (a_section, b_section) = after
        .split_once("[profiles.B]")
        .expect("A and B profile sections");
    assert!(
        !a_section.contains("fingerprint"),
        "A must not be pinned: {after}"
    );
    assert!(
        b_section.contains("fingerprint = \"server-b-fingerprint\""),
        "B must receive its own TOFU pin: {after}"
    );
}

#[test]
fn saved_profile_falls_back_to_a_learned_endpoint_and_promotes_it() {
    let home = TempHome::new("profile-endpoint-roaming");
    // The learned endpoint holds this device's token, so it can complete the
    // handshake. That is the precondition for being tried at all: an endpoint
    // Fleety taught itself never gets the credential without proving it first.
    let (reachable_url, rx) = start_non_loopback_ws_server(
        vec![
            vec![welcome_with_fingerprint("server-roaming")],
            vec![ServerMsg::ServerStatusResult {
                version: "roaming-server".into(),
                uptime_secs: 1,
                connected_devices: 0,
                device_ids_json: "[]".into(),
                extra_json: None,
            }],
        ],
        Some(("roaming-token".to_string(), "server-roaming".to_string())),
    );
    let port = reachable_url
        .rsplit_once(':')
        .map(|(_, port)| port)
        .expect("reachable endpoint port");
    let old_url = format!("ws://127.0.0.1:{port}");
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "device_id = \"roaming-cli\"\ncurrent = \"home\"\n\n\
             [profiles.home]\nurl = \"{old_url}\"\nendpoints = [\"{reachable_url}\"]\n\
             token = \"roaming-token\"\nfingerprint = \"server-roaming\"\ngeneration = \"roaming-generation\"\n"
        ),
    )
    .expect("seed roaming profile");

    let (output, received) = run_against_profile(&["status"], &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello { token, .. }) if token.as_deref() == Some("roaming-token")
    ));
    let saved = fleety_tools::connection::load_at(&connections).expect("load promoted CLI profile");
    assert_eq!(saved.profiles["home"].url, reachable_url);
    assert_eq!(saved.profiles["home"].endpoints, vec![old_url]);
    // The fake Server only answers a handshake keyed by this exact token and
    // identity, so the pin landing here is proof the session really was sealed
    // — not merely that a connection was made.
    assert!(
        saved.profiles["home"].secure,
        "roaming onto a learned endpoint must run inside the encrypted channel"
    );
}

/// A latched profile must not have its saved token replayed over a plain
/// connection — and `init` is exactly the command the "could not prove it is
/// the Server" guidance sends people to, so it is the path an attacker who took
/// over the address would steer them onto.
/// Low 2: the profile returning to the configured address is covered, but the
/// half the spec actually names — where the one-time code is *sent* — was not.
/// A regression here hands a pairing code, in the clear, to an address the user
/// never chose.
#[test]
fn pair_sends_its_code_to_the_configured_address_not_the_roamed_one() {
    let home = TempHome::new("pair-configured-address");
    // The address the user configured answers; the roamed primary is a dead
    // port, so reaching it at all would be visible as a failure.
    let (configured_url, rx) = start_ws_server(vec![vec![ServerMsg::Welcome {
        session_id: "paired".into(),
        conversation_id: "paired".into(),
        protocol: PROTOCOL_VERSION,
        server_version: String::new(),
        audio_input: false,
        config_protocol: CONFIG_PROTOCOL_VERSION,
        server_fingerprint: Some("server-paired".into()),
        server_endpoints: Vec::new(),
        loopback_trusted: false,
        token: Some("minted-token".into()),
    }]]);
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "device_id = \"pair-configured\"\ncurrent = \"home\"\n\n\
             [profiles.home]\nurl = \"ws://127.0.0.1:9\"\n\
             configured_url = \"{configured_url}\"\ntoken = \"old-token\"\n\
             fingerprint = \"server-paired\"\ngeneration = \"pair-generation\"\n"
        ),
    )
    .expect("seed roamed profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["pair", "abc12345"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run fleety pair");

    let received = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default();
    assert!(
        matches!(
            received.first(),
            Some(ClientMsg::Hello { pairing_code, token, .. })
                if pairing_code.as_deref() == Some("abc12345") && token.is_none()
        ),
        "the configured address must receive the code, and no old token: {received:?}"
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let saved = fleety_tools::connection::load_at(&connections).expect("reload profile");
    assert_eq!(
        saved.profiles["home"].url, configured_url,
        "the profile must end up on the address the pairing happened at"
    );
}

/// `fleety … | head` is an ordinary thing to type. Rust masks SIGPIPE, so the
/// print macros panic on the closed pipe — a crash, exit 101, and a backtrace
/// for something the user did nothing wrong in.
/// `--help` is the only place a user can discover what a command takes, so an
/// argument printed without a description teaches nothing — and two of them
/// delete a credential or refuse to pick a replacement.
#[test]
fn every_command_argument_carries_a_description_in_help() {
    for path in [
        &["init"][..],
        &["pair"][..],
        &["resume"][..],
        &["connection", "add"][..],
        &["connection", "remove"][..],
        &["connection", "set-url"][..],
        &["provider", "add"][..],
        &["provider", "login"][..],
        &["model", "set"][..],
        &["completion"][..],
    ] {
        let mut args = path.to_vec();
        args.push("--help");
        let output = run(&args);
        assert!(output.status.success(), "{path:?} --help failed");
        let help = String::from_utf8_lossy(&output.stdout);

        // An argument line with nothing after the value placeholder is a
        // declaration whose description was never written.
        for line in help.lines() {
            let trimmed = line.trim_end();
            let is_argument = trimmed.starts_with("  <")
                || trimmed.starts_with("  [")
                || trimmed.starts_with("      --")
                || trimmed.starts_with("  -");
            if !is_argument || trimmed.contains("--help") || trimmed.contains("--version") {
                continue;
            }
            let described = trimmed
                .split_once("  ")
                .map(|(_, rest)| rest.trim_start().len() > 2)
                .unwrap_or(false)
                || trimmed.split_whitespace().count() > 2;
            assert!(
                described,
                "{path:?} --help has an undescribed argument: {trimmed:?}"
            );
        }
    }
}

#[test]
fn a_closed_pipe_ends_quietly_instead_of_panicking() {
    for args in [&["config", "list"][..], &["completion", "zsh"][..]] {
        let home = TempHome::new("broken-pipe");
        let mut child = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("run fleety");
        // Drop the read end immediately: the next write hits a closed pipe.
        drop(child.stdout.take());
        let output = child.wait_with_output().expect("await fleety");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked"),
            "{args:?} panicked on a closed pipe: {stderr}"
        );
        assert_ne!(
            output.status.code(),
            Some(101),
            "{args:?} exited as a panic on a closed pipe"
        );
    }
}

#[test]
fn init_reuses_a_latched_profile_only_through_its_secure_channel() {
    let home = TempHome::new("init-latched");
    let (url, rx) = start_ws_server_with(
        vec![vec![welcome_with_fingerprint("server-a")]],
        Some(("latched-token".into(), "server-a".into())),
    );
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "device_id = \"init-latched\"\ncurrent = \"home\"\n\n\
             [profiles.home]\nurl = \"{url}\"\ntoken = \"latched-token\"\n\
             fingerprint = \"server-a\"\nsecure = true\n\
             generation = \"fleety-profile-v1:4:latched-generation\"\n"
        ),
    )
    .expect("seed latched profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", &url, "--name", "home"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run fleety init");

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("latched secure Server requests");
    assert!(matches!(
        received.as_slice(),
        [ClientMsg::Hello {
            token: Some(token),
            pairing_code: None,
            ..
        }] if token == "latched-token"
    ));
    let saved = fleety_tools::connection::load_at(&connections).expect("load latched profile");
    assert_eq!(
        saved.profiles["home"].token.as_deref(),
        Some("latched-token")
    );
    assert_eq!(
        saved.profiles["home"].fingerprint.as_deref(),
        Some("server-a")
    );
    assert!(saved.profiles["home"].secure);
}

#[test]
fn init_never_drops_a_tokenless_profiles_secure_latch_or_owner() {
    let home = TempHome::new("init-tokenless-latched");
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind tokenless transport sentinel");
    listener
        .set_nonblocking(true)
        .expect("make transport sentinel nonblocking");
    let url = format!(
        "ws://{}",
        listener.local_addr().expect("transport sentinel address")
    );
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "device_id = \"init-tokenless-latched\"\ncurrent = \"home\"\n\n\
             [profiles.home]\nurl = \"{url}\"\nfingerprint = \"server-a\"\n\
             secure = true\ngeneration = \"fleety-profile-v1:4:tokenless-generation\"\n"
        ),
    )
    .expect("seed tokenless latched profile");
    let before = std::fs::read(&connections).expect("read tokenless profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", &url, "--name", "home"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run tokenless latched init");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires an encrypted channel"), "{stderr}");
    assert!(stderr.contains("--pairing-code"), "{stderr}");
    assert!(
        matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "a tokenless latched profile must fail before any transport opens"
    );
    assert_eq!(
        std::fs::read(&connections).expect("read unchanged tokenless profile"),
        before
    );
}

#[test]
fn init_requires_repair_before_moving_a_tokenless_secure_profile_to_another_url() {
    let home = TempHome::new("init-tokenless-latched-new-url");
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind new-url transport sentinel");
    listener
        .set_nonblocking(true)
        .expect("make new-url sentinel nonblocking");
    let new_url = format!(
        "ws://{}",
        listener.local_addr().expect("new-url sentinel address")
    );
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        "device_id = \"init-tokenless-latched-new-url\"\ncurrent = \"home\"\n\n\
         [profiles.home]\nurl = \"ws://old.example:8787\"\nfingerprint = \"server-a\"\n\
         secure = true\ngeneration = \"fleety-profile-v1:4:tokenless-new-url-generation\"\n",
    )
    .expect("seed tokenless latched profile on the old URL");
    let before = std::fs::read(&connections).expect("read protected profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["init", &new_url, "--name", "home"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_MDNS_DISABLED", "1")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run different-URL init");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--pairing-code"), "{stderr}");
    assert!(
        matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "a protected profile must reject a different URL before transport"
    );
    assert_eq!(
        std::fs::read(&connections).expect("read unchanged protected profile"),
        before
    );
}

#[test]
fn durable_profile_rejects_unusable_server_identity_before_control() {
    for (case, reply) in [
        ("missing", welcome_without_fingerprint(None)),
        ("whitespace", welcome_with_fingerprint(" \t ")),
        ("mismatch", welcome_with_fingerprint("fp-attacker")),
    ] {
        let home = TempHome::new(&format!("durable-identity-{case}"));
        let (url, rx) = start_ws_server(vec![
            vec![reply],
            vec![ServerMsg::ServerStatusResult {
                version: "must-not-be-requested".into(),
                uptime_secs: 1,
                connected_devices: 0,
                device_ids_json: "[]".into(),
                extra_json: None,
            }],
        ]);
        let connections = home.0.join(".fleety").join("connections.toml");
        std::fs::create_dir_all(connections.parent().expect("connections parent"))
            .expect("create Fleety home");
        std::fs::write(
            &connections,
            format!(
                "device_id = \"identity-test\"\ncurrent = \"A\"\n\n\
                 [profiles.A]\nurl = \"{url}\"\ntoken = \"token-a\"\nfingerprint = \"fp-a\"\ngeneration = \"fleety-profile-v1:0:identity-generation\"\n"
            ),
        )
        .expect("seed pinned profile");
        let before = std::fs::read(&connections).expect("read profile before handshake");

        let (output, received) = run_against_profile(&["status"], &home, rx);

        assert!(!output.status.success(), "{case} identity was accepted");
        assert!(
            matches!(received.as_slice(), [ClientMsg::Hello { .. }]),
            "{case} identity received post-Welcome control: {received:?}"
        );
        assert_eq!(
            std::fs::read(&connections).expect("read profile after handshake"),
            before,
            "{case} identity mutated the durable owner"
        );
    }
}

#[test]
fn status_json_is_one_semantic_secret_free_envelope() {
    let home = TempHome::new("status-json");
    let (url, rx) = start_ws_server(vec![
        vec![welcome_with_fingerprint("server-json-fp")],
        vec![ServerMsg::ServerStatusResult {
            version: "server-json-version".into(),
            uptime_secs: 65,
            connected_devices: 1,
            device_ids_json: "[\"device-1\"]".into(),
            extra_json: None,
        }],
    ]);
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "current = \"A\"\n\n[profiles.A]\nurl = \"ws://127.0.0.1:9\"\n\n[profiles.B]\nurl = \"{url}\"\ntoken = \"json-secret-token\"\n"
        ),
    )
    .expect("seed profiles");

    let (output, received) =
        run_against_profile(&["status", "--profile", "B", "--json"], &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON value on stdout");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], true);
    assert_eq!(value["context"]["profile"], "B");
    assert_eq!(value["context"]["owner"], "server");
    assert_eq!(value["context"]["endpoint"], url);
    assert_eq!(value["context"]["server_identity"], "server-json-fp");
    assert_eq!(value["data"]["server"]["version"], "server-json-version");
    assert_eq!(value["data"]["server"]["connected_devices"], 1);
    assert_eq!(value["data"]["server"]["device_ids"][0], "device-1");
    assert_eq!(value["errors"], serde_json::json!([]));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("json-secret-token"));
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello { token: Some(token), .. }) if token == "json-secret-token"
    ));
}

#[test]
fn raw_server_and_url_overrides_send_only_the_explicit_token() {
    for (case, current, flag, explicit_token, expected_token) in [
        ("server-none", "A", "--server", None, None),
        ("url-none", "B", "--url", None, None),
        ("server-empty", "A", "--server", Some(""), None),
        (
            "server-explicit",
            "B",
            "--server",
            Some("caller-token"),
            Some("caller-token"),
        ),
    ] {
        let home = TempHome::new(&format!("raw-provenance-{case}"));
        let (url, rx) = start_ws_server(vec![
            vec![welcome_with_fingerprint("raw-server")],
            vec![ServerMsg::ServerStatusResult {
                version: "raw-version".into(),
                uptime_secs: 1,
                connected_devices: 0,
                device_ids_json: "[]".into(),
                extra_json: None,
            }],
        ]);
        let connections = home.0.join(".fleety").join("connections.toml");
        std::fs::create_dir_all(connections.parent().expect("connections parent"))
            .expect("create Fleety home");
        std::fs::write(
            &connections,
            format!(
                "current = \"{current}\"\n\n\
                 [profiles.A]\nurl = \"{url}\"\ntoken = \"token-a\"\n\n\
                 [profiles.B]\nurl = \"{url}\"\ntoken = \"token-b\"\n"
            ),
        )
        .expect("seed same-URL profiles");
        let before = std::fs::read(&connections).expect("read profiles before raw command");

        let mut command = Command::new(env!("CARGO_BIN_EXE_fleety"));
        command
            .args(["status", flag, url.as_str()])
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_DEVICE_ID", "raw-provenance")
            .env_remove("FLEETY_AGENT_URL")
            .env_remove("FLEETY_TOKEN")
            .stdin(std::process::Stdio::null());
        if let Some(token) = explicit_token {
            command.env("FLEETY_TOKEN", token);
        }
        let output = command.output().expect("run raw override status");
        let received = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("capture raw override frames");

        assert!(
            output.status.success(),
            "{}: {}",
            case,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(matches!(
            received.first(),
            Some(ClientMsg::Hello { token, .. }) if token.as_deref() == expected_token
        ));
        assert_eq!(
            std::fs::read(&connections).expect("read profiles after raw command"),
            before,
            "raw override must not pin or otherwise mutate a URL-matched profile"
        );
    }
}

#[test]
fn raw_override_never_sends_a_server_token_loaded_from_config() {
    let home = TempHome::new("raw-config-token-provenance");
    let (url, rx) = start_ws_server(vec![
        vec![welcome_with_fingerprint("raw-server")],
        vec![ServerMsg::ServerStatusResult {
            version: "raw-version".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("create Fleety home");
    std::fs::write(
        fleety.join("config.toml"),
        "[server]\nFLEETY_TOKEN = \"bootstrap-admin-secret\"\n",
    )
    .expect("seed Server-owned bootstrap token");

    let mut command = Command::new(env!("CARGO_BIN_EXE_fleety"));
    let output = command
        .args(["status", "--server", url.as_str()])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "raw-config-token-provenance")
        .env_remove("FLEETY_AGENT_URL")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run raw override status");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("capture raw override frames");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        matches!(received.first(), Some(ClientMsg::Hello { token: None, .. })),
        "a persisted Server bootstrap token must never become caller-explicit client input"
    );
}

#[test]
fn raw_auth_rejection_explains_the_transient_credential_boundary() {
    let home = TempHome::new("raw-auth-guidance");
    let (url, rx) = start_ws_server(vec![vec![ServerMsg::Error {
        error: WireError {
            kind: "unauthenticated".into(),
            message: "token rejected".into(),
            remediation: None,
        },
    }]]);
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("connections parent"))
        .expect("create Fleety home");
    std::fs::write(
        &connections,
        format!(
            "current = \"A\"\n\n\
             [profiles.A]\nurl = \"{url}\"\ntoken = \"saved-token\"\n"
        ),
    )
    .expect("seed same-URL saved profile");
    let before = std::fs::read(&connections).expect("read profiles before rejection");

    let (output, received) = run_against_profile(&["status", "--server", url.as_str()], &home, rx);
    assert!(!output.status.success());
    assert!(matches!(
        received.first(),
        Some(ClientMsg::Hello { token: None, .. })
    ));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("FLEETY_TOKEN"), "{stderr}");
    assert!(stderr.contains("fleety init <ws-url>"), "{stderr}");
    assert!(!stderr.contains("`fleety pair <code>`"), "{stderr}");
    assert_eq!(
        std::fs::read(&connections).expect("read profiles after rejection"),
        before
    );
}

#[test]
fn generic_acp_install_uses_the_saved_current_profile_by_default() {
    let home = TempHome::new("acp-install-current-profile");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["acp", "install"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .output()
        .expect("print generic ACP setup");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("FLEETY_AGENT_URL="), "{stdout}");
    assert!(stdout.contains("saved current profile"), "{stdout}");
}

#[test]
fn acp_environment_endpoint_does_not_borrow_a_same_url_profile_token() {
    for (case, current, explicit_token, expected_token) in [
        ("none", "A", None, None),
        ("explicit", "B", Some("caller-token"), Some("caller-token")),
    ] {
        let home = TempHome::new(&format!("acp-raw-provenance-{case}"));
        let (url, rx) = start_ws_server(vec![
            vec![welcome_with_fingerprint("acp-transient")],
            vec![ServerMsg::Done {
                conversation_id: "acp-transient".into(),
            }],
        ]);
        let connections = home.0.join(".fleety").join("connections.toml");
        std::fs::create_dir_all(connections.parent().expect("connections parent"))
            .expect("create Fleety home");
        std::fs::write(
            &connections,
            format!(
                "current = \"{current}\"\n\n\
                 [profiles.A]\nurl = \"{url}\"\ntoken = \"token-a\"\n\n\
                 [profiles.B]\nurl = \"{url}\"\ntoken = \"token-b\"\n"
            ),
        )
        .expect("seed ACP same-URL profiles");
        let before = std::fs::read(&connections).expect("read ACP profiles before");

        let mut command = Command::new(env!("CARGO_BIN_EXE_fleety"));
        command
            .arg("acp")
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env("FLEETY_AGENT_URL", &url)
            .env_remove("FLEETY_TOKEN")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(token) = explicit_token {
            command.env("FLEETY_TOKEN", token);
        }
        let mut child = command.spawn().expect("run ACP adapter");
        let mut stdin = child.stdin.take().expect("ACP stdin");
        std::io::Write::write_all(
            &mut stdin,
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"acp-transient\",\"prompt\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
        )
        .expect("write ACP prompt");
        drop(stdin);
        let output = child.wait_with_output().expect("collect ACP output");
        let received = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("capture ACP frames");

        assert!(
            output.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(matches!(
            received.first(),
            Some(ClientMsg::Hello { token, .. }) if token.as_deref() == expected_token
        ));
        assert_eq!(
            std::fs::read(&connections).expect("read ACP profiles after"),
            before,
            "{case}"
        );
    }
}

#[test]
fn json_usage_error_uses_the_envelope_and_exit_two() {
    let output = run(&["status", "extra", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("usage JSON envelope");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["errors"][0]["owner"], "cli");
    assert_eq!(value["errors"][0]["kind"], "usage");
}

#[test]
fn local_config_json_identifies_cli_owner() {
    let output = run(&[
        "config",
        "--owner",
        "cli",
        "get",
        "FLEETY_VOICE_AUDIO",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("config envelope");
    assert_eq!(value["context"]["owner"], "cli");
    assert_eq!(value["context"]["source"], "local");
    assert!(value["data"]["output"]
        .as_str()
        .is_some_and(|output| output.contains("FLEETY_VOICE_AUDIO")));
}

#[test]
fn config_list_json_preserves_available_owners_on_partial_failure() {
    let home = TempHome::new("config-partial-json");
    let daemon_error = ServerMsg::ConfigResult {
        ok: false,
        output: String::new(),
        effect: None,
        error: Some(WireError {
            kind: "unavailable".into(),
            message: "daemon is offline".into(),
            remediation: Some("run `fleetyd start`".into()),
        }),
    };
    let server_result = ServerMsg::ConfigResult {
        ok: true,
        output: "server settings available".into(),
        effect: None,
        error: None,
    };
    let (url, rx) = start_ws_multi_server(vec![
        vec![vec![welcome(None)], vec![daemon_error]],
        vec![vec![welcome(None)], vec![server_result]],
    ]);

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["config", "list", "--json"])
        .env("FLEETY_AGENT_URL", &url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run partial config list");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("both owner requests");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one partial JSON envelope");
    assert_eq!(value["ok"], false);
    assert!(value["data"]["cli"].is_object());
    assert_eq!(
        value["data"]["server"]["output"],
        "server settings available"
    );
    assert_eq!(value["errors"][0]["owner"], "daemon");
    assert_eq!(value["errors"][0]["kind"], "unavailable");
    assert_eq!(value["errors"][0]["remediation"], "run `fleetyd start`");
    assert_eq!(
        received.len(),
        2,
        "Daemon failure must not skip Server read"
    );
    assert!(matches!(
        received[0].get(1),
        Some(ClientMsg::ConfigExec {
            target: fleety_protocol::ConfigTarget::Device(id),
            ..
        }) if id == "cli-smoke"
    ));
    assert!(matches!(
        received[1].get(1),
        Some(ClientMsg::ConfigExec {
            target: fleety_protocol::ConfigTarget::Server,
            ..
        })
    ));
}

#[test]
fn config_list_human_marks_partial_and_keeps_non_owner_files_unchanged() {
    let home = TempHome::new("config-partial-human");
    let fleety = home.0.join(".fleety");
    std::fs::create_dir_all(&fleety).expect("fleety dir");
    let local_config = fleety.join("config.toml");
    let providers = fleety.join("providers.toml");
    std::fs::write(&local_config, "[cli]\nFLEETY_VOICE_AUDIO = \"auto\"\n").expect("local config");
    std::fs::write(&providers, "# provider sentinel\n").expect("providers sentinel");
    let config_before = std::fs::read(&local_config).expect("config before");
    let providers_before = std::fs::read(&providers).expect("providers before");

    let daemon_error = ServerMsg::ConfigResult {
        ok: false,
        output: String::new(),
        effect: None,
        error: Some(WireError {
            kind: "unavailable".into(),
            message: "daemon is offline".into(),
            remediation: Some("run `fleetyd start`".into()),
        }),
    };
    let server_result = ServerMsg::ConfigResult {
        ok: true,
        output: "server settings available".into(),
        effect: None,
        error: None,
    };
    let (url, rx) = start_ws_multi_server(vec![
        vec![vec![welcome(None)], vec![daemon_error]],
        vec![vec![welcome(None)], vec![server_result]],
    ]);

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["config", "list"])
        .env("FLEETY_AGENT_URL", &url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run partial human config list");
    let received = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("both owner requests");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("CLI settings:"), "{stdout}");
    assert!(stdout.contains("server settings available"), "{stdout}");
    assert!(
        stderr.contains("UNAVAILABLE: daemon is offline"),
        "{stderr}"
    );
    assert!(stderr.contains("run `fleetyd start`"), "{stderr}");
    assert!(stderr.contains("PARTIAL:"), "{stderr}");
    assert_eq!(received.len(), 2);
    assert_eq!(
        std::fs::read(local_config).expect("config after"),
        config_before
    );
    assert_eq!(
        std::fs::read(providers).expect("providers after"),
        providers_before
    );
}

#[test]
fn config_list_quiet_prints_only_owner_values_without_headings_or_effects() {
    let home = TempHome::new("config-list-quiet");
    let result = |output: &str| ServerMsg::ConfigResult {
        ok: true,
        output: output.to_string(),
        effect: Some(fleety_protocol::Effect::Restart),
        error: None,
    };
    let (url, rx) = start_ws_multi_server(vec![
        vec![vec![welcome(None)], vec![result("daemon-value")]],
        vec![vec![welcome(None)], vec![result("server-value")]],
    ]);

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["--quiet", "config", "list"])
        .env("FLEETY_AGENT_URL", &url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run quiet config list");
    rx.recv_timeout(Duration::from_secs(15))
        .expect("both owner requests");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("daemon-value"), "{stdout}");
    assert!(stdout.contains("server-value"), "{stdout}");
    assert!(!stdout.contains("CLI settings:"), "{stdout}");
    assert!(!stdout.contains("Daemon settings:"), "{stdout}");
    assert!(!stdout.contains("Server settings:"), "{stdout}");
    assert!(!stdout.contains("takes effect"), "{stdout}");
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn config_list_quiet_partial_keeps_values_and_actionable_error_without_partial_prose() {
    let home = TempHome::new("config-list-quiet-partial");
    let daemon_error = ServerMsg::ConfigResult {
        ok: false,
        output: String::new(),
        effect: None,
        error: Some(WireError {
            kind: "unavailable".into(),
            message: "daemon is offline".into(),
            remediation: Some("run `fleetyd start`".into()),
        }),
    };
    let server_result = ServerMsg::ConfigResult {
        ok: true,
        output: "server-value".into(),
        effect: None,
        error: None,
    };
    let (url, rx) = start_ws_multi_server(vec![
        vec![vec![welcome(None)], vec![daemon_error]],
        vec![vec![welcome(None)], vec![server_result]],
    ]);

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["--quiet", "config", "list"])
        .env("FLEETY_AGENT_URL", &url)
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_DEVICE_ID", "cli-smoke")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run quiet partial config list");
    rx.recv_timeout(Duration::from_secs(15))
        .expect("both owner requests");

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("server-value"), "{stdout}");
    assert!(!stdout.contains("settings:"), "{stdout}");
    assert!(stderr.contains("daemon is offline"), "{stderr}");
    assert!(stderr.contains("run `fleetyd start`"), "{stderr}");
    assert!(!stderr.contains("PARTIAL:"), "{stderr}");
}

#[test]
fn quiet_status_keeps_results_and_suppresses_context_prose() {
    let home = TempHome::new("status-quiet");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ServerStatusResult {
            version: "quiet-server".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);

    let (output, _) = run_against_server(&["--quiet", "status"], &url, &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("quiet-server"), "{stdout}");
    assert!(!stdout.contains("context:"), "{stdout}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("note:"));
}

#[test]
fn help_documents_global_output_modes_without_ansi() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in ["--json", "--quiet", "--no-color", "--warnings"] {
        assert!(stdout.contains(flag), "help missing {flag}: {stdout}");
    }
    assert!(!stdout.contains('\u{1b}'));
}

#[test]
fn no_color_and_quiet_keep_failure_streams_script_safe() {
    let no_color = run_with_rejecting_agent(&["status", "--no-color"]);
    assert_eq!(no_color.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&no_color.stderr).contains('\u{1b}'));

    let quiet = run_with_rejecting_agent(&["status", "--quiet"]);
    assert_eq!(quiet.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(stderr.contains("error:"), "{stderr}");
    assert!(!stderr.contains(" WARN "), "{stderr}");
    assert!(!stderr.contains("note:"), "{stderr}");
    assert!(!String::from_utf8_lossy(&quiet.stdout).contains("context:"));
}

#[test]
fn quiet_connection_list_suppresses_environment_context_note() {
    let home = TempHome::new("quiet-connection-list");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["connection", "list", "--quiet"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("FLEETY_AGENT_URL", "ws://override.example:8787")
        .output()
        .expect("quiet connection list");

    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("FLEETY_AGENT_URL"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("FLEETY_AGENT_URL"));
}

#[test]
fn compatibility_warnings_are_opt_in_for_non_tty_output() {
    let quiet_alias = run(&["server", "list"]);
    assert!(quiet_alias.status.success());
    assert!(!String::from_utf8_lossy(&quiet_alias.stderr).contains("compatibility alias"));

    let warned_alias = run(&["server", "list", "--warnings"]);
    assert!(warned_alias.status.success());
    let stderr = String::from_utf8_lossy(&warned_alias.stderr);
    assert!(stderr.contains("compatibility alias"), "{stderr}");
    assert!(stderr.contains("fleety connection"), "{stderr}");

    let canonical = run(&["connection", "list", "--warnings"]);
    assert!(canonical.status.success());
    assert!(!String::from_utf8_lossy(&canonical.stderr).contains("compatibility alias"));
}

#[test]
fn semantic_json_preserves_requested_compatibility_warning() {
    let home = TempHome::new("semantic-json-warning");
    let response = doctor_snapshot(r#"{"providers":{},"models":{}}"#);
    let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);

    let (output, _) = run_against_server(
        &["config", "provider", "list", "--json", "--warnings"],
        &url,
        &home,
        rx,
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert!(value["data"]["diagnostics"]
        .as_str()
        .is_some_and(|warning| warning.contains("compatibility alias")));
}

#[test]
fn legacy_server_url_override_is_transient_and_never_selects_a_profile() {
    let home = TempHome::new("server-url-override");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ServerStatusResult {
            version: "test-server".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        "current = \"A\"\n\n[profiles.A]\nurl = \"ws://127.0.0.1:9\"\n",
    )
    .expect("seed profile");
    let before = std::fs::read(&connections).expect("connections before");

    let (output, received) = run_against_profile(&["status", "--server", &url], &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("transient URL override"), "{stdout}");
    assert!(stdout.contains("owner: Server"), "{stdout}");
    assert!(matches!(received.get(1), Some(ClientMsg::ServerStatus)));
    assert_eq!(
        std::fs::read(connections).expect("connections after"),
        before
    );
}

#[test]
fn invocation_accepts_exactly_one_profile_or_url_selector() {
    let output = run(&[
        "--profile",
        "A",
        "status",
        "--server",
        "ws://127.0.0.1:8787",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("choose only one"));
}

#[test]
fn owner_and_target_are_one_non_conflicting_config_option() {
    let canonical = run(&["config", "--owner", "cli", "get", "FLEETY_VOICE_AUDIO"]);
    let legacy = run(&["config", "--target", "cli", "get", "FLEETY_VOICE_AUDIO"]);
    assert!(canonical.status.success());
    assert_eq!(canonical.stdout, legacy.stdout);

    let conflict = run(&["config", "--owner", "cli", "--target", "server", "list"]);
    assert_eq!(conflict.status.code(), Some(2));
}

#[test]
fn remote_config_result_names_selected_profile_and_server_owner() {
    let home = TempHome::new("server-owner-context");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: "server settings".into(),
            effect: None,
            error: None,
        }],
    ]);
    let connections = seed_a_and_b_profiles(&home, &url);

    let (output, received) = run_against_profile(
        &["config", "--owner", "server", "--profile", "B", "list"],
        &home,
        rx,
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("profile 'B'"), "{stdout}");
    assert!(stdout.contains("owner: Server"), "{stdout}");
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::ConfigExec {
            target: fleety_protocol::ConfigTarget::Server,
            ..
        })
    ));
    assert!(std::fs::read_to_string(connections)
        .expect("connections after")
        .contains("current = \"A\""));
}

#[test]
fn relayed_daemon_config_result_names_daemon_as_actual_owner() {
    let home = TempHome::new("daemon-owner-context");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: "daemon settings".into(),
            effect: None,
            error: None,
        }],
    ]);
    let connections = seed_a_and_b_profiles(&home, &url);

    let (output, received) = run_against_profile(
        &["--profile", "B", "config", "--owner", "daemon", "list"],
        &home,
        rx,
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("profile 'B'"), "{stdout}");
    assert!(stdout.contains("owner: Daemon 'cli-smoke'"), "{stdout}");
    assert!(!stdout.contains("owner: Server"), "{stdout}");
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::ConfigExec {
            target: fleety_protocol::ConfigTarget::Device(id),
            ..
        }) if id == "cli-smoke"
    ));
    assert!(std::fs::read_to_string(connections)
        .expect("connections after")
        .contains("current = \"A\""));
}

fn seed_a_and_b_profiles(home: &TempHome, b_url: &str) -> std::path::PathBuf {
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!(
            "current = \"A\"\n\n[profiles.A]\nurl = \"ws://127.0.0.1:9\"\n\n[profiles.B]\nurl = \"{b_url}\"\n"
        ),
    )
    .expect("seed profiles");
    connections
}

#[test]
fn canonical_provider_and_config_alias_send_the_same_server_request() {
    fn run_provider(args: &[&str], name: &str) -> (std::process::Output, Vec<ClientMsg>) {
        let home = TempHome::new(name);
        let response = doctor_snapshot(r#"{"providers":{},"models":{}}"#);
        let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);
        run_against_server(args, &url, &home, rx)
    }

    let (canonical_output, canonical_messages) =
        run_provider(&["provider", "list"], "provider-canonical");
    let (legacy_output, legacy_messages) = run_provider(
        &["config", "--target", "server", "provider", "list"],
        "provider-legacy",
    );

    assert!(
        canonical_output.status.success(),
        "canonical provider list: {}",
        String::from_utf8_lossy(&canonical_output.stderr)
    );
    assert_eq!(canonical_output.status.code(), legacy_output.status.code());
    let canonical_stdout = String::from_utf8_lossy(&canonical_output.stdout);
    let legacy_stdout = String::from_utf8_lossy(&legacy_output.stdout);
    assert!(canonical_stdout.lines().next().is_some_and(|line| {
        line.contains("environment override") && line.contains("owner: Server")
    }));
    assert!(legacy_stdout.lines().next().is_some_and(|line| {
        line.contains("environment override") && line.contains("owner: Server")
    }));
    assert_eq!(
        canonical_stdout.lines().skip(1).collect::<Vec<_>>(),
        legacy_stdout.lines().skip(1).collect::<Vec<_>>()
    );
    assert_eq!(canonical_messages.get(1), legacy_messages.get(1));
}

#[test]
fn provider_list_rejects_malformed_key_presence_metadata() {
    let provider = r#""openai": {
        "type": "api",
        "base_url": "https://api.example.test/v1"
    }"#;
    let cases = [
        (
            "mixed-type",
            format!(r#"{{"providers":{{{provider}}},"models":{{}},"key_present":["openai",7]}}"#),
        ),
        (
            "missing",
            format!(r#"{{"providers":{{{provider}}},"models":{{}}}}"#),
        ),
        (
            "wrong-container",
            format!(r#"{{"providers":{{{provider}}},"models":{{}},"key_present":"openai"}}"#),
        ),
        (
            "duplicate",
            format!(
                r#"{{"providers":{{{provider}}},"models":{{}},"key_present":["openai","openai"]}}"#
            ),
        ),
        (
            "unknown",
            format!(r#"{{"providers":{{{provider}}},"models":{{}},"key_present":["ghost"]}}"#),
        ),
    ];

    for (case, providers_json) in cases {
        let home = TempHome::new(&format!("provider-malformed-key-presence-{case}"));
        let response = raw_provider_snapshot(&providers_json);
        let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);

        let (output, _) = run_against_server(&["provider", "list"], &url, &home, rx);

        assert!(!output.status.success(), "{case}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("key metadata"), "{case}: {stderr}");
    }
}

#[test]
fn provider_list_rejects_plaintext_snapshot_without_echoing_the_secret() {
    let home = TempHome::new("provider-plaintext-key-refusal");
    let response = raw_provider_snapshot(
        r#"{
            "providers": {
                "openai": {
                    "type": "api",
                    "base_url": "https://api.example.test/v1",
                    "key": "snapshot-secret-must-not-render"
                }
            },
            "models": {},
            "key_present": ["openai"]
        }"#,
    );
    let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);

    let (output, _) = run_against_server(&["provider", "list"], &url, &home, rx);

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("snapshot-secret-must-not-render"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("snapshot-secret-must-not-render"));
}

#[test]
fn provider_list_rejects_duplicate_key_metadata_without_echoing_the_name() {
    let secret = "metadata-secret-must-not-render";
    let home = TempHome::new("provider-duplicate-key-metadata-redaction");
    let response = raw_provider_snapshot(&format!(
        r#"{{
            "providers": {{
                "{secret}": {{
                    "type": "api",
                    "base_url": "https://api.example.test/v1"
                }}
            }},
            "models": {{}},
            "key_present": ["{secret}", "{secret}"]
        }}"#
    ));
    let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);

    let (output, _) = run_against_server(&["provider", "list"], &url, &home, rx);

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
}

#[test]
fn provider_list_rejects_invalid_key_metadata_name_without_echoing_it() {
    let hostile_name = "../metadata-secret-must-not-render";
    let home = TempHome::new("provider-invalid-key-metadata-name");
    let response = raw_provider_snapshot(&format!(
        r#"{{
            "providers": {{
                "{hostile_name}": {{
                    "type": "api",
                    "base_url": "https://api.example.test/v1"
                }}
            }},
            "models": {{}},
            "key_present": ["{hostile_name}"]
        }}"#
    ));
    let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);

    let (output, _) = run_against_server(&["provider", "list"], &url, &home, rx);

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(hostile_name));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(hostile_name));
}

#[test]
fn provider_list_rejects_key_presence_for_oauth_provider() {
    let home = TempHome::new("provider-oauth-key-presence");
    let response = raw_provider_snapshot(
        r#"{
            "providers": {
                "codex": { "type": "oauth:codex" }
            },
            "models": {},
            "key_present": ["codex"]
        }"#,
    );
    let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);

    let (output, _) = run_against_server(&["provider", "list"], &url, &home, rx);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("key metadata"), "{stderr}");
}

#[test]
fn provider_list_human_and_json_report_key_presence_without_secret_bytes() {
    fn run(args: &[&str], name: &str) -> std::process::Output {
        let home = TempHome::new(name);
        let response = raw_provider_snapshot(
            r#"{
                "providers": {
                    "openai": {
                        "type": "api",
                        "base_url": "https://api.example.test/v1"
                    },
                    "other": {
                        "type": "api",
                        "base_url": "https://other.example.test/v1"
                    }
                },
                "models": {},
                "key_present": ["openai"]
            }"#,
        );
        let (url, rx) = start_ws_server(vec![vec![doctor_welcome()], vec![response]]);
        run_against_server(args, &url, &home, rx).0
    }

    let human = run(&["provider", "list"], "provider-key-state-human");
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human_stdout = String::from_utf8_lossy(&human.stdout);
    let openai = human_stdout
        .lines()
        .find(|line| line.starts_with("openai:"))
        .expect("openai row");
    let other = human_stdout
        .lines()
        .find(|line| line.starts_with("other:"))
        .expect("other row");
    assert!(openai.contains("key=Set"), "{openai}");
    assert!(other.contains("key=Not set"), "{other}");

    let json = run(&["provider", "list", "--json"], "provider-key-state-json");
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("Provider JSON envelope");
    let output = envelope["data"]["output"]
        .as_str()
        .expect("Provider data.output");
    let openai_json = output
        .lines()
        .find(|line| line.starts_with("openai:"))
        .expect("JSON openai row");
    let other_json = output
        .lines()
        .find(|line| line.starts_with("other:"))
        .expect("JSON other row");
    assert!(openai_json.contains("key=Set"), "{openai_json}");
    assert!(other_json.contains("key=Not set"), "{other_json}");
    let providers = envelope["data"]["providers"]
        .as_array()
        .expect("structured Provider rows");
    let openai_data = providers
        .iter()
        .find(|provider| provider["name"] == "openai")
        .expect("structured openai");
    let other_data = providers
        .iter()
        .find(|provider| provider["name"] == "other")
        .expect("structured other");
    assert_eq!(openai_data["key_present"], true);
    assert_eq!(other_data["key_present"], false);
}

#[test]
fn provider_add_set_and_clear_send_explicit_secret_safe_transitions() {
    fn run_mutation(
        args: &[&str],
        snapshot: &str,
        name: &str,
    ) -> (std::process::Output, Vec<ClientMsg>) {
        let home = TempHome::new(name);
        let response = raw_provider_snapshot(snapshot);
        let (url, rx) = start_ws_server(vec![
            vec![doctor_welcome()],
            vec![response],
            vec![ServerMsg::ConfigResult {
                ok: true,
                output: String::new(),
                effect: None,
                error: None,
            }],
        ]);
        run_against_server(args, &url, &home, rx)
    }

    let empty = r#"{"providers":{},"models":{},"key_present":[]}"#;
    let (added, add_messages) = run_mutation(
        &[
            "provider",
            "add",
            "openai",
            "--type",
            "api",
            "--base-url",
            "https://api.example.test/v1",
            "--key",
            "add-secret-must-not-render",
        ],
        empty,
        "provider-add-key-transition",
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(!String::from_utf8_lossy(&added.stdout).contains("add-secret-must-not-render"));
    assert!(!String::from_utf8_lossy(&added.stderr).contains("add-secret-must-not-render"));
    let add_json = add_messages
        .iter()
        .find_map(|message| match message {
            ClientMsg::ConfigApply {
                providers_json: Some(json),
                ..
            } => Some(json),
            _ => None,
        })
        .expect("add ConfigApply");
    let add_value: serde_json::Value = serde_json::from_str(add_json).expect("add payload");
    assert_eq!(
        add_value["providers"]["openai"]["key"],
        "add-secret-must-not-render"
    );
    assert_eq!(add_value["clear_keys"], serde_json::json!([]));

    let configured = r#"{
        "providers": {
            "openai": {
                "type": "api",
                "base_url": "https://api.example.test/v1"
            }
        },
        "models": {},
        "key_present": ["openai"]
    }"#;
    let (kept, keep_messages) = run_mutation(
        &[
            "provider",
            "set",
            "openai",
            "--base-url",
            "https://other.example.test/v1",
        ],
        configured,
        "provider-keep-key-transition",
    );
    assert!(
        kept.status.success(),
        "{}",
        String::from_utf8_lossy(&kept.stderr)
    );
    let keep_json = keep_messages
        .iter()
        .find_map(|message| match message {
            ClientMsg::ConfigApply {
                providers_json: Some(json),
                ..
            } => Some(json),
            _ => None,
        })
        .expect("keep ConfigApply");
    let keep_value: serde_json::Value = serde_json::from_str(keep_json).expect("keep payload");
    assert!(keep_value["providers"]["openai"].get("key").is_none());
    assert_eq!(keep_value["clear_keys"], serde_json::json!([]));

    let (cleared, clear_messages) = run_mutation(
        &["provider", "set", "openai", "--clear-key"],
        configured,
        "provider-clear-key-transition",
    );
    assert!(
        cleared.status.success(),
        "{}",
        String::from_utf8_lossy(&cleared.stderr)
    );
    let clear_json = clear_messages
        .iter()
        .find_map(|message| match message {
            ClientMsg::ConfigApply {
                providers_json: Some(json),
                ..
            } => Some(json),
            _ => None,
        })
        .expect("clear ConfigApply");
    let clear_value: serde_json::Value = serde_json::from_str(clear_json).expect("clear payload");
    assert!(clear_value["providers"]["openai"].get("key").is_none());
    assert_eq!(clear_value["clear_keys"], serde_json::json!(["openai"]));
}

#[test]
fn provider_mutation_error_cannot_echo_the_submitted_key_in_human_or_json_output() {
    fn run(args: &[&str], name: &str) -> std::process::Output {
        let home = TempHome::new(name);
        let response = raw_provider_snapshot(r#"{"providers":{},"models":{},"key_present":[]}"#);
        let echoed = "mutation-error-secret";
        let (url, rx) = start_ws_server(vec![
            vec![doctor_welcome()],
            vec![response],
            vec![ServerMsg::ConfigResult {
                ok: false,
                output: String::new(),
                effect: None,
                error: Some(fleety_protocol::WireError {
                    kind: "rejected".into(),
                    message: format!("Server rejected {echoed}"),
                    remediation: Some(format!("Remove {echoed} and retry")),
                }),
            }],
        ]);
        let mut command = args.to_vec();
        command.extend([
            "provider",
            "add",
            "openai",
            "--type",
            "api",
            "--base-url",
            "https://api.example.test/v1",
            "--key",
            echoed,
        ]);
        run_against_server(&command, &url, &home, rx).0
    }

    for (prefix, name) in [
        (Vec::<&str>::new(), "provider-error-human"),
        (vec!["--json"], "provider-error-json"),
    ] {
        let output = run(&prefix, name);
        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains("mutation-error-secret"), "{stdout}");
        assert!(!stderr.contains("mutation-error-secret"), "{stderr}");
        assert!(
            stdout.contains("<redacted>") || stderr.contains("<redacted>"),
            "stdout={stdout}; stderr={stderr}"
        );
    }
}

#[test]
fn model_catalog_uses_oauth_state_then_fetches_models_on_the_same_connection() {
    fn run_catalog(args: &[&str], name: &str) -> (std::process::Output, Vec<ClientMsg>) {
        let home = TempHome::new(name);
        let providers = r#"{
            "providers": {
                "tingzhen-codex": { "type": "oauth:codex" }
            },
            "models": {}
        }"#;
        let (url, rx) = start_ws_server(vec![
            vec![doctor_welcome()],
            vec![doctor_snapshot(providers)],
            vec![ServerMsg::CredentialStatusResult {
                present: true,
                expires_at_secs: Some(u64::MAX),
                detail: Some("account test".into()),
                error: None,
            }],
            vec![ServerMsg::ProviderModelListResult {
                provider: "tingzhen-codex".into(),
                model_ids: vec!["gpt-test-a".into(), "gpt-test-b".into()],
                error: None,
            }],
        ]);
        run_against_server(args, &url, &home, rx)
    }

    let (canonical, canonical_messages) = run_catalog(
        &["model", "catalog", "tingzhen-codex", "--role", "main"],
        "model-catalog-canonical",
    );
    assert!(
        canonical.status.success(),
        "{}",
        String::from_utf8_lossy(&canonical.stderr)
    );
    let stdout = String::from_utf8_lossy(&canonical.stdout);
    assert!(stdout.contains("gpt-test-a"), "{stdout}");
    assert!(stdout.contains("gpt-test-b"), "{stdout}");
    assert!(matches!(
        canonical_messages.as_slice(),
        [
            ClientMsg::Hello { .. },
            ClientMsg::ConfigSnapshot { target: fleety_protocol::ConfigTarget::Server },
            ClientMsg::CredentialStatus { provider: Some(provider), .. },
            ClientMsg::ProviderModelList { provider: catalog_provider },
        ] if provider == "tingzhen-codex" && catalog_provider == "tingzhen-codex"
    ));

    let (compatibility, compatibility_messages) = run_catalog(
        &[
            "config",
            "--target",
            "server",
            "model",
            "catalog",
            "tingzhen-codex",
            "--role",
            "main",
        ],
        "model-catalog-compatibility",
    );
    assert_eq!(compatibility.status.code(), canonical.status.code());
    assert_eq!(
        String::from_utf8_lossy(&compatibility.stdout)
            .lines()
            .skip(1)
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&canonical.stdout)
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
    );
    assert_eq!(compatibility_messages, canonical_messages);

    let (quiet, _) = run_catalog(
        &["--quiet", "model", "catalog", "tingzhen-codex"],
        "model-catalog-quiet",
    );
    assert!(quiet.status.success());
    assert_eq!(
        String::from_utf8_lossy(&quiet.stdout),
        "gpt-test-a\ngpt-test-b\n"
    );

    let (json, _) = run_catalog(
        &["model", "catalog", "tingzhen-codex", "--json"],
        "model-catalog-json",
    );
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("catalog JSON");
    assert_eq!(value["data"]["provider"], "tingzhen-codex");
    assert_eq!(value["data"]["role"], "main");
    assert_eq!(
        value["data"]["models"],
        serde_json::json!(["gpt-test-a", "gpt-test-b"])
    );
    assert_eq!(value["errors"], serde_json::json!([]));
}

#[test]
fn model_catalog_requires_oauth_login_before_requesting_the_catalog() {
    let home = TempHome::new("model-catalog-not-signed-in");
    let providers = r#"{
        "providers": {
            "tingzhen-codex": { "type": "oauth:codex" }
        },
        "models": {}
    }"#;
    let (url, rx) = start_ws_server(vec![
        vec![doctor_welcome()],
        vec![doctor_snapshot(providers)],
        vec![ServerMsg::CredentialStatusResult {
            present: false,
            expires_at_secs: None,
            detail: None,
            error: None,
        }],
    ]);

    let (output, messages) =
        run_against_server(&["model", "catalog", "tingzhen-codex"], &url, &home, rx);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Not signed in"), "{stderr}");
    assert!(
        stderr.contains("fleety provider login <provider>"),
        "{stderr}"
    );
    assert!(!messages
        .iter()
        .any(|message| matches!(message, ClientMsg::ProviderModelList { .. })));
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
fn ask_rejects_duplicate_welcome_before_accepting_more_server_output() {
    let home = TempHome::new("ask-duplicate-welcome");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            welcome(None),
            ServerMsg::Assistant {
                conversation_id: "c1".into(),
                text: "must-not-be-accepted".into(),
                seq: 7,
                speech: None,
                attention: None,
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
    ]);

    let (output, _) = run_against_server(&["ask", "hello"], &url, &home, rx);

    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must-not-be-accepted"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate Welcome"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn resume_rejects_duplicate_welcome_before_accepting_replay() {
    let home = TempHome::new("resume-duplicate-welcome");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            welcome(None),
            ServerMsg::Replay {
                conversation_id: "c1".into(),
                seq: 3,
                role: "assistant".into(),
                content: "must-not-be-accepted".into(),
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
    ]);

    let (output, _) = run_against_server(&["resume", "c1"], &url, &home, rx);

    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("must-not-be-accepted"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate Welcome"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn assistant_and_approval_human_output_cannot_inject_terminal_controls() {
    let home = TempHome::new("ask-terminal-safe");
    let hostile = "wss://u:p@host/x?token=SECRET#tail\u{1b}]52;c;STEAL\u{7}\rnext";
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            ServerMsg::ApprovalRequested {
                approval_id: "approval-1".into(),
                tool: hostile.into(),
                risk: hostile.into(),
                summary: hostile.into(),
            },
            ServerMsg::Assistant {
                conversation_id: "c1".into(),
                text: format!("first\n{hostile}"),
                seq: 7,
                speech: None,
                attention: None,
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
        vec![],
    ]);

    let (output, _) = run_against_server(&["ask", "hello"], &url, &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = [output.stdout, output.stderr].concat();
    assert!(!rendered.contains(&0x1b), "{:?}", rendered);
    assert!(!rendered.contains(&0x07), "{:?}", rendered);
    assert!(!rendered.contains(&b'\r'), "{:?}", rendered);
    let rendered = String::from_utf8_lossy(&rendered);
    for secret in ["SECRET", "u:p", "#tail"] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
    }
    assert!(rendered.contains("token=<redacted>"), "{rendered}");
    assert!(rendered.contains("\\rnext"), "{rendered}");
}

#[test]
fn config_result_human_output_cannot_inject_terminal_controls() {
    let home = TempHome::new("config-result-terminal-safe");
    let hostile = "saved\u{1b}]52;c;STEAL\u{7}\rnext wss://u:p@host/x?token=SECRET#tail";
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: hostile.into(),
            effect: None,
            error: None,
        }],
    ]);
    let (output, _) = run_against_server(&["config", "--owner", "server", "list"], &url, &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.contains(&0x1b), "{:?}", output.stdout);
    assert!(!output.stdout.contains(&0x07), "{:?}", output.stdout);
    assert!(!output.stdout.contains(&b'\r'), "{:?}", output.stdout);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("SECRET"), "{stdout}");
    assert!(!stdout.contains("u:p"), "{stdout}");
    assert!(stdout.contains("token=<redacted>"), "{stdout}");
}

#[test]
fn config_result_json_redacts_endpoint_secrets_without_corrupting_json() {
    let home = TempHome::new("config-result-json-redaction");
    let hostile = "provider \u{1b}]52;c;semantic\u{7} wss://u:p@host/x?token=SECRET#tail";
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: hostile.into(),
            effect: None,
            error: None,
        }],
    ]);
    let (output, _) = run_against_server(
        &["--json", "config", "--owner", "server", "list"],
        &url,
        &home,
        rx,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    let rendered = value["data"]["output"].as_str().expect("output string");
    assert!(
        rendered.contains("wss://host/x?token=<redacted>"),
        "{rendered:?}"
    );
    for secret in ["u:p", "SECRET", "#tail"] {
        assert!(!rendered.contains(secret), "leaked {secret}: {rendered:?}");
    }
    assert!(
        rendered.contains('\u{1b}'),
        "JSON semantic value keeps its control character"
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "JSON encoding must escape terminal controls"
    );
}

#[test]
fn ask_option_terminator_preserves_flag_like_prompt_text() {
    let home = TempHome::new("ask-option-terminator");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::Done {
            conversation_id: "c1".into(),
        }],
    ]);

    let (output, received) =
        run_against_server(&["ask", "--", "--server", "literal"], &url, &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::UserMessage { text, .. }) if text == "--server literal"
    ));
}

#[test]
fn generic_json_mode_wraps_ask_without_mixed_stdout() {
    let home = TempHome::new("ask-json");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            ServerMsg::Assistant {
                conversation_id: "c1".into(),
                text: "json answer".into(),
                seq: 1,
                speech: None,
                attention: None,
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
    ]);

    let (output, _) = run_against_server(&["ask", "hello", "--json"], &url, &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("ask envelope");
    assert_eq!(value["ok"], true);
    assert_eq!(value["context"]["owner"], "server");
    assert_eq!(value["data"]["output"], "json answer");
    assert!(value["data"]["diagnostics"]
        .as_str()
        .is_some_and(|text| text.contains("conversation c1")));
}

#[test]
fn json_runtime_failure_keeps_resolved_context() {
    let home = TempHome::new("json-runtime-context");
    let url = rejecting_ws_url();
    let connections = home.0.join(".fleety").join("connections.toml");
    std::fs::create_dir_all(connections.parent().expect("parent")).expect("fleety home");
    std::fs::write(
        &connections,
        format!("current = \"B\"\n\n[profiles.B]\nurl = \"{url}\"\n"),
    )
    .expect("seed profile");

    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args(["status", "--json"])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run failed JSON status");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("error envelope");
    assert_eq!(value["ok"], false);
    assert_eq!(value["context"]["profile"], "B");
    assert_eq!(value["context"]["owner"], "server");
    assert_eq!(value["context"]["endpoint"], url);
    assert_eq!(value["errors"][0]["kind"], "runtime");
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
fn audit_context_names_server_owner_and_device_only_as_filter() {
    let audit_result = || ServerMsg::AuditListResult {
        device_id: "cli-smoke".into(),
        entries_json: "[]".into(),
    };

    let home = TempHome::new("audit-server-owner-human");
    let (url, rx) = start_ws_server(vec![vec![welcome(None)], vec![audit_result()]]);
    seed_a_and_b_profiles(&home, &url);
    let (output, received) = run_against_profile(&["--profile", "B", "audit", "list"], &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("owner: Server"), "{stdout}");
    assert!(!stdout.contains("owner: Daemon"), "{stdout}");
    assert!(stdout.contains("device filter: cli-smoke"), "{stdout}");
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::AuditList { device_id, .. }) if device_id == "cli-smoke"
    ));

    let home = TempHome::new("audit-server-owner-json");
    let (url, rx) = start_ws_server(vec![vec![welcome(None)], vec![audit_result()]]);
    seed_a_and_b_profiles(&home, &url);
    let (output, received) =
        run_against_profile(&["audit", "list", "--profile=B", "--json"], &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("audit JSON envelope");
    assert_eq!(envelope["context"]["owner"], "server");
    assert_eq!(envelope["context"]["profile"], "B");
    assert_eq!(envelope["context"]["device_id"], "cli-smoke");
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::AuditList { device_id, .. }) if device_id == "cli-smoke"
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
fn human_transport_failure_never_leaks_raw_endpoint_or_trace_secrets() {
    let home = TempHome::new("human-endpoint-failure-redaction");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .arg("status")
        .env(
            "FLEETY_AGENT_URL",
            "ws://user:USERINFOSECRET@127.0.0.1:1/path?token=QUERYSECRET#fragment",
        )
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env("RUST_LOG", "trace")
        .env_remove("FLEETY_TOKEN")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run failed human transport");

    assert_eq!(output.status.code(), Some(1));
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!rendered.contains("USERINFOSECRET"), "{rendered}");
    assert!(!rendered.contains("QUERYSECRET"), "{rendered}");
    assert!(!rendered.contains("user@"), "{rendered}");
    assert!(!rendered.contains("#fragment"), "{rendered}");
}

#[test]
fn successful_human_status_redacts_resolved_endpoint_query_values() {
    let home = TempHome::new("human-endpoint-success-redaction");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ServerStatusResult {
            version: "safe-server".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);
    let endpoint = format!("{url}/?token=SUCCESSSECRET");

    let (output, _) = run_against_server(&["status"], &endpoint, &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!rendered.contains("SUCCESSSECRET"), "{rendered}");
    assert!(rendered.contains("token=<redacted>"), "{rendered}");
}

#[test]
fn equals_form_global_server_selector_reaches_the_same_target() {
    let home = TempHome::new("equals-global-server");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ServerStatusResult {
            version: "equals-server".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);
    let selector = format!("--server={url}");

    let (output, received) = run_against_server(&[&selector, "status"], &url, &home, rx);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("equals-server"));
    assert!(matches!(received.get(1), Some(ClientMsg::ServerStatus)));

    let home = TempHome::new("equals-global-url");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ServerStatusResult {
            version: "equals-url-server".into(),
            uptime_secs: 1,
            connected_devices: 0,
            device_ids_json: "[]".into(),
            extra_json: None,
        }],
    ]);
    let selector = format!("--url={url}");
    let (output, received) = run_against_server(&["status", &selector], &url, &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("equals-url-server"));
    assert!(matches!(received.get(1), Some(ClientMsg::ServerStatus)));
}

#[test]
fn attached_short_server_selector_reaches_the_same_target() {
    for (case, prefix) in [("equals", "-s="), ("attached", "-s")] {
        let home = TempHome::new(&format!("short-server-{case}"));
        let (url, rx) = start_ws_server(vec![
            vec![welcome(None)],
            vec![ServerMsg::ServerStatusResult {
                version: format!("short-{case}-server"),
                uptime_secs: 1,
                connected_devices: 0,
                device_ids_json: "[]".into(),
                extra_json: None,
            }],
        ]);
        let selector = format!("{prefix}{url}");

        let (output, received) = run_against_server(&[&selector, "status"], &url, &home, rx);

        assert!(
            output.status.success(),
            "{}: {}",
            case,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(&format!("short-{case}-server")));
        assert!(matches!(received.get(1), Some(ClientMsg::ServerStatus)));
    }
}

#[test]
fn equals_form_profile_owner_target_and_local_option_match_execution_contracts() {
    let home = TempHome::new("equals-profile-owner");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: "server settings".into(),
            effect: None,
            error: None,
        }],
    ]);
    seed_a_and_b_profiles(&home, &url);
    let (output, received) = run_against_profile(
        &["config", "--owner=server", "--profile=B", "list"],
        &home,
        rx,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::ConfigExec {
            target: fleety_protocol::ConfigTarget::Server,
            args,
        }) if args == &["list"]
    ));

    let home = TempHome::new("equals-target-alias");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConfigResult {
            ok: true,
            output: "server settings".into(),
            effect: None,
            error: None,
        }],
    ]);
    let (output, received) =
        run_against_server(&["config", "--target=server", "list"], &url, &home, rx);
    assert!(output.status.success());
    assert!(matches!(
        received.get(1),
        Some(ClientMsg::ConfigExec {
            target: fleety_protocol::ConfigTarget::Server,
            args,
        }) if args == &["list"]
    ));

    let home = TempHome::new("equals-command-local");
    let output = Command::new(env!("CARGO_BIN_EXE_fleety"))
        .args([
            "connection",
            "add",
            "office",
            "ws://office.test:8787",
            "--label=Office LAN",
        ])
        .env("HOME", &home.0)
        .env("USERPROFILE", &home.0)
        .env_remove("FLEETY_AGENT_URL")
        .output()
        .expect("run equals-form connection add");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = std::fs::read_to_string(home.0.join(".fleety").join("connections.toml"))
        .expect("saved connections");
    assert!(saved.contains("label = \"Office LAN\""), "{saved}");
}

#[test]
fn remote_status_human_scalars_are_terminal_safe_while_json_stays_semantic() {
    let dangerous = "safe\u{1b}]52;c;COPIED\u{7}\r\nFORGED";
    let status_frame = || ServerMsg::ServerStatusResult {
        version: dangerous.into(),
        uptime_secs: 1,
        connected_devices: 1,
        device_ids_json: serde_json::json!([dangerous]).to_string(),
        extra_json: Some(
            serde_json::json!({
                "sidecars": {
                    dangerous: { "status": dangerous, "path": dangerous }
                }
            })
            .to_string(),
        ),
    };

    let home = TempHome::new("remote-status-human-controls");
    let (url, rx) = start_ws_server(vec![vec![welcome(None)], vec![status_frame()]]);
    let (output, _) = run_against_server(&["status"], &url, &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for control in ['\u{1b}', '\u{7}', '\r'] {
        assert!(
            !stdout.contains(control),
            "human output kept control {control:?}: {stdout}"
        );
    }
    assert!(!stdout.lines().any(|line| line == "FORGED"), "{stdout}");
    assert!(stdout.contains("\\nFORGED"), "{stdout}");

    let home = TempHome::new("remote-status-json-controls");
    let (url, rx) = start_ws_server(vec![vec![welcome(None)], vec![status_frame()]]);
    let (output, _) = run_against_server(&["--json", "status"], &url, &home, rx);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON envelope");
    assert_eq!(envelope["data"]["server"]["version"], dangerous);
    assert_eq!(envelope["data"]["server"]["device_ids"][0], dangerous);
    assert_eq!(
        envelope["data"]["server"]["extra"]["sidecars"][dangerous]["path"],
        dangerous
    );
}

#[test]
fn conversation_audit_rollback_and_resume_scalars_cannot_inject_terminal_controls() {
    let dangerous = "value\u{1b}]52;c;COPIED\u{7}\r\nFORGED";
    let assert_safe = |output: &std::process::Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for control in ['\u{1b}', '\u{7}', '\r'] {
            assert!(
                !rendered.contains(control),
                "output kept control {control:?}: {rendered}"
            );
        }
        assert!(!rendered.lines().any(|line| line == "FORGED"), "{rendered}");
        assert!(rendered.contains("\\nFORGED"), "{rendered}");
    };

    let home = TempHome::new("conversation-human-controls");
    let rows = serde_json::json!([{
        "conversation_id": dangerous,
        "last_ts_secs": 1,
        "events": 1,
        "preview": dangerous
    }]);
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::ConversationListResult {
            conversations_json: rows.to_string(),
        }],
    ]);
    let (output, _) = run_against_server(&["conversations", "list"], &url, &home, rx);
    assert_safe(&output);

    let home = TempHome::new("audit-human-controls");
    let entries = serde_json::json!([{
        "index": 1,
        "kind": dangerous,
        "tool": dangerous,
        "ts_secs": 1
    }]);
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::AuditListResult {
            device_id: "cli-smoke".into(),
            entries_json: entries.to_string(),
        }],
    ]);
    let (output, _) = run_against_server(&["audit", "list"], &url, &home, rx);
    assert_safe(&output);

    let home = TempHome::new("rollback-human-controls");
    let backups = serde_json::json!([{
        "id": dangerous,
        "original_rel_path": dangerous,
        "ts_secs": 1
    }]);
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![ServerMsg::RollbackListResult {
            device_id: "cli-smoke".into(),
            backups_json: backups.to_string(),
        }],
    ]);
    let (output, _) = run_against_server(&["rollback", "list"], &url, &home, rx);
    assert_safe(&output);

    let home = TempHome::new("resume-human-controls");
    let (url, rx) = start_ws_server(vec![
        vec![welcome(None)],
        vec![
            ServerMsg::Replay {
                conversation_id: "c1".into(),
                seq: 1,
                role: dangerous.into(),
                content: dangerous.into(),
            },
            ServerMsg::Done {
                conversation_id: "c1".into(),
            },
        ],
    ]);
    let (output, _) = run_against_server(&["resume", "c1"], &url, &home, rx);
    assert_safe(&output);
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
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("empty token or no identity fingerprint")
    );
}

#[test]
fn generated_help_and_documented_command_inventory_cannot_drift() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    let commands_block = help
        .split_once("Commands:\n")
        .and_then(|(_, rest)| rest.split_once("\nOptions:"))
        .map(|(commands, _)| commands)
        .expect("generated Commands block");
    let actual: Vec<&str> = commands_block
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        actual,
        vec![
            "init",
            "ask",
            "chat",
            "conversations",
            "connection",
            "provider",
            "model",
            "config",
            "status",
            "doctor",
            "completion",
            "voice",
            "audit",
            "rollback",
            "pair",
            "pair-code",
            "daemon",
            "update",
            "acp",
            "version",
            "help",
        ],
        "update README and this exhaustive inventory when the generated command tree changes"
    );

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root");
    let readme = std::fs::read_to_string(repo.join("README.md")).expect("README");
    for command in actual.iter().copied().filter(|command| *command != "help") {
        assert!(
            readme.contains(&format!("`fleety {command}")),
            "README command inventory is missing canonical `fleety {command}`"
        );
    }
    let cli_design = std::fs::read_to_string(repo.join("docs/design-cli-config.md"))
        .expect("CLI design document");
    assert!(
        !cli_design.contains("connection add <name> <url> [--label …] [--pair"),
        "CLI design must not advertise the nonexistent `connection add --pair` flag"
    );
    for mapping in [
        "`fleety tui` | `fleety chat`",
        "`fleety server …` | `fleety connection …`",
        "`fleety auth login\\|logout\\|status …` | `fleety provider login\\|logout\\|status …`",
        "`fleety config --target …` | `fleety config --owner …`",
    ] {
        assert!(
            readme.contains(mapping),
            "README alias mapping missing: {mapping}"
        );
    }

    let config_help = run(&["config", "--help"]);
    let config_help = String::from_utf8(config_help.stdout).expect("UTF-8 config help");
    assert!(config_help.contains("--owner <server|daemon|cli|DEVICE_ID>"));
    assert!(config_help.contains("[aliases: target]"));
    let completion_help = run(&["completion", "--help"]);
    let completion_help = String::from_utf8(completion_help.stdout).expect("completion help");
    assert!(completion_help.contains("fleety completion bash >"));
    assert!(completion_help.contains("fleety completion powershell |"));
}

/// `--help` is the only place a user can discover what a command takes, and
/// three separate rounds of review found arguments printing a blank column —
/// including credential-bearing ones. This walks every help page the binary can
/// produce and fails on any argument, option, or subcommand with no
/// description, so the next one is caught here instead of by a reader.
#[test]
fn every_help_page_describes_every_argument_and_subcommand() {
    fn help_lines(path: &[String]) -> Vec<String> {
        let home = TempHome::new("help-sweep");
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let out = Command::new(env!("CARGO_BIN_EXE_fleety"))
            .args(&args)
            .env("HOME", &home.0)
            .env("USERPROFILE", &home.0)
            .env_remove("FLEETY_AGENT_URL")
            .output()
            .expect("run fleety --help");
        let text = if out.stdout.is_empty() {
            String::from_utf8_lossy(&out.stderr).into_owned()
        } else {
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        text.lines().map(str::to_string).collect()
    }

    let mut undescribed: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut queue: Vec<Vec<String>> = vec![Vec::new()];
    let mut seen: Vec<String> = Vec::new();
    while let Some(path) = queue.pop() {
        let key = path.join(" ");
        if seen.contains(&key) || path.len() > 4 {
            continue;
        }
        seen.push(key.clone());
        pages += 1;
        let lines = help_lines(&path);
        let mut section = String::new();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim_end();
            if trimmed.ends_with(':') && !trimmed.starts_with(' ') {
                section = trimmed.trim_end_matches(':').to_string();
                continue;
            }
            // Only the entry lines themselves are indented exactly two spaces;
            // a wrapped description is indented further.
            let indent = line.len() - line.trim_start().len();
            if indent != 2 || line.trim().is_empty() {
                continue;
            }
            let body = line.trim();
            // Clap separates an entry from its description by two or more
            // spaces, or wraps the description onto a more-indented next line.
            let has_inline = body
                .find("  ")
                .is_some_and(|at| !body[at..].trim().is_empty());
            let has_wrapped = lines.get(index + 1).is_some_and(|next| {
                !next.trim().is_empty() && (next.len() - next.trim_start().len()) > 2
            });
            let described = has_inline || has_wrapped;
            match section.as_str() {
                "Commands" => {
                    let name = body.split_whitespace().next().unwrap_or_default();
                    if name == "help" {
                        continue;
                    }
                    queue.push([path.clone(), vec![name.to_string()]].concat());
                    if !described {
                        undescribed.push(format!("`fleety {key}` subcommand `{name}`"));
                    }
                }
                "Arguments" | "Options" if !described => {
                    undescribed.push(format!("`fleety {key}` -> {body}"));
                }
                _ => {}
            }
        }
    }
    assert!(pages > 40, "the sweep only reached {pages} help pages");
    assert!(
        undescribed.is_empty(),
        "{} help entries print no description:\n{}",
        undescribed.len(),
        undescribed.join("\n")
    );
}
