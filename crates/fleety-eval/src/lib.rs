//! fleety-eval — offline regression harness for the agent loop.
//!
//! A *golden* is a recorded conversation: a fresh workspace, a single user
//! message, the assistant responses the model would have produced (the
//! `scripted_responses`), and a set of assertions about how the loop actually
//! ran (which tools were called, the final reply contains certain phrases, no
//! forbidden tools were invoked). The runner replays each golden with a real
//! tool registry against a [`MockProvider`](agent_core::MockProvider) that
//! feeds the scripted responses back; the workspace is set up on disk so any
//! real tool side effects happen for real.
//!
//! Golden files are JSONL — one [`Golden`] per line. Run via the bin:
//! `cargo run -p fleety-eval -- run path/to/file.jsonl …`.
//!
//! The harness is deterministic and offline (no real model calls). It catches
//! regressions where a previously-correct tool sequence changes shape, a tool
//! gets accidentally dropped, or a workspace-tool side effect stops happening.

#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod golden;
pub mod runner;

pub use golden::{Expected, Golden, ScriptedResponse};
pub use runner::{run_file, run_one, Verdict};
