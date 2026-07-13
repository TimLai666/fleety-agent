//! Cross-device tool routing (the `client_session` bridge).
//!
//! Each connection registers an outbound sender in the [`Hub`] keyed by its
//! `device_id`. The `device_exec` tool sends a `RunTool` frame to a target
//! device's connection and awaits the daemon's `ToolResult`/`ToolError`,
//! correlated by `call_id` through [`Pending`]. This lets the agent operate any
//! connected device, not just the server's own workspace.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};
use fleety_protocol::ServerMsg;

/// Active device connections: `device_id -> outbound frame sender`.
pub type Hub = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<WsMessage>>>>;
/// In-flight on-device calls: `call_id -> reply channel` (Ok(value) / Err(msg)).
pub type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>>>;
/// Stateful tool handles bound to a device: `handle_id -> owning device_id`.
/// Enforces the device-scoping invariant — a handle made on one device can't be
/// used against another.
pub type Handles = Arc<Mutex<HashMap<String, String>>>;
/// Tool specs each connected device advertises in its `Hello`, keyed by
/// `device_id`. Lets the agent see what `device_exec` can invoke per device
/// (instead of guessing) and lets `device_show` enumerate device capabilities.
pub type DeviceTools = Arc<Mutex<HashMap<String, Vec<ToolSpec>>>>;

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub fn new_hub() -> Hub {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn new_pending() -> Pending {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn new_handles() -> Handles {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn new_device_tools() -> DeviceTools {
    Arc::new(Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod strict_tests {
    use super::*;
    use agent_core::ToolSpec;

    #[tokio::test]
    async fn device_exec_strict_rejects_unadvertised_tool() {
        let hub = new_hub();
        let pending = new_pending();
        let handles = new_handles();
        let device_tools = new_device_tools();
        // Pretend "pi" advertised only read_file and list_dir.
        device_tools.lock().await.insert(
            "pi".to_string(),
            vec![
                ToolSpec {
                    name: "read_file".to_string(),
                    description: String::new(),
                    parameters: serde_json::json!({}),
                    risk: agent_core::RiskLevel::Read,
                },
                ToolSpec {
                    name: "list_dir".to_string(),
                    description: String::new(),
                    parameters: serde_json::json!({}),
                    risk: agent_core::RiskLevel::Read,
                },
            ],
        );

        let mut registry = ToolRegistry::new();
        register(&mut registry, hub, pending, handles, device_tools);
        let err = registry
            .call(
                "device_exec",
                serde_json::json!({
                    "device": "pi",
                    "tool": "rm_all_the_things",
                    "args": {}
                }),
            )
            .await
            .expect_err("should reject");
        let msg = err.report().message;
        assert!(msg.contains("did not advertise"), "msg: {msg}");
        assert!(msg.contains("read_file"), "lists advertised tools: {msg}");
    }

    #[tokio::test]
    async fn device_exec_lets_advertised_tool_through_to_dispatch() {
        // The dispatch itself will fail (no hub entry), but it must get *past*
        // the strict-name check before failing on "not connected".
        let hub = new_hub();
        let pending = new_pending();
        let handles = new_handles();
        let device_tools = new_device_tools();
        device_tools.lock().await.insert(
            "pi".to_string(),
            vec![ToolSpec {
                name: "read_file".to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
                risk: agent_core::RiskLevel::Read,
            }],
        );

        let mut registry = ToolRegistry::new();
        register(&mut registry, hub, pending, handles, device_tools);
        let err = registry
            .call(
                "device_exec",
                serde_json::json!({
                    "device": "pi",
                    "tool": "read_file",
                    "args": {}
                }),
            )
            .await
            .expect_err("dispatch fails (no hub entry)");
        let msg = err.report().message;
        assert!(
            msg.contains("not connected"),
            "should pass strict-name check then fail dispatch: {msg}"
        );
    }

    #[tokio::test]
    async fn device_exec_skips_strict_check_when_device_never_advertised() {
        // A legacy device that didn't advertise tools must still be reachable
        // (backward compatibility). It'll fail on "not connected" rather than
        // on "did not advertise".
        let hub = new_hub();
        let pending = new_pending();
        let handles = new_handles();
        let device_tools = new_device_tools();

        let mut registry = ToolRegistry::new();
        register(&mut registry, hub, pending, handles, device_tools);
        let err = registry
            .call(
                "device_exec",
                serde_json::json!({
                    "device": "legacy",
                    "tool": "anything",
                    "args": {}
                }),
            )
            .await
            .expect_err("dispatch fails");
        let msg = err.report().message;
        assert!(!msg.contains("did not advertise"), "msg: {msg}");
        assert!(msg.contains("not connected"), "msg: {msg}");
    }
}

/// Reject using a handle bound to a different device (actionable: owning device
/// + two remediation paths).
fn check_handle(handles: &HashMap<String, String>, handle: &str, device: &str) -> Result<()> {
    if let Some(owner) = handles.get(handle) {
        if owner != device {
            return Err(CoreError::Message(format!(
                "handle '{handle}' belongs to device '{owner}', not '{device}'. Either target device '{owner}' for this handle, or open a fresh handle on '{device}'."
            )));
        }
    }
    Ok(())
}

/// Register the `device_exec` routing tool.
pub fn register(
    registry: &mut ToolRegistry,
    hub: Hub,
    pending: Pending,
    handles: Handles,
    device_tools: DeviceTools,
) {
    registry.register(Box::new(DeviceExec {
        hub,
        pending,
        handles,
        device_tools,
    }));
}

struct DeviceExec {
    hub: Hub,
    pending: Pending,
    handles: Handles,
    /// The map of advertised on-device tool specs. When a device advertised at
    /// Hello (most do; legacy/CLI sessions don't), we strict-check the tool
    /// name against the list before bothering to dispatch — fail-fast beats a
    /// 30-second timeout.
    device_tools: DeviceTools,
}

#[async_trait]
impl Tool for DeviceExec {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_exec".to_string(),
            description: "Run a tool on another connected device by id (routes to that device's daemon). On-device tools: read_file, list_dir, write_file, run_command.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string", "description": "target device_id (see device_list)" },
                    "tool": { "type": "string" },
                    "args": { "type": "object" },
                    "handle": { "type": "string", "description": "a stateful handle from a prior call; must belong to this device" }
                },
                "required": ["device", "tool"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = args.get("device").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'device'".to_string())
        })?;
        let tool = args.get("tool").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'tool'".to_string())
        })?;
        let tool_args = args.get("args").cloned().unwrap_or_else(|| json!({}));

        // Device-scoping: a supplied handle must belong to the target device.
        if let Some(handle) = args.get("handle").and_then(Value::as_str) {
            check_handle(&*self.handles.lock().await, handle, device)?;
        }

        // Strict tool name check against the device's advertised list — but
        // only when the device actually advertised. A device that connected
        // without `local_tools_json` (older fleetyd / interactive CLI) is
        // treated as "any tool" so we don't break backward compatibility.
        {
            let specs = self.device_tools.lock().await;
            if let Some(advertised) = specs.get(device) {
                if !advertised.is_empty() && !advertised.iter().any(|s| s.name == tool) {
                    let names: Vec<&str> = advertised.iter().map(|s| s.name.as_str()).collect();
                    return Err(CoreError::Message(format!(
                        "device '{device}' did not advertise tool '{tool}'. \
                         Available on that device: {}",
                        names.join(", ")
                    )));
                }
            }
        }

        let call_id = uuid::Uuid::new_v4().to_string();
        let (reply_tx, reply_rx) = oneshot::channel();
        let sender = {
            let hub = self.hub.lock().await;
            hub.get(device).cloned().ok_or_else(|| {
                CoreError::Message(format!(
                    "device '{device}' is not connected; use device_list to see connected devices"
                ))
            })?
        };
        self.pending.lock().await.insert(call_id.clone(), reply_tx);

        let frame = ServerMsg::RunTool {
            call_id: call_id.clone(),
            tool: tool.to_string(),
            args_json: tool_args.to_string(),
        };
        let text = serde_json::to_string(&frame)
            .map_err(|e| CoreError::Message(format!("serialize RunTool: {e}")))?;
        if sender.send(WsMessage::Text(text)).is_err() {
            self.pending.lock().await.remove(&call_id);
            return Err(CoreError::Provider(format!(
                "device '{device}' connection closed"
            )));
        }

        match tokio::time::timeout(CALL_TIMEOUT, reply_rx).await {
            Ok(Ok(Ok(value))) => {
                // Bind any handle the device returned to this device (scoping).
                if let Some(handle) = value.get("handle").and_then(Value::as_str) {
                    self.handles
                        .lock()
                        .await
                        .insert(handle.to_string(), device.to_string());
                }
                Ok(value)
            }
            Ok(Ok(Err(message))) => Err(CoreError::Provider(format!(
                "device '{device}' tool failed: {message}"
            ))),
            Ok(Err(_)) => Err(CoreError::Provider(format!(
                "device '{device}' reply channel dropped (disconnected?)"
            ))),
            Err(_) => {
                self.pending.lock().await.remove(&call_id);
                Err(CoreError::Provider(format!(
                    "device '{device}' tool '{tool}' timed out after {}s",
                    CALL_TIMEOUT.as_secs()
                )))
            }
        }
    }
}

/// Send a `RunTool` directly to a connection's outbound `sender` and await its
/// `ToolResult`/`ToolError`, correlated by a fresh `call_id`. The sender *is* the
/// per-connection address — multiple connections on one machine never collide.
/// Shared by `device_exec` (which looks the sender up in the hub) and the
/// editor-backed tools (which hold their connection's sender directly).
pub async fn route_run_tool_via(
    sender: &mpsc::UnboundedSender<WsMessage>,
    pending: &Pending,
    tool: &str,
    args: Value,
) -> Result<Value> {
    let call_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = oneshot::channel();
    pending.lock().await.insert(call_id.clone(), reply_tx);

    let frame = ServerMsg::RunTool {
        call_id: call_id.clone(),
        tool: tool.to_string(),
        args_json: args.to_string(),
    };
    let text = serde_json::to_string(&frame)
        .map_err(|e| CoreError::Message(format!("serialize RunTool: {e}")))?;
    sender
        .send(WsMessage::Text(text))
        .map_err(|_| CoreError::Provider(format!("tool '{tool}': connection closed")))?;
    match tokio::time::timeout(CALL_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(message))) => Err(CoreError::Provider(format!(
            "tool '{tool}' failed: {message}"
        ))),
        Ok(Err(_)) => Err(CoreError::Provider(format!(
            "tool '{tool}': reply channel dropped (disconnected?)"
        ))),
        Err(_) => {
            pending.lock().await.remove(&call_id);
            Err(CoreError::Provider(format!(
                "tool '{tool}' timed out after {}s",
                CALL_TIMEOUT.as_secs()
            )))
        }
    }
}

/// Route a reserved server operation to the daemon that owns `device` without
/// exposing the operation through the public device tool registry.
pub async fn route_run_tool_to_device(
    hub: &Hub,
    pending: &Pending,
    device: &str,
    tool: &str,
    args: Value,
) -> Result<Value> {
    let sender = hub.lock().await.get(device).cloned().ok_or_else(|| {
        CoreError::Message(format!(
            "device daemon '{device}' is not connected; no local config fallback is permitted"
        ))
    })?;
    route_run_tool_via(&sender, pending, tool, args).await
}

/// Register the `transfer_file` relay tool. Needs the server's own workspace
/// root + backups for the `server` endpoint, and the hub/pending to dispatch to
/// device endpoints.
pub fn register_transfer(
    registry: &mut ToolRegistry,
    hub: Hub,
    pending: Pending,
    root: std::path::PathBuf,
    backups: std::path::PathBuf,
) {
    registry.register(Box::new(TransferFile {
        hub,
        pending,
        root,
        backups,
    }));
}

/// Whether an endpoint string names the server itself (its own workspace) rather
/// than a connected device. `server` or empty → server. Pure.
pub(crate) fn is_server_endpoint(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.eq_ignore_ascii_case("server")
}

struct TransferFile {
    hub: Hub,
    pending: Pending,
    root: std::path::PathBuf,
    backups: std::path::PathBuf,
}

impl TransferFile {
    /// The outbound sender for a connected device, or an error if it isn't in
    /// the hub.
    async fn device_sender(&self, device: &str) -> Result<mpsc::UnboundedSender<WsMessage>> {
        self.hub.lock().await.get(device).cloned().ok_or_else(|| {
            CoreError::Message(format!(
                "device '{device}' is not connected; use device_list to see connected devices"
            ))
        })
    }
}

#[async_trait]
impl Tool for TransferFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "transfer_file".to_string(),
            description: "Copy a single file between two endpoints — each is a connected device \
                          (its device_id) or the server (`server`). Reads the source bytes and \
                          writes them to the destination, verifying SHA-256 (binary-safe, bounded \
                          by FLEETY_TRANSFER_MAX_BYTES). Backs up an existing destination."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "source device_id, or `server`" },
                    "from_path": { "type": "string", "description": "workspace-relative source path" },
                    "to": { "type": "string", "description": "destination device_id, or `server`" },
                    "to_path": { "type": "string", "description": "workspace-relative destination path" },
                    "overwrite": { "type": "boolean", "description": "replace an existing destination (default true; a backup is kept)" }
                },
                "required": ["from", "from_path", "to", "to_path"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let s = |k: &str| -> Result<String> {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    CoreError::Message(format!("missing required string argument '{k}'"))
                })
        };
        let from = s("from")?;
        let from_path = s("from_path")?;
        let to = s("to")?;
        let to_path = s("to_path")?;
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        // Read the source bytes (server-local via the shared helper, or a device
        // via the on-device `read_file_bytes`).
        let read = if is_server_endpoint(&from) {
            fleety_tools::read_file_bytes_at(&self.root, &from_path)?
        } else {
            let sender = self.device_sender(&from).await?;
            route_run_tool_via(
                &sender,
                &self.pending,
                "read_file_bytes",
                json!({ "path": from_path }),
            )
            .await?
        };
        let content_b64 = read
            .get("content_b64")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Message("source did not return file bytes (old daemon?)".to_string())
            })?;
        let src_sha = read
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // Write to the destination (server-local or device), then verify the
        // hashes match — a mismatch is a corrupted relay, not a success.
        let write = if is_server_endpoint(&to) {
            fleety_tools::write_file_bytes_at(
                &self.root,
                &self.backups,
                &to_path,
                content_b64,
                overwrite,
            )?
        } else {
            let sender = self.device_sender(&to).await?;
            route_run_tool_via(
                &sender,
                &self.pending,
                "write_file_bytes",
                json!({ "path": to_path, "content_b64": content_b64, "overwrite": overwrite }),
            )
            .await?
        };
        let dst_sha = write
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !src_sha.is_empty() && dst_sha != src_sha {
            return Err(CoreError::Message(format!(
                "transfer corrupted: source sha256 {src_sha} != destination {dst_sha} — the \
                 destination backup can be rolled back; retry the transfer"
            )));
        }
        Ok(json!({
            "ok": true,
            "bytes": read.get("bytes").cloned().unwrap_or(Value::Null),
            "sha256": src_sha,
            "from": from,
            "to": to,
        }))
    }
}

/// Dispatch a daemon's tool reply to the waiting `device_exec` call.
pub async fn dispatch_result(
    pending: &Pending,
    call_id: &str,
    result: std::result::Result<Value, String>,
) {
    if let Some(tx) = pending.lock().await.remove(call_id) {
        let _ = tx.send(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::ToolRegistry;

    #[test]
    fn server_endpoint_classification() {
        assert!(is_server_endpoint("server"));
        assert!(is_server_endpoint("SERVER"));
        assert!(is_server_endpoint(""));
        assert!(is_server_endpoint("  "));
        assert!(!is_server_endpoint("pi"));
        assert!(!is_server_endpoint("laptop-01"));
    }

    #[tokio::test]
    async fn transfer_server_to_server_copies_and_verifies() {
        let root = std::env::temp_dir().join(format!("fleety-xfer-{}", uuid::Uuid::new_v4()));
        let backups = std::env::temp_dir().join(format!("fleety-xferbk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("mk root");
        // A binary source the text tools couldn't move.
        std::fs::write(root.join("src.bin"), [0u8, 0xff, 0x42, 0x00]).expect("seed");

        let tool = TransferFile {
            hub: new_hub(),
            pending: new_pending(),
            root: root.clone(),
            backups: backups.clone(),
        };
        // server → server: both endpoints are the local workspace.
        let out = tool
            .call(json!({
                "from": "server", "from_path": "src.bin",
                "to": "server", "to_path": "dst.bin"
            }))
            .await
            .expect("transfer");
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["bytes"], json!(4));
        assert_eq!(
            std::fs::read(root.join("dst.bin")).expect("read dst"),
            vec![0u8, 0xff, 0x42, 0x00]
        );
        // A missing source surfaces a readable error, not a panic.
        assert!(tool
            .call(json!({"from":"server","from_path":"nope.bin","to":"server","to_path":"x.bin"}))
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&backups);
    }

    #[test]
    fn handle_scoping_rejects_cross_device() {
        let mut map = HashMap::new();
        map.insert("sess1".to_string(), "pi".to_string());
        // same device: ok; unknown handle: ok (not yet bound).
        assert!(check_handle(&map, "sess1", "pi").is_ok());
        assert!(check_handle(&map, "other", "laptop").is_ok());
        // cross-device: rejected, naming the owner.
        let err = check_handle(&map, "sess1", "laptop").expect_err("cross-device");
        assert!(err.report().message.contains("pi"));
    }

    #[tokio::test]
    async fn device_exec_validates_args_and_connection_before_dispatch() {
        let hub = new_hub();
        let pending = new_pending();
        let handles = new_handles();
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            hub.clone(),
            pending.clone(),
            handles,
            new_device_tools(),
        );

        let missing_device = registry
            .call("device_exec", json!({ "tool": "read_file" }))
            .await
            .expect_err("device is required");
        assert!(missing_device.report().message.contains("'device'"));

        let missing_tool = registry
            .call("device_exec", json!({ "device": "dev" }))
            .await
            .expect_err("tool is required");
        assert!(missing_tool.report().message.contains("'tool'"));

        let disconnected = registry
            .call(
                "device_exec",
                json!({ "device": "dev", "tool": "read_file" }),
            )
            .await
            .expect_err("device is not connected");
        assert!(disconnected.report().message.contains("not connected"));
        assert!(
            pending.lock().await.is_empty(),
            "failed dispatch should not leave pending calls"
        );

        let (tx, rx) = mpsc::unbounded_channel::<WsMessage>();
        hub.lock().await.insert("dev".to_string(), tx);
        drop(rx);
        let closed = registry
            .call(
                "device_exec",
                json!({ "device": "dev", "tool": "read_file" }),
            )
            .await
            .expect_err("closed device sender");
        assert!(closed.report().message.contains("connection closed"));
        assert!(
            pending.lock().await.is_empty(),
            "closed sender should not leave pending calls"
        );
    }

    #[tokio::test]
    async fn device_exec_sends_run_tool_with_default_args_and_binds_returned_handle() {
        let hub = new_hub();
        let pending = new_pending();
        let handles = new_handles();
        let mut registry = ToolRegistry::new();
        register(
            &mut registry,
            hub.clone(),
            pending.clone(),
            handles.clone(),
            new_device_tools(),
        );

        let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();
        hub.lock().await.insert("dev".to_string(), tx);

        let call = tokio::spawn(async move {
            registry
                .call(
                    "device_exec",
                    json!({ "device": "dev", "tool": "open_handle" }),
                )
                .await
        });

        let frame = rx.recv().await.expect("run tool frame");
        let text = frame.to_text().expect("text frame");
        let msg: ServerMsg = serde_json::from_str(text).expect("server msg");
        let call_id = match msg {
            ServerMsg::RunTool {
                call_id,
                tool,
                args_json,
            } => {
                assert_eq!(tool, "open_handle");
                assert_eq!(args_json, "{}");
                call_id
            }
            other => panic!("unexpected frame: {other:?}"),
        };
        dispatch_result(&pending, &call_id, Ok(json!({ "handle": "h1" }))).await;

        let result = call.await.expect("join").expect("tool result");
        assert_eq!(result["handle"], json!("h1"));
        assert_eq!(
            handles.lock().await.get("h1").map(String::as_str),
            Some("dev")
        );
    }
}
