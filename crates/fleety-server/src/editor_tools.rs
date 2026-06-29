//! Editor-backed tools: the agent's `editor_*` tools for an ACP-delegated
//! conversation, which execute in the user's editor (its fs/terminal) rather than
//! on disk. Each is a thin proxy that routes a `RunTool` to the originating ACP
//! connection (addressed by its unique per-connection id) and returns the
//! editor's result — reusing the same hub/pending machinery as `device_exec`.
//!
//! The CLI is the source of truth for which `editor_*` tools exist (it gates them
//! by the editor's advertised ACP capabilities and sends the specs in `Hello`); the
//! server simply proxies the advertised ones to that connection.

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use agent_core::{Result, Tool, ToolRegistry, ToolSpec};

use crate::bridge::Pending;

struct EditorTool {
    spec: ToolSpec,
    /// This connection's outbound sender — the per-connection address, so two
    /// editors on one machine never cross-talk.
    out: mpsc::UnboundedSender<WsMessage>,
    pending: Pending,
}

#[async_trait]
impl Tool for EditorTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn call(&self, args: Value) -> Result<Value> {
        crate::bridge::route_run_tool_via(&self.out, &self.pending, &self.spec.name, args).await
    }
}

/// Register the advertised `editor_*` tools as proxies routed to this connection's
/// `out` sender. Only specs whose name starts with `editor_` are taken (the
/// editor-backed subset the CLI gated by capability); everything else is ignored.
/// Returns how many were registered.
pub fn register_editor(
    registry: &mut ToolRegistry,
    out: &mpsc::UnboundedSender<WsMessage>,
    pending: Pending,
    advertised: &[ToolSpec],
) -> usize {
    let mut n = 0;
    for spec in advertised {
        if spec.name.starts_with("editor_") {
            registry.register(Box::new(EditorTool {
                spec: spec.clone(),
                out: out.clone(),
                pending: pending.clone(),
            }));
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{dispatch_result, new_pending};
    use agent_core::RiskLevel;
    use serde_json::json;

    fn editor_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: "editor tool".to_string(),
            parameters: json!({ "type": "object" }),
            risk: RiskLevel::Mutate,
        }
    }

    #[tokio::test]
    async fn registers_only_editor_tools_and_routes_to_connection() {
        let pending = new_pending();
        let mut reg = ToolRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        // Only the editor_* specs are registered (a stray non-editor spec is skipped).
        let advertised = vec![
            editor_spec("editor_read_file"),
            editor_spec("editor_run"),
            editor_spec("read_file"), // not an editor tool → ignored
        ];
        let n = register_editor(&mut reg, &tx, pending.clone(), &advertised);
        assert_eq!(n, 2);
        assert!(reg.contains("editor_read_file"));
        assert!(reg.contains("editor_run"));
        assert!(!reg.contains("read_file"));

        // Answer the RunTool the proxy forwards to this connection's sender.
        let pending2 = pending.clone();
        let responder = tokio::spawn(async move {
            if let Some(WsMessage::Text(frame)) = rx.recv().await {
                let v: Value = serde_json::from_str(&frame).unwrap();
                let call_id = v["call_id"].as_str().unwrap().to_string();
                // The proxy forwarded the tool name + args to this connection.
                assert_eq!(v["tool"], "editor_read_file");
                dispatch_result(&pending2, &call_id, Ok(json!({ "content": "hi" }))).await;
            }
        });
        let out = reg
            .call("editor_read_file", json!({ "path": "a.rs" }))
            .await
            .expect("call");
        assert_eq!(out["content"], "hi");
        responder.await.unwrap();
    }
}
