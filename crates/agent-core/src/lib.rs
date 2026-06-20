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

pub mod error;
pub mod obs;
pub mod panic;

pub use error::{CoreError, ErrorReport, Result};

/// Version of agent-core. In M0 this also stands in for the runtime version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
