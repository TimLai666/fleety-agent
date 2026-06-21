//! Approval gating for tool execution.
//!
//! Under `Policy::FullAccess` (the default) read/mutate run freely. Under
//! `Policy::RequireApproval`, any non-read tool is sent to an [`ApprovalGate`]
//! before it runs; a denial is fed back to the model as a tool result instead of
//! executing. This is how critical/irreversible actions are confirmed.

use async_trait::async_trait;
use serde_json::Value;

use crate::model::RiskLevel;
use crate::Result;

/// How tool execution is gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Read/mutate run directly; nothing is gated (the v0 default).
    #[default]
    FullAccess,
    /// Any non-read tool requires approval before running.
    RequireApproval,
}

impl Policy {
    /// Whether a tool of the given risk must be approved before running.
    pub fn needs_approval(self, risk: RiskLevel) -> bool {
        match self {
            Policy::FullAccess => false,
            Policy::RequireApproval => !matches!(risk, RiskLevel::Read),
        }
    }
}

/// The outcome of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Decides whether a gated tool call may proceed.
#[async_trait]
pub trait ApprovalGate: Send {
    async fn request(
        &mut self,
        tool: &str,
        args: &Value,
        risk: RiskLevel,
    ) -> Result<ApprovalDecision>;
}

/// Always approves; used for `FullAccess` contexts and tests.
pub struct AutoApprove;

#[async_trait]
impl ApprovalGate for AutoApprove {
    async fn request(
        &mut self,
        _tool: &str,
        _args: &Value,
        _risk: RiskLevel,
    ) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision::Approve)
    }
}

/// Always denies; used for unattended runs (e.g. scheduled jobs) where no human
/// can confirm. Under `RequireApproval` this lets reads proceed but feeds back a
/// denial for any mutate/critical tool instead of executing it.
pub struct AutoDeny;

#[async_trait]
impl ApprovalGate for AutoDeny {
    async fn request(
        &mut self,
        _tool: &str,
        _args: &Value,
        _risk: RiskLevel,
    ) -> Result<ApprovalDecision> {
        Ok(ApprovalDecision::Deny)
    }
}
