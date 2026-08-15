//! Approval gating for tool execution.
//!
//! Under the default `Policy::AutoReview`, read tools run directly and every
//! non-read tool is sent to an [`ApprovalGate`] before it runs. Explicit
//! `Policy::FullAccess` and `Policy::RequireApproval` values retain their
//! direct and interactive behaviors.

use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::Value;

use crate::model::RiskLevel;
use crate::Result;

/// How tool execution is gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Non-read tools are reviewed by an unattended policy gate (the default).
    #[default]
    AutoReview,
    /// Read/mutate run directly; nothing is gated.
    FullAccess,
    /// Any non-read tool requires approval before running.
    RequireApproval,
}

impl Policy {
    /// Whether a tool of the given risk must be approved before running.
    pub fn needs_approval(self, risk: RiskLevel) -> bool {
        match self {
            Policy::FullAccess => false,
            Policy::RequireApproval | Policy::AutoReview => !matches!(risk, RiskLevel::Read),
        }
    }
}

/// A deterministic warning produced before a candidate reaches an unattended
/// review gate. The message is trusted runtime evidence, not candidate text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DangerSignal {
    pub code: String,
    pub message: String,
}

/// Bounded context supplied to an approval gate for one candidate call.
///
/// The server-side auto-review gate is responsible for redacting and bounding
/// values before they enter a model prompt. Other gates may ignore the context.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewContext {
    pub objective: String,
    pub conversation_context: String,
    pub tool: String,
    pub arguments: Value,
    pub risk: RiskLevel,
    pub danger_signals: Vec<DangerSignal>,
}

/// The outcome of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Sanitized metadata emitted by a policy gate for the audit trail.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalAudit {
    pub details: Value,
}

/// Decides whether a gated tool call may proceed.
#[async_trait]
pub trait ApprovalGate: Send {
    async fn request(&mut self, context: &ReviewContext) -> Result<ApprovalDecision>;

    /// Return metadata for the most recent request, when the gate has an
    /// auditable decision. Fixed and interactive gates retain their behavior.
    fn take_audit(&mut self) -> Option<ApprovalAudit> {
        None
    }
}

/// Always approves; used for `FullAccess` contexts and tests.
pub struct AutoApprove;

#[async_trait]
impl ApprovalGate for AutoApprove {
    async fn request(&mut self, _context: &ReviewContext) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approve)
    }
}

/// Always denies; used for unattended runs (e.g. scheduled jobs) where no human
/// can confirm. Under `RequireApproval` this lets reads proceed but feeds back a
/// denial for any mutate/critical tool instead of executing it.
pub struct AutoDeny;

#[async_trait]
impl ApprovalGate for AutoDeny {
    async fn request(&mut self, _context: &ReviewContext) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision::Deny)
    }
}

/// Approves only tools named in a mandate (the set of non-read tools an
/// unattended run was authorized to use at creation time); denies everything
/// else. Reads never reach the gate under `RequireApproval`.
pub struct MandateGate {
    allowed: HashSet<String>,
}

impl MandateGate {
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }
}

#[async_trait]
impl ApprovalGate for MandateGate {
    async fn request(&mut self, context: &ReviewContext) -> Result<ApprovalDecision> {
        Ok(if self.allowed.contains(&context.tool) {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Deny
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mandate_gate_allows_only_listed_tools() {
        let mut gate = MandateGate::new(["write_file".to_string(), "run_command".to_string()]);
        assert_eq!(
            gate.request(&ReviewContext {
                tool: "write_file".to_string(),
                arguments: Value::Null,
                risk: RiskLevel::Mutate,
                objective: String::new(),
                conversation_context: String::new(),
                danger_signals: Vec::new(),
            })
            .await
            .expect("ok"),
            ApprovalDecision::Approve
        );
        assert_eq!(
            gate.request(&ReviewContext {
                tool: "delete_everything".to_string(),
                arguments: Value::Null,
                risk: RiskLevel::Critical,
                objective: String::new(),
                conversation_context: String::new(),
                danger_signals: Vec::new(),
            })
            .await
            .expect("ok"),
            ApprovalDecision::Deny
        );
    }

    #[test]
    fn policy_gates_only_non_read_tools_when_required() {
        assert!(!Policy::FullAccess.needs_approval(RiskLevel::Read));
        assert!(!Policy::FullAccess.needs_approval(RiskLevel::Mutate));
        assert!(!Policy::FullAccess.needs_approval(RiskLevel::Critical));

        assert!(!Policy::RequireApproval.needs_approval(RiskLevel::Read));
        assert!(Policy::RequireApproval.needs_approval(RiskLevel::Mutate));
        assert!(Policy::RequireApproval.needs_approval(RiskLevel::Critical));

        assert!(!Policy::AutoReview.needs_approval(RiskLevel::Read));
        assert!(Policy::AutoReview.needs_approval(RiskLevel::Mutate));
        assert!(Policy::AutoReview.needs_approval(RiskLevel::Critical));
    }

    #[test]
    fn policy_default_is_auto_review() {
        assert_eq!(Policy::default(), Policy::AutoReview);
    }

    #[test]
    fn review_context_carries_sanitized_candidate_and_trusted_signals() {
        let context = ReviewContext {
            objective: "rotate the service key".to_string(),
            conversation_context: "the service is in maintenance mode".to_string(),
            tool: "write_file".to_string(),
            arguments: serde_json::json!({"path": "<redacted>", "contents": "<redacted>"}),
            risk: RiskLevel::Critical,
            danger_signals: vec![DangerSignal {
                code: "sensitive_path".to_string(),
                message: "candidate targets a sensitive path".to_string(),
            }],
        };

        assert_eq!(context.objective, "rotate the service key");
        assert_eq!(
            context.conversation_context,
            "the service is in maintenance mode"
        );
        assert_eq!(context.tool, "write_file");
        assert_eq!(context.risk, RiskLevel::Critical);
        assert_eq!(context.danger_signals[0].code, "sensitive_path");
        assert_eq!(context.arguments["path"], "<redacted>");
    }

    #[tokio::test]
    async fn auto_gates_return_fixed_decisions() {
        let mut approve = AutoApprove;
        let mut deny = AutoDeny;

        assert_eq!(
            approve
                .request(&ReviewContext {
                    tool: "delete_file".to_string(),
                    arguments: Value::Null,
                    risk: RiskLevel::Critical,
                    objective: String::new(),
                    conversation_context: String::new(),
                    danger_signals: Vec::new(),
                })
                .await
                .expect("approve"),
            ApprovalDecision::Approve
        );
        assert_eq!(
            deny.request(&ReviewContext {
                tool: "read_file".to_string(),
                arguments: Value::Null,
                risk: RiskLevel::Read,
                objective: String::new(),
                conversation_context: String::new(),
                danger_signals: Vec::new(),
            })
            .await
            .expect("deny"),
            ApprovalDecision::Deny
        );
    }
}
