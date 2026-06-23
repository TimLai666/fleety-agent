//! Tools the agent can call, and a registry to hold them.

use std::collections::HashMap;

use serde_json::Value;

use crate::model::ToolSpec;
use crate::{CoreError, Result};

/// A callable tool. Implementations describe themselves via [`Tool::spec`] and
/// run via [`Tool::call`].
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn call(&self, args: Value) -> Result<Value>;
}

/// Holds the tools available to the agent, keyed by name.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool (later registration of the same name wins).
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.spec().name;
        self.tools.insert(name, tool);
    }

    /// Specs for all registered tools (what the model is told it can call).
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Call a tool by name. Returns an actionable [`CoreError::ToolNotFound`] if
    /// the name is not registered.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        match self.tools.get(name) {
            Some(tool) => tool.call(args).await,
            None => Err(CoreError::ToolNotFound(name.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct StaticTool {
        name: &'static str,
        output: Value,
    }

    #[async_trait::async_trait]
    impl Tool for StaticTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.to_string(),
                description: "static test tool".to_string(),
                parameters: json!({"type":"object"}),
                risk: crate::model::RiskLevel::Read,
            }
        }

        async fn call(&self, args: Value) -> Result<Value> {
            Ok(json!({
                "args": args,
                "output": self.output,
            }))
        }
    }

    #[tokio::test]
    async fn registry_calls_registered_tool_and_replaces_duplicates() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(StaticTool {
            name: "echo",
            output: json!("first"),
        }));
        registry.register(Box::new(StaticTool {
            name: "echo",
            output: json!("second"),
        }));

        assert!(registry.contains("echo"));
        assert_eq!(registry.specs().len(), 1);

        let value = registry
            .call("echo", json!({"x": 1}))
            .await
            .expect("registered tool should run");
        assert_eq!(value["args"], json!({"x": 1}));
        assert_eq!(value["output"], json!("second"));
    }

    #[tokio::test]
    async fn registry_reports_missing_tool_as_actionable_error() {
        let registry = ToolRegistry::new();
        let err = registry
            .call("missing", Value::Null)
            .await
            .expect_err("unknown tool should error");

        let report = err.report();
        assert_eq!(report.kind, "tool_not_found");
        assert!(report.message.contains("missing"));
    }
}
