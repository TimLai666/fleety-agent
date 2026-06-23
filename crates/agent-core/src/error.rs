//! Structured, actionable errors.
//!
//! Per the spec's never-crash principle, every error states cause **and** how to
//! proceed — never a bare "failed". [`ErrorReport`] is the actionable shape;
//! [`CoreError`] is the typed error returned across the core and renders into a
//! report via [`CoreError::report`].

use serde::Serialize;

/// An actionable error report: what went wrong and how to proceed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorReport {
    /// Stable, machine-friendly category, e.g. `"panic"`, `"task"`.
    pub kind: String,
    /// Human/agent-readable description of the cause.
    pub message: String,
    /// How the caller can proceed. `None` only when nothing actionable applies.
    pub remediation: Option<String>,
}

impl ErrorReport {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            remediation: None,
        }
    }

    pub fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

/// Typed error for the agent core. Convertible to an [`ErrorReport`].
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A unit of work panicked and was isolated at a never-crash boundary.
    #[error("isolated panic: {0}")]
    Panic(String),

    /// A background task failed to run to completion (non-panic).
    #[error("task failed: {0}")]
    Task(String),

    /// A requested tool is not registered.
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    /// The model provider failed (network, API error, or unexpected response).
    #[error("model provider error: {0}")]
    Provider(String),

    /// A generic, already-described error.
    #[error("{0}")]
    Message(String),
}

impl CoreError {
    /// Render an actionable report. Every variant carries a remediation hint so
    /// callers and agents never receive a dead-end "failed".
    pub fn report(&self) -> ErrorReport {
        match self {
            CoreError::Panic(msg) => ErrorReport::new("panic", msg.clone()).with_remediation(
                "the work was isolated and the process is still alive — retry the operation or report the panic",
            ),
            CoreError::Task(msg) => ErrorReport::new("task", msg.clone()).with_remediation(
                "the background task did not complete — retry, or check the logs for the underlying cause",
            ),
            CoreError::ToolNotFound(name) => {
                ErrorReport::new("tool_not_found", format!("no tool named '{name}' is registered"))
                    .with_remediation("call a tool that exists; list the available tools and pick a valid name")
            }
            CoreError::Provider(msg) => ErrorReport::new("provider", msg.clone())
                .with_remediation("the model provider call failed — retry; if it persists, check the endpoint, key, and model id"),
            CoreError::Message(msg) => ErrorReport::new("error", msg.clone()),
        }
    }
}

/// Core result type.
pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_actionable_for_panic() {
        let report = CoreError::Panic("boom".into()).report();
        assert_eq!(report.kind, "panic");
        assert!(report.remediation.is_some());
    }

    #[test]
    fn with_remediation_sets_field() {
        let report = ErrorReport::new("io", "disk full").with_remediation("free space and retry");
        assert_eq!(report.remediation.as_deref(), Some("free space and retry"));
    }
}
