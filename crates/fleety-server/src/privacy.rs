//! Privacy guard: the acting user is a hard boundary.
//!
//! Every cross-user data access is mediated by [`can_access`]: the acting user
//! may reach their own data, or data an explicit [`Grant`] covers; everything
//! else is denied. Guest reaches no real user's data. Decisions fail closed —
//! any ambiguity or error denies. A denial reveals nothing (the caller maps it
//! to a uniform "not available" response, see conn).

use std::collections::HashMap;
use std::path::Path;

use agent_core::{CoreError, Result};
use serde::{Deserialize, Serialize};

use crate::identity::ActingUser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    pub fn is_allow(self) -> bool {
        self == Decision::Allow
    }
}

/// One explicit grant: `owner` lets `grantee` access a `scope` of their data.
/// `scope == "*"` means all of the owner's data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grant {
    pub grantee: String,
    pub scope: String,
}

/// Explicit cross-user grants, keyed by the data owner.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Grants {
    /// owner user id -> grants that owner has made.
    by_owner: HashMap<String, Vec<Grant>>,
}

impl Grants {
    /// Record a grant (idempotent on an identical grant). Grant-management API
    /// consumed by the cross-user sharing surface (a later change / config).
    #[allow(dead_code)]
    pub fn grant(&mut self, owner: &str, grantee: &str, scope: &str) {
        let list = self.by_owner.entry(owner.to_string()).or_default();
        let g = Grant {
            grantee: grantee.to_string(),
            scope: scope.to_string(),
        };
        if !list.contains(&g) {
            list.push(g);
        }
    }

    /// Whether `owner` has granted `grantee` access covering `scope`
    /// (an exact-scope or wildcard `*` grant).
    fn allows(&self, owner: &str, grantee: &str, scope: &str) -> bool {
        self.by_owner
            .get(owner)
            .map(|list| {
                list.iter()
                    .any(|g| g.grantee == grantee && (g.scope == "*" || g.scope == scope))
            })
            .unwrap_or(false)
    }

    /// Load grants from `path`; a missing file is empty, a corrupt file fails
    /// closed to empty (so a damaged grants file never widens access).
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Grants::default(),
        }
    }

    /// Persist grants to `path`.
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create grants dir: {e}")))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Message(format!("serialize grants: {e}")))?;
        std::fs::write(path, text).map_err(|e| CoreError::Message(format!("write grants: {e}")))?;
        Ok(())
    }
}

/// The guard: may `acting` access `resource_owner`'s data in `scope`?
/// - the owner themselves → Allow
/// - an explicit grant covering `scope` → Allow
/// - everything else (different user, Guest, unknown) → Deny (fail closed)
pub fn can_access(
    acting: &ActingUser,
    resource_owner: &str,
    scope: &str,
    grants: &Grants,
) -> Decision {
    match acting.user_id() {
        Some(id) if id == resource_owner => Decision::Allow,
        Some(id) if grants.allows(resource_owner, id, scope) => Decision::Allow,
        _ => Decision::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> ActingUser {
        ActingUser::User(id.to_string())
    }

    #[test]
    fn owner_is_allowed() {
        let g = Grants::default();
        assert_eq!(
            can_access(&user("alice"), "alice", "notes", &g),
            Decision::Allow
        );
    }

    #[test]
    fn other_user_without_grant_is_denied() {
        let g = Grants::default();
        assert_eq!(
            can_access(&user("bob"), "alice", "notes", &g),
            Decision::Deny
        );
    }

    #[test]
    fn grant_within_scope_is_allowed() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &g),
            Decision::Allow
        );
    }

    #[test]
    fn grant_outside_scope_is_denied() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        assert_eq!(
            can_access(&user("bob"), "alice", "finances", &g),
            Decision::Deny
        );
    }

    #[test]
    fn wildcard_grant_covers_any_scope() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "*");
        assert_eq!(
            can_access(&user("bob"), "alice", "anything", &g),
            Decision::Allow
        );
    }

    #[test]
    fn guest_is_denied_all_private() {
        let g = Grants::default();
        assert_eq!(
            can_access(&ActingUser::Guest, "alice", "notes", &g),
            Decision::Deny
        );
    }

    #[test]
    fn corrupt_grants_file_fails_closed_to_empty() {
        let dir = std::env::temp_dir().join(format!("fleety-grants-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("grants.json");
        std::fs::write(&p, "{ not json").unwrap();
        let g = Grants::load(&p);
        // Empty → a different user is denied (no accidental widening).
        assert_eq!(can_access(&user("bob"), "alice", "x", &g), Decision::Deny);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grants_roundtrip() {
        let dir = std::env::temp_dir().join(format!("fleety-grants-rt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("grants.json");
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        g.save(&p).unwrap();
        let loaded = Grants::load(&p);
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &loaded),
            Decision::Allow
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
