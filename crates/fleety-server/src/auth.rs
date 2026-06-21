//! Connection authentication: per-device tokens + short-lived pairing codes.
//!
//! Enforced only when the server runs with `FLEETY_REQUIRE_AUTH`. A bootstrap
//! admin token can be set via `FLEETY_TOKEN` (use it to pair the first device).
//! State persists to `<agent-home>/auth.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const PAIRING_TTL_SECS: u64 = 600; // 10 minutes

#[derive(Default, Serialize, Deserialize)]
struct State {
    tokens: HashMap<String, String>, // token -> device_id
    pairing: HashMap<String, u64>,   // pairing code -> expiry (unix secs)
}

pub struct AuthStore {
    path: PathBuf,
    bootstrap: Option<String>,
    required: bool,
    state: Mutex<State>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl AuthStore {
    /// Load from `path`; `bootstrap` is an always-valid admin token (or None);
    /// `required` enforces auth on connect.
    pub fn load(path: PathBuf, bootstrap: Option<String>, required: bool) -> Self {
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            path,
            bootstrap: bootstrap.filter(|b| !b.is_empty()),
            required,
            state: Mutex::new(state),
        }
    }

    pub fn required(&self) -> bool {
        self.required
    }

    fn save(&self, state: &State) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create auth dir: {e}")))?;
        }
        let body = serde_json::to_string_pretty(state)
            .map_err(|e| CoreError::Message(format!("serialize auth: {e}")))?;
        std::fs::write(&self.path, body)
            .map_err(|e| CoreError::Message(format!("write auth: {e}")))?;
        Ok(())
    }

    /// The owning device for a valid token (bootstrap token -> "admin"), else None.
    pub fn verify(&self, token: &str) -> Option<String> {
        if let Some(b) = &self.bootstrap {
            if token == b {
                return Some("admin".to_string());
            }
        }
        let state = self.state.lock().ok()?;
        state.tokens.get(token).cloned()
    }

    /// Mint a short-lived pairing code.
    pub fn create_pairing(&self) -> Result<String> {
        let full = uuid::Uuid::new_v4().simple().to_string();
        let code = full[..8].to_string();
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoreError::Message("auth lock poisoned".to_string()))?;
        state
            .pairing
            .insert(code.clone(), now_secs() + PAIRING_TTL_SECS);
        self.save(&state)?;
        Ok(code)
    }

    /// Redeem a valid, unexpired pairing code: mint + persist a device token.
    pub fn redeem(&self, code: &str, device_id: &str) -> Result<String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CoreError::Message("auth lock poisoned".to_string()))?;
        let now = now_secs();
        state.pairing.retain(|_, exp| *exp > now);
        match state.pairing.remove(code) {
            Some(_) => {
                let token = uuid::Uuid::new_v4().simple().to_string();
                state.tokens.insert(token.clone(), device_id.to_string());
                self.save(&state)?;
                Ok(token)
            }
            None => Err(CoreError::Message(
                "invalid or expired pairing code".to_string(),
            )),
        }
    }
}

/// Register the `pair_create` tool (makes a pairing code for a new device).
pub fn register(registry: &mut ToolRegistry, auth: Arc<AuthStore>) {
    registry.register(Box::new(PairCreate { auth }));
}

struct PairCreate {
    auth: Arc<AuthStore>,
}

#[async_trait]
impl Tool for PairCreate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "pair_create".to_string(),
            description: "Create a short-lived pairing code so a new device can enroll (it connects with the code and receives a token).".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let code = self.auth.create_pairing()?;
        Ok(json!({ "pairing_code": code, "expires_in_secs": PAIRING_TTL_SECS }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(required: bool, bootstrap: Option<String>) -> AuthStore {
        let path = std::env::temp_dir()
            .join(format!("fleety-auth-{}", uuid::Uuid::new_v4()))
            .join("auth.json");
        AuthStore::load(path, bootstrap, required)
    }

    #[test]
    fn bootstrap_token_validates() {
        let store = temp_store(true, Some("admin-tok".to_string()));
        assert_eq!(store.verify("admin-tok").as_deref(), Some("admin"));
        assert!(store.verify("nope").is_none());
    }

    #[test]
    fn pairing_mints_a_working_token() {
        let store = temp_store(true, None);
        let code = store.create_pairing().expect("code");
        let token = store.redeem(&code, "laptop").expect("redeem");
        assert_eq!(store.verify(&token).as_deref(), Some("laptop"));
        // a code is single-use
        assert!(store.redeem(&code, "laptop").is_err());
        // unknown code rejected
        assert!(store.redeem("zzzzzzzz", "x").is_err());
    }
}
