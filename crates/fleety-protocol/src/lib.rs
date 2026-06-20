//! fleety-protocol — wire types shared across the Fleety client runtime and server.
//!
//! Pure data: this crate carries no logic and depends only on `serde`, so it can
//! act as the contract between components (and, later, across languages).
#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use serde::{Deserialize, Serialize};

/// Bumped when the wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 0;

/// Wire form of an actionable error (mirrors `agent_core::ErrorReport`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireError {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub remediation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_error_roundtrips() {
        let err = WireError {
            kind: "io".into(),
            message: "nope".into(),
            remediation: Some("retry".into()),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let back: WireError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, back);
    }

    #[test]
    fn remediation_omitted_when_none() {
        let err = WireError {
            kind: "x".into(),
            message: "y".into(),
            remediation: None,
        };
        let json = serde_json::to_string(&err).expect("serialize");
        assert!(!json.contains("remediation"));
    }
}
