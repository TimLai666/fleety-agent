//! agent-core — the generic agent runtime core.
//!
//! This crate is the future standalone agent framework: it will be extracted to
//! its own repository and mounted back into Fleety as a git submodule. The iron
//! rule that keeps that possible is that it depends on **no Fleety-specific
//! crate** and so always builds standalone. Dependency direction is always
//! `fleety-* -> agent-core`, never the reverse.
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod agent;
pub mod approval;
pub mod error;
pub mod event;
pub mod model;
pub mod obs;
pub mod openai;
pub mod panic;
pub mod tools;

pub use agent::{run_turn, LoopConfig, TurnOutcome};
pub use approval::{ApprovalDecision, ApprovalGate, AutoApprove, Policy};
pub use error::{CoreError, ErrorReport, Result};
pub use event::{Event, EventLog};
pub use model::{
    Message, MockProvider, ModelProvider, ModelResponse, RiskLevel, Role, ToolCall, ToolSpec,
};
pub use openai::OpenAiCompat;
pub use tools::{Tool, ToolRegistry};

/// Version of agent-core. In M0 this also stands in for the runtime version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
