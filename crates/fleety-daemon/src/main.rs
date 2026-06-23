//! fleetyd — the Fleety device background service.
//!
//! Connects to the Agent on startup (registering this device) and holds the
//! connection open. `fleetyd install`/`uninstall` set up OS autostart. Heartbeat
//! is handled by WebSocket control frames; reconnect/backoff and on-device tool
//! execution are later milestones.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod ondevice;
mod provision;
mod service;
mod update;

use std::path::PathBuf;

use agent_core::{obs, CoreError, Result};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use fleety_protocol::{ClientMsg, ServerMsg, WireError, PROTOCOL_VERSION};

/// `~/.fleety/fleetyd.token` — the path where fleetyd persists a token it
/// received from the server after a successful pair, so a restart can come
/// straight back without needing `FLEETY_TOKEN` or another pairing code.
fn token_path() -> Option<PathBuf> {
    let base = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(PathBuf::from(base).join(".fleety").join("fleetyd.token"))
}

fn read_saved_token() -> Option<String> {
    let path = token_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn write_saved_token(token: &str) -> Result<()> {
    let path =
        token_path().ok_or_else(|| CoreError::Message("no home dir for token".to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create ~/.fleety: {e}")))?;
    }
    std::fs::write(&path, token)
        .map_err(|e| CoreError::Message(format!("cannot save token: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn clear_saved_token() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[tokio::main]
async fn main() {
    obs::init();
    // Subcommands: `install` / `uninstall` configure OS autostart, then exit.
    match std::env::args().nth(1).as_deref() {
        Some("install") => {
            if let Err(e) = service::install() {
                tracing::error!(report = ?e.report(), "install failed");
            }
            // Provision the data-analysis sidecar (best-effort).
            if let Err(e) = provision::ensure_insyra(false).await {
                tracing::warn!(report = ?e.report(), "could not provision fleety-insyra sidecar");
            }
            return;
        }
        Some("uninstall") => {
            if let Err(e) = service::uninstall() {
                tracing::error!(report = ?e.report(), "uninstall failed");
            }
            return;
        }
        Some("update") => {
            if let Err(e) = update::update().await {
                tracing::error!(report = ?e.report(), "update failed");
            }
            // Refresh the data-analysis sidecar alongside fleetyd (best-effort).
            if let Err(e) = provision::ensure_insyra(true).await {
                tracing::warn!(report = ?e.report(), "could not refresh fleety-insyra sidecar");
            }
            return;
        }
        _ => {}
    }
    tracing::info!(version = agent_core::VERSION, "fleetyd starting");
    if let Err(e) = run().await {
        tracing::error!(report = ?e.report(), "fleetyd exited with error");
    }
}

fn agent_url() -> String {
    std::env::var("FLEETY_AGENT_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787".to_string())
}

fn device_id() -> String {
    std::env::var("FLEETY_DEVICE_ID")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "fleetyd-device".to_string())
}

async fn run() -> Result<()> {
    let url = agent_url();
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| CoreError::Provider(format!("cannot connect to {url}: {e}")))?;
    let (mut tx, mut rx) = ws.split();

    // Token precedence: env override > on-disk persisted token > pairing flow.
    let token = std::env::var("FLEETY_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(read_saved_token);
    let pairing_code = std::env::var("FLEETY_PAIRING_CODE")
        .ok()
        .filter(|s| !s.is_empty());
    let hello = serde_json::to_string(&ClientMsg::Hello {
        device_id: device_id(),
        protocol: PROTOCOL_VERSION,
        token,
        pairing_code,
    })
    .map_err(|e| CoreError::Message(format!("serialize hello: {e}")))?;
    tx.send(WsMessage::Text(hello))
        .await
        .map_err(|e| CoreError::Provider(format!("send hello failed: {e}")))?;

    let registry = ondevice::build_local_registry(&ondevice::device_root());
    tracing::info!(%url, "connected; holding connection");
    while let Some(frame) = rx.next().await {
        let frame =
            frame.map_err(|e| CoreError::Provider(format!("websocket read failed: {e}")))?;
        if frame.is_close() {
            break;
        }
        if !frame.is_text() {
            continue;
        }
        let Ok(text) = frame.to_text() else { continue };
        let Ok(msg) = serde_json::from_str::<ServerMsg>(text) else {
            continue;
        };
        match msg {
            ServerMsg::Welcome {
                session_id, token, ..
            } => {
                if let Some(tok) = token {
                    if let Err(e) = write_saved_token(&tok) {
                        tracing::warn!(report = ?e.report(), "could not persist fleetyd token");
                    } else {
                        tracing::info!("fleetyd token persisted to ~/.fleety/fleetyd.token");
                    }
                }
                tracing::info!(%session_id, "registered with agent");
            }
            ServerMsg::Error { ref error } if error.kind == "unauthenticated" => {
                tracing::warn!(
                    "server rejected our token: {} — clearing saved token so the next \
                     connect can re-pair",
                    error.message
                );
                clear_saved_token();
                break;
            }
            ServerMsg::RunTool {
                call_id,
                tool,
                args_json,
            } => {
                let args: serde_json::Value =
                    serde_json::from_str(&args_json).unwrap_or_else(|_| serde_json::json!({}));
                tracing::info!(%tool, "running on-device tool");
                let reply = match registry.call(&tool, args).await {
                    Ok(value) => ClientMsg::ToolResult {
                        call_id,
                        result_json: value.to_string(),
                    },
                    Err(e) => {
                        let r = e.report();
                        ClientMsg::ToolError {
                            call_id,
                            error: WireError {
                                kind: r.kind,
                                message: r.message,
                                remediation: r.remediation,
                            },
                        }
                    }
                };
                let out = serde_json::to_string(&reply)
                    .map_err(|e| CoreError::Message(format!("serialize reply: {e}")))?;
                tx.send(WsMessage::Text(out))
                    .await
                    .map_err(|e| CoreError::Provider(format!("send reply failed: {e}")))?;
            }
            _ => {}
        }
    }
    tracing::info!("fleetyd disconnected");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
        temp_home: PathBuf,
    }

    impl EnvGuard {
        fn new(name: &str) -> Self {
            let keys = [
                "HOME",
                "USERPROFILE",
                "FLEETY_AGENT_URL",
                "FLEETY_DEVICE_ID",
                "COMPUTERNAME",
                "HOSTNAME",
            ];
            let saved = keys
                .into_iter()
                .map(|key| (key, std::env::var(key).ok()))
                .collect::<Vec<_>>();
            let temp_home =
                std::env::temp_dir().join(format!("fleetyd-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&temp_home);
            std::fs::create_dir_all(&temp_home).expect("temp home");

            std::env::set_var("HOME", &temp_home);
            std::env::set_var("USERPROFILE", &temp_home);
            for key in [
                "FLEETY_AGENT_URL",
                "FLEETY_DEVICE_ID",
                "COMPUTERNAME",
                "HOSTNAME",
            ] {
                std::env::remove_var(key);
            }

            Self { saved, temp_home }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            let _ = std::fs::remove_dir_all(&self.temp_home);
        }
    }

    #[test]
    fn token_roundtrip_trims_empty_and_clear_removes_file() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("token");

        assert!(read_saved_token().is_none());

        let path = token_path().expect("token path");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("token dir");
        std::fs::write(&path, "  saved-token\n").expect("seed token");
        assert_eq!(read_saved_token().as_deref(), Some("saved-token"));

        std::fs::write(&path, " \n").expect("empty token");
        assert!(read_saved_token().is_none());

        write_saved_token("fresh-token").expect("write token");
        assert_eq!(read_saved_token().as_deref(), Some("fresh-token"));
        clear_saved_token();
        assert!(!path.exists());
    }

    #[test]
    fn agent_url_prefers_env_then_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("agent-url");

        assert_eq!(agent_url(), "ws://127.0.0.1:8787");
        std::env::set_var("FLEETY_AGENT_URL", "ws://agent");
        assert_eq!(agent_url(), "ws://agent");
    }

    #[test]
    fn device_id_prefers_explicit_env_and_has_default() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::new("device-id");

        assert_eq!(device_id(), "fleetyd-device");
        std::env::set_var("HOSTNAME", "host-a");
        assert_eq!(device_id(), "host-a");
        std::env::set_var("COMPUTERNAME", "computer-a");
        assert_eq!(device_id(), "computer-a");
        std::env::set_var("FLEETY_DEVICE_ID", "device-a");
        assert_eq!(device_id(), "device-a");
    }
}
