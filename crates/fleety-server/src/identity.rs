//! Identity layer: who a turn is for.
//!
//! The per-device token still authenticates the device/transport; on top of it
//! each turn resolves an [`ActingUser`] — the device's owner, an explicitly
//! asserted (and authorized) user, or [`ActingUser::Guest`] when no one can be
//! identified. Resolution is pure and total (always a `User` or `Guest`, never
//! an undefined state). This change only *identifies*; scoping/authorization of
//! data is `privacy-isolation`.

use agent_core::{CoreError, Result};

/// The principal a turn is acting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActingUser {
    /// An identified user (a validated user id).
    User(String),
    /// Identified-as-unknown: no user could be resolved.
    Guest,
}

impl ActingUser {
    /// The user id, or `None` for [`ActingUser::Guest`].
    pub fn user_id(&self) -> Option<&str> {
        match self {
            ActingUser::User(id) => Some(id),
            ActingUser::Guest => None,
        }
    }
}

/// Validate a `user_id` slug — same rules as device/conversation ids (no path
/// separators, `:`, `..`, NUL, or empty).
pub fn validate_user_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains('\0')
        || id.contains(':')
    {
        return Err(CoreError::Message(format!(
            "invalid user_id '{id}': must not be empty or contain path separators, ':' or '..'"
        )));
    }
    Ok(())
}

/// Whether `id` is a usable (non-blank, valid) asserted user.
fn valid_asserted(id: Option<&str>) -> Option<&str> {
    let id = id?.trim();
    if id.is_empty() || validate_user_id(id).is_err() {
        return None;
    }
    Some(id)
}

/// Resolve the acting user for a turn (pure). Precedence:
/// 1. A valid asserted user that the device authorizes (it is the owner or
///    listed in the device's `users`) → that user.
/// 2. A valid asserted user the device does NOT authorize → `Guest` (fail
///    closed — a foreign identity is never silently accepted as the owner).
/// 3. No usable assertion, device has an owner → the owner.
/// 4. Otherwise → `Guest`.
pub fn resolve_acting_user(
    device_owner: Option<&str>,
    device_users: &[String],
    asserted: Option<&str>,
) -> ActingUser {
    if let Some(asserted) = valid_asserted(asserted) {
        let authorized =
            device_owner == Some(asserted) || device_users.iter().any(|u| u == asserted);
        return if authorized {
            ActingUser::User(asserted.to_string())
        } else {
            ActingUser::Guest
        };
    }
    match device_owner {
        Some(owner) if !owner.is_empty() => ActingUser::User(owner.to_string()),
        _ => ActingUser::Guest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolution_example_table() {
        // | owner | asserted | shares to | resolved |
        // alice   | (none)   | —          | alice
        assert_eq!(
            resolve_acting_user(Some("alice"), &[], None),
            ActingUser::User("alice".to_string())
        );
        // (none)  | bob      | [bob]      | bob
        assert_eq!(
            resolve_acting_user(None, &users(&["bob"]), Some("bob")),
            ActingUser::User("bob".to_string())
        );
        // (none)  | bob      | (not listed) | Guest (fail closed)
        assert_eq!(
            resolve_acting_user(None, &[], Some("bob")),
            ActingUser::Guest
        );
        // (none)  | (none)   | —          | Guest
        assert_eq!(resolve_acting_user(None, &[], None), ActingUser::Guest);
    }

    #[test]
    fn owner_can_assert_self() {
        assert_eq!(
            resolve_acting_user(Some("alice"), &[], Some("alice")),
            ActingUser::User("alice".to_string())
        );
    }

    #[test]
    fn foreign_assertion_on_owned_device_fails_closed() {
        // bob asserts on alice's device, not authorized → Guest, not alice.
        assert_eq!(
            resolve_acting_user(Some("alice"), &[], Some("bob")),
            ActingUser::Guest
        );
    }

    #[test]
    fn blank_or_invalid_assertion_falls_back_to_owner() {
        assert_eq!(
            resolve_acting_user(Some("alice"), &[], Some("   ")),
            ActingUser::User("alice".to_string())
        );
        assert_eq!(
            resolve_acting_user(Some("alice"), &[], Some("a/b")),
            ActingUser::User("alice".to_string())
        );
    }

    #[test]
    fn user_id_validation() {
        assert!(validate_user_id("alice").is_ok());
        assert!(validate_user_id("").is_err());
        assert!(validate_user_id("a/b").is_err());
        assert!(validate_user_id("a:b").is_err());
        assert!(validate_user_id("..").is_err());
    }

    #[test]
    fn acting_user_accessors() {
        assert_eq!(ActingUser::User("x".into()).user_id(), Some("x"));
        assert_eq!(ActingUser::Guest.user_id(), None);
    }

    #[test]
    fn acting_user_equality_scopes_owner() {
        // The owner-scoped schedule delivery compares two resolved acting users:
        // the same owner resolved twice must be equal, and a Guest must never
        // equal a named user (so Guest connections are excluded).
        assert_eq!(
            ActingUser::User("alice".to_string()),
            ActingUser::User("alice".to_string())
        );
        assert_ne!(
            ActingUser::User("alice".to_string()),
            ActingUser::User("bob".to_string())
        );
        assert_ne!(ActingUser::Guest, ActingUser::User("alice".to_string()));
        assert_eq!(ActingUser::Guest, ActingUser::Guest);
    }
}
