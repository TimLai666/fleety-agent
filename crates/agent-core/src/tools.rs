//! Tools the agent can call, and a registry to hold them.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::model::ToolSpec;
use crate::{CoreError, Result};

/// Which tools the model is currently shown.
///
/// This is a **context budget, never an authorization boundary**: it decides
/// what appears in the request, and nothing else. Whether a call may run is
/// still decided by the tool's risk class and the approval gate, so narrowing
/// this can never grant or revoke a permission — see [`ToolRegistry::call`],
/// which does not consult it.
///
/// Unrestricted (the default) means every registered tool is offered, which is
/// how every caller that never touches this behaves. The handle is shared and
/// interior-mutable so a tool can widen the set mid-turn — the loop re-reads
/// [`ToolRegistry::specs`] before each model call, so an activation is visible
/// to the very next call of the same turn.
#[derive(Clone, Default)]
pub struct ActiveTools(Arc<Mutex<Option<BTreeSet<String>>>>);

impl ActiveTools {
    /// Whether every registered tool is offered (no restriction set).
    pub fn is_unrestricted(&self) -> bool {
        self.read().is_none()
    }

    /// Replace the offered set. Names that are not registered are simply never
    /// matched — see [`ToolRegistry::specs`].
    pub fn restrict_to(&self, names: impl IntoIterator<Item = String>) {
        *self.write() = Some(names.into_iter().collect());
    }

    /// Add to the offered set. On an unrestricted handle this is a no-op: every
    /// tool is already offered, so there is nothing to widen.
    pub fn activate(&self, names: impl IntoIterator<Item = String>) {
        if let Some(active) = self.write().as_mut() {
            active.extend(names);
        }
    }

    /// Whether `name` is currently offered to the model.
    pub fn offers(&self, name: &str) -> bool {
        match self.read().as_ref() {
            Some(active) => active.contains(name),
            None => true,
        }
    }

    /// The current set, or `None` when unrestricted.
    pub fn snapshot(&self) -> Option<BTreeSet<String>> {
        self.read().clone()
    }

    /// A poisoned lock is not worth failing a turn over: the value behind it is
    /// a plain set with no invariant to violate, so recover it and carry on.
    fn read(&self) -> std::sync::MutexGuard<'_, Option<BTreeSet<String>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::MutexGuard<'_, Option<BTreeSet<String>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

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
    active: ActiveTools,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The shared handle deciding which tools are offered to the model. Clone it
    /// to let a tool widen the set while the turn runs. Unrestricted by default,
    /// so a registry nobody configures behaves exactly as it always has.
    pub fn active_tools(&self) -> ActiveTools {
        self.active.clone()
    }

    /// Register a tool (later registration of the same name wins).
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.spec().name;
        self.tools.insert(name, tool);
    }

    /// Specs for the tools the model is told it can call.
    ///
    /// Unrestricted (the default) yields every registered tool. When a set is in
    /// effect, only its members are offered — and a name in the set that is not
    /// registered is simply never matched, so activation state written by a
    /// different build cannot break a conversation.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .filter(|(name, _)| self.active.offers(name))
            .map(|(_, tool)| tool.spec())
            .collect()
    }

    /// Remove and return every registered tool, leaving the registry empty. A
    /// neutral accessor for callers that need to transform tools in bulk (e.g.
    /// wrap them) and re-[`register`](Self::register) the results; carries no
    /// policy of its own.
    pub fn drain(&mut self) -> Vec<Box<dyn Tool>> {
        self.tools.drain().map(|(_, tool)| tool).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Call a tool by name. Returns an actionable [`CoreError::ToolNotFound`] if
    /// the name is not registered.
    ///
    /// Deliberately does **not** consult [`ActiveTools`]: activation decides what
    /// the model is shown, not what may run. Gating execution on it too would put
    /// a second, weaker permission source beside the approval gate, and would
    /// spuriously refuse a tool in the window between activating it and the model
    /// being shown it.
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

    fn three_tools() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for name in ["read_file", "browser_open", "tool_search"] {
            registry.register(Box::new(StaticTool {
                name,
                output: json!(name),
            }));
        }
        registry
    }

    fn spec_names(registry: &ToolRegistry) -> Vec<String> {
        let mut names: Vec<String> = registry.specs().into_iter().map(|s| s.name).collect();
        names.sort();
        names
    }

    /// No activation state → every registered tool, exactly as before this
    /// capability existed. Every caller that never sets one relies on this.
    #[test]
    fn unset_activation_offers_every_tool() {
        let registry = three_tools();
        assert!(registry.active_tools().is_unrestricted());
        assert_eq!(
            spec_names(&registry),
            vec!["browser_open", "read_file", "tool_search"]
        );
    }

    /// A set activation state narrows what the model is shown.
    #[test]
    fn activation_narrows_the_offered_specs() {
        let registry = three_tools();
        registry
            .active_tools()
            .restrict_to(["tool_search".to_string(), "read_file".to_string()]);
        assert_eq!(spec_names(&registry), vec!["read_file", "tool_search"]);

        // Activating adds to the set rather than replacing it.
        registry
            .active_tools()
            .activate(["browser_open".to_string()]);
        assert_eq!(
            spec_names(&registry),
            vec!["browser_open", "read_file", "tool_search"]
        );
    }

    /// State written by another version can name tools this build does not have;
    /// that must be ignored, not fatal.
    #[test]
    fn activation_ignores_unregistered_names() {
        let registry = three_tools();
        registry
            .active_tools()
            .restrict_to(["read_file".to_string(), "from_the_future".to_string()]);
        assert_eq!(spec_names(&registry), vec!["read_file"]);
    }

    /// Activation is a context budget, not an authorization boundary: execution
    /// eligibility stays with the risk class and the approval gate.
    #[tokio::test]
    async fn call_ignores_activation() {
        let registry = three_tools();
        registry
            .active_tools()
            .restrict_to(["tool_search".to_string()]);
        // Not offered to the model...
        assert_eq!(spec_names(&registry), vec!["tool_search"]);
        // ...but still callable: the registry does not gate execution.
        let value = registry
            .call("browser_open", json!({}))
            .await
            .expect("a registered tool runs regardless of activation");
        assert_eq!(value["output"], json!("browser_open"));
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
