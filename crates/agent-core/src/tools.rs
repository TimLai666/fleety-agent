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
