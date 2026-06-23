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
pub fn register(registry: &mut ToolRegistry, hub: Hub, pending: Pending, handles: Handles) {
    registry.register(Box::new(DeviceExec {
        hub,
        pending,
        handles,
    }));
}

struct DeviceExec {
    hub: Hub,
    pending: Pending,
    handles: Handles,
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

        let call_id = uuid::Uuid::new_v4().to_string();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending.lock().await.insert(call_id.clone(), reply_tx);

        let frame = ServerMsg::RunTool {
            call_id: call_id.clone(),
            tool: tool.to_string(),
            args_json: tool_args.to_string(),
        };
        let text = serde_json::to_string(&frame)
            .map_err(|e| CoreError::Message(format!("serialize RunTool: {e}")))?;
        {
            let hub = self.hub.lock().await;
            let sender = hub.get(device).ok_or_else(|| {
                CoreError::Message(format!(
                    "device '{device}' is not connected; use device_list to see connected devices"
                ))
            })?;
            sender
                .send(WsMessage::Text(text))
                .map_err(|_| CoreError::Provider(format!("device '{device}' connection closed")))?;
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
}
