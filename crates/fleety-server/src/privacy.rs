//! Privacy guard: the acting user is a hard boundary.
//!
//! Every cross-user data access is mediated by [`can_access`]: the acting user
//! may reach their own data, or data an explicit [`Grant`] covers; everything
//! else is denied. Guest reaches no real user's data. Decisions fail closed —
//! any ambiguity or error denies. A denial reveals nothing (the caller maps it
//! to a uniform "not available" response, see conn).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::identity::ActingUser;
use crate::storage::Storage;

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

    /// Revoke grants `owner` made to `grantee`, returning how many were removed.
    /// `scope` is an exact match (so `Some("trip")` never touches a `*` grant);
    /// `None` removes every grant `owner` gave `grantee`. An owner whose grant
    /// list ends up empty is dropped from the map (no dangling owner key).
    /// Removing a grant that isn't there simply removes nothing (returns 0).
    pub fn revoke(&mut self, owner: &str, grantee: &str, scope: Option<&str>) -> usize {
        let Some(list) = self.by_owner.get_mut(owner) else {
            return 0;
        };
        let before = list.len();
        match scope {
            Some(scope) => list.retain(|g| !(g.grantee == grantee && g.scope == scope)),
            None => list.retain(|g| g.grantee != grantee),
        }
        let removed = before - list.len();
        if list.is_empty() {
            self.by_owner.remove(owner);
        }
        removed
    }

    /// The grants `owner` currently holds (empty if `owner` has none). Lets the
    /// owner review what they've shared before revoking.
    pub fn grants_for(&self, owner: &str) -> Vec<Grant> {
        self.by_owner.get(owner).cloned().unwrap_or_default()
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

/// Register the cross-user grant tools, all scoped to the acting user (the data
/// owner): `grant_access` shares the user's own data, `revoke_access` takes a
/// grant back, and `list_access` shows what they've shared. A guest can't grant
/// or revoke and lists nothing.
pub fn register_grant(tools: &mut ToolRegistry, storage: Arc<Storage>, acting: ActingUser) {
    tools.register(Box::new(GrantAccess {
        storage: Arc::clone(&storage),
        acting: acting.clone(),
    }));
    tools.register(Box::new(RevokeAccess {
        storage: Arc::clone(&storage),
        acting: acting.clone(),
    }));
    tools.register(Box::new(ListAccess { storage, acting }));
}

struct GrantAccess {
    storage: Arc<Storage>,
    acting: ActingUser,
}

#[async_trait]
impl Tool for GrantAccess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grant_access".to_string(),
            description: "Let another user access YOUR data. Pass the grantee's user id and an \
                optional scope ('*' = everything, the default; or a specific scope like \
                'conversation'). Only grants access to your own data. Use revoke_access to take \
                a grant back, and list_access to review what you've shared."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "grantee": { "type": "string", "description": "User id to grant access to." },
                    "scope": { "type": "string", "description": "Scope, default '*' (all your data)." }
                },
                "required": ["grantee"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let Some(owner) = self.acting.user_id() else {
            return Err(CoreError::Message(
                "no identified user for this turn; only a real user can grant access to their data"
                    .to_string(),
            ));
        };
        let grantee = args
            .get("grantee")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Message("grant_access requires 'grantee'".to_string()))?;
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("*");
        if grantee == owner {
            return Err(CoreError::Message(
                "you already have access to your own data".to_string(),
            ));
        }
        self.storage.add_grant(owner, grantee, scope)?;
        Ok(json!({ "ok": true, "owner": owner, "grantee": grantee, "scope": scope }))
    }
}

struct RevokeAccess {
    storage: Arc<Storage>,
    acting: ActingUser,
}

#[async_trait]
impl Tool for RevokeAccess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "revoke_access".to_string(),
            description: "Take back a grant you made with grant_access. Pass the grantee's user \
                id and an optional scope; omit the scope to revoke EVERY grant you gave that \
                user. Scope is matched exactly, so revoking 'trip' leaves a '*' (everything) \
                grant in place — revoke '*' to remove that. Only affects grants on your own \
                data, takes effect immediately, and returns how many grants were removed \
                (0 if there was nothing to revoke). Use list_access to see what you've shared."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "grantee": { "type": "string", "description": "User id whose access to revoke." },
                    "scope": { "type": "string", "description": "Exact scope to revoke; omit to revoke all grants to this grantee." }
                },
                "required": ["grantee"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let Some(owner) = self.acting.user_id() else {
            return Err(CoreError::Message(
                "no identified user for this turn; only a real user can revoke access to their data"
                    .to_string(),
            ));
        };
        let grantee = args
            .get("grantee")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::Message("revoke_access requires 'grantee'".to_string()))?;
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let removed = self.storage.remove_grant(owner, grantee, scope)?;
        Ok(json!({
            "ok": true,
            "owner": owner,
            "grantee": grantee,
            "scope": scope,
            "removed": removed,
        }))
    }
}

struct ListAccess {
    storage: Arc<Storage>,
    acting: ActingUser,
}

#[async_trait]
impl Tool for ListAccess {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_access".to_string(),
            description: "List the grants you've given other users over YOUR data — each with \
                its grantee and scope — so you can see what to take back with revoke_access. \
                A guest sees an empty list."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let Some(owner) = self.acting.user_id() else {
            return Ok(json!({ "grants": [] }));
        };
        let grants: Vec<Value> = self
            .storage
            .grants()
            .grants_for(owner)
            .into_iter()
            .map(|g| json!({ "grantee": g.grantee, "scope": g.scope }))
            .collect();
        Ok(json!({ "grants": grants }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> ActingUser {
        ActingUser::User(id.to_string())
    }

    #[tokio::test]
    async fn grant_access_tool_records_and_guards_guest() {
        let home = std::env::temp_dir().join(format!("fleety-grant-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        let tool = GrantAccess {
            storage: Arc::clone(&storage),
            acting: user("alice"),
        };
        // Alice grants bob scope "trip".
        let r = tool
            .call(json!({ "grantee": "bob", "scope": "trip" }))
            .await
            .unwrap();
        assert_eq!(r["ok"], true);
        // The grant is persisted: bob can now access alice's "trip" scope.
        let grants = storage.grants();
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &grants),
            Decision::Allow
        );
        assert_eq!(
            can_access(&user("bob"), "alice", "other", &grants),
            Decision::Deny
        );
        // Guest can't grant.
        let guest = GrantAccess {
            storage,
            acting: ActingUser::Guest,
        };
        assert!(guest.call(json!({ "grantee": "bob" })).await.is_err());
        let _ = std::fs::remove_dir_all(&home);
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

    #[tokio::test]
    async fn revoke_access_tool_removes_and_guards_guest() {
        let home =
            std::env::temp_dir().join(format!("fleety-revoke-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        // Alice has granted bob two scopes.
        storage.add_grant("alice", "bob", "trip").unwrap();
        storage.add_grant("alice", "bob", "finances").unwrap();

        // Alice revokes bob entirely (no scope) → both grants gone, count == 2.
        let tool = RevokeAccess {
            storage: Arc::clone(&storage),
            acting: user("alice"),
        };
        let r = tool.call(json!({ "grantee": "bob" })).await.unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["removed"], 2);
        let grants = storage.grants();
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &grants),
            Decision::Deny
        );
        assert_eq!(
            can_access(&user("bob"), "alice", "finances", &grants),
            Decision::Deny
        );
        // Revoking something never granted succeeds, reporting zero removed.
        let r0 = tool.call(json!({ "grantee": "ghost" })).await.unwrap();
        assert_eq!(r0["removed"], 0);

        // Guest can't revoke.
        let guest = RevokeAccess {
            storage,
            acting: ActingUser::Guest,
        };
        assert!(guest.call(json!({ "grantee": "bob" })).await.is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn list_access_tool_lists_and_guards_guest() {
        let home = std::env::temp_dir().join(format!("fleety-list-tool-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        storage.add_grant("alice", "bob", "trip").unwrap();

        let list = ListAccess {
            storage: Arc::clone(&storage),
            acting: user("alice"),
        };
        // The grant Alice made shows up with its grantee and scope.
        let r = list.call(json!({})).await.unwrap();
        let grants = r["grants"].as_array().unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0]["grantee"], "bob");
        assert_eq!(grants[0]["scope"], "trip");

        // After revoking, the list is empty.
        storage.remove_grant("alice", "bob", None).unwrap();
        let r2 = list.call(json!({})).await.unwrap();
        assert!(r2["grants"].as_array().unwrap().is_empty());

        // Guest sees an empty list (never another owner's grants).
        let guest = ListAccess {
            storage,
            acting: ActingUser::Guest,
        };
        let rg = guest.call(json!({})).await.unwrap();
        assert!(rg["grants"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn register_grant_registers_all_three_tools() {
        let home = std::env::temp_dir().join(format!("fleety-reg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        let mut tools = ToolRegistry::new();
        register_grant(&mut tools, storage, user("alice"));
        assert!(tools.contains("grant_access"));
        assert!(tools.contains("revoke_access"));
        assert!(tools.contains("list_access"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn grant_access_description_points_to_revoke_and_list() {
        let home = std::env::temp_dir().join(format!("fleety-desc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));
        let tool = GrantAccess {
            storage,
            acting: user("alice"),
        };
        let desc = tool.spec().description;
        assert!(!desc.contains("revoke is not yet supported"));
        assert!(desc.contains("revoke_access"));
        assert!(desc.contains("list_access"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn revoke_removes_matching_grant() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        g.grant("alice", "bob", "finances");
        // Exact-scope revoke removes only that grant; access is denied at once.
        assert_eq!(g.revoke("alice", "bob", Some("trip")), 1);
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &g),
            Decision::Deny
        );
        // The other scope survives.
        assert_eq!(
            can_access(&user("bob"), "alice", "finances", &g),
            Decision::Allow
        );
        // Removing the last grant drops the now-empty owner key.
        assert_eq!(g.revoke("alice", "bob", Some("finances")), 1);
        assert!(!g.by_owner.contains_key("alice"));
    }

    #[test]
    fn revoke_without_scope_removes_all_for_grantee() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        g.grant("alice", "bob", "finances");
        g.grant("alice", "carol", "trip");
        // No scope → every grant to bob goes; carol is untouched.
        assert_eq!(g.revoke("alice", "bob", None), 2);
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &g),
            Decision::Deny
        );
        assert_eq!(
            can_access(&user("bob"), "alice", "finances", &g),
            Decision::Deny
        );
        assert_eq!(
            can_access(&user("carol"), "alice", "trip", &g),
            Decision::Allow
        );
    }

    #[test]
    fn revoke_nonexistent_returns_zero() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        // A never-granted grantee, and a non-matching scope, both remove nothing.
        assert_eq!(g.revoke("alice", "dave", None), 0);
        assert_eq!(g.revoke("alice", "bob", Some("finances")), 0);
        // An unknown owner is also zero (and never panics).
        assert_eq!(g.revoke("nobody", "bob", None), 0);
        // The original grant is intact.
        assert_eq!(
            can_access(&user("bob"), "alice", "trip", &g),
            Decision::Allow
        );
    }

    #[test]
    fn grants_for_lists_only_owner_grants() {
        let mut g = Grants::default();
        g.grant("alice", "bob", "trip");
        g.grant("alice", "carol", "*");
        g.grant("dave", "erin", "notes");
        let mut alice = g.grants_for("alice");
        alice.sort_by(|a, b| a.grantee.cmp(&b.grantee));
        assert_eq!(
            alice,
            vec![
                Grant {
                    grantee: "bob".to_string(),
                    scope: "trip".to_string()
                },
                Grant {
                    grantee: "carol".to_string(),
                    scope: "*".to_string()
                },
            ]
        );
        // Only alice's grants appear — dave's grantee never leaks in.
        assert!(g.grants_for("alice").iter().all(|gr| gr.grantee != "erin"));
        // Unknown owner → empty.
        assert!(g.grants_for("nobody").is_empty());
    }
}
