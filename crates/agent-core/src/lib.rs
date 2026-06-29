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
pub mod compress;
pub mod error;
pub mod event;
pub mod gemini;
pub mod goal;
pub mod model;
pub mod obs;
pub mod openai;
pub mod panic;
pub mod retry;
pub mod subagent;
pub mod tools;

pub use agent::{
    is_cache_usable, run_turn, run_turn_streaming, run_turn_streaming_cached, AttentionHint,
    CompactionCache, LoopConfig, TurnOutcome, ATTENTION_SENTINEL, SPEECH_SENTINEL,
};
pub use approval::{ApprovalDecision, ApprovalGate, AutoApprove, AutoDeny, MandateGate, Policy};
pub use compress::{CacheAligner, CodeCompressor, ContextCompressor, Lang, SmartCrusher};
pub use error::{CoreError, ErrorReport, Result};
pub use event::{interrupted_tool_result, reconstruct_messages, Event, EventLog};
pub use gemini::Gemini;
pub use goal::{register_goal_tools, GoalState, Step, Terminal};
pub use model::{
    Attachment, Message, MockProvider, ModelProvider, ModelResponse, RiskLevel, Role, ToolCall,
    ToolSpec,
};
pub use openai::OpenAiCompat;
pub use subagent::{
    register_orchestration, SpawnRequest, SubagentHost, SubagentManager, SubagentMode,
    SubagentState,
};
pub use tools::{Tool, ToolRegistry};

/// Version of agent-core. In M0 this also stands in for the runtime version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
