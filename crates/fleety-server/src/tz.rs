//! Per-user timezone rendering. Storage keeps timestamps as Unix epoch (UTC);
//! this module resolves the acting user's IANA timezone (profile → `FLEETY_TZ`
//! → the host device's local timezone → UTC) and renders an epoch for display /
//! "now". Reuses `chrono-tz` (the same library schedules use for cron zones) and
//! `iana-time-zone` to detect the host's own zone so times follow the device by
//! default instead of always UTC.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::TimeZone;
use chrono_tz::Tz;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

use crate::identity::ActingUser;
use crate::storage::Storage;

fn parse(s: Option<&str>) -> Option<Tz> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<Tz>().ok())
}

/// Whether `s` is a valid IANA timezone.
pub fn is_valid_tz(s: &str) -> bool {
    s.trim().parse::<Tz>().is_ok()
}

/// Register the `set_timezone` tool, scoped to the acting user. Lets the agent
/// record the user's IANA timezone so times are shown in it (the read side already
/// exists via `user_timezone` → `resolve_tz`). A guest can't set one.
pub fn register(tools: &mut ToolRegistry, storage: Arc<Storage>, acting: ActingUser) {
    tools.register(Box::new(SetTimezone { storage, acting }));
}

struct SetTimezone {
    storage: Arc<Storage>,
    acting: ActingUser,
}

#[async_trait]
impl Tool for SetTimezone {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_timezone".to_string(),
            description: "Set the current user's timezone (IANA name, e.g. 'Asia/Taipei' or \
                'Europe/Paris') so times and \"now\" are shown in it. Call this when the user \
                tells you their timezone or location."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "tz": { "type": "string", "description": "IANA timezone, e.g. Asia/Taipei" } },
                "required": ["tz"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let tz = args
            .get("tz")
            .and_then(Value::as_str)
            .map(str::trim)
            .ok_or_else(|| CoreError::Message("set_timezone requires 'tz'".to_string()))?;
        if !is_valid_tz(tz) {
            return Err(CoreError::Message(format!(
                "'{tz}' is not a valid IANA timezone (e.g. 'Asia/Taipei', 'America/New_York')"
            )));
        }
        let Some(user) = self.acting.user_id() else {
            return Err(CoreError::Message(
                "no identified user for this turn; cannot set a per-user timezone".to_string(),
            ));
        };
        self.storage.set_user_timezone(user, tz)?;
        Ok(json!({ "ok": true, "timezone": tz }))
    }
}

/// Resolve the timezone: the acting user's configured zone, else `FLEETY_TZ`,
/// else UTC. An invalid zone string falls through to the next source.
/// The host device's own IANA timezone (e.g. `Asia/Taipei`), or `None` if it
/// can't be detected or isn't a zone `chrono-tz` knows.
pub fn device_tz() -> Option<Tz> {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse::<Tz>().ok())
}

/// Resolve the timezone to render "now" in: the acting user's saved zone, else
/// `FLEETY_TZ`, else the **host device's** local zone, else UTC. So a stock
/// server shows times in the device's timezone rather than always UTC.
pub fn resolve_tz(user_tz: Option<&str>, env_tz: Option<&str>) -> Tz {
    parse(user_tz)
        .or_else(|| parse(env_tz))
        .or_else(device_tz)
        .unwrap_or(Tz::UTC)
}

/// Render a Unix epoch (seconds) as a human string in `tz`, e.g.
/// `2026-06-29 14:05:00 CST`.
pub fn format_for_user(ts_secs: u64, tz: Tz) -> String {
    match tz.timestamp_opt(ts_secs as i64, 0).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
        None => format!("{ts_secs} (epoch)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_user_then_env_then_device_then_utc() {
        // Valid user tz wins.
        assert_eq!(
            resolve_tz(Some("Asia/Taipei"), Some("Europe/Paris")),
            Tz::Asia__Taipei
        );
        // Invalid user → env.
        assert_eq!(
            resolve_tz(Some("Not/AZone"), Some("Europe/Paris")),
            Tz::Europe__Paris
        );
        // Invalid/absent user+env → the host device's zone, else UTC.
        let device_fallback = device_tz().unwrap_or(Tz::UTC);
        assert_eq!(resolve_tz(Some("bad"), Some("also-bad")), device_fallback);
        assert_eq!(resolve_tz(None, None), device_fallback);
        // Blank strings fall through to the same fallback.
        assert_eq!(resolve_tz(Some("  "), None), device_fallback);
    }

    #[tokio::test]
    async fn set_timezone_stores_validates_and_guards_guest() {
        let home = std::env::temp_dir().join(format!("fleety-tz-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let storage = Arc::new(Storage::new(home.clone()));

        // A real user can set a valid zone; it round-trips via user_timezone.
        let alice = ActingUser::User("alice".to_string());
        let tool = SetTimezone {
            storage: Arc::clone(&storage),
            acting: alice.clone(),
        };
        let ok = tool.call(json!({ "tz": "Asia/Taipei" })).await.unwrap();
        assert_eq!(ok["ok"], true);
        assert_eq!(
            storage.user_timezone(&alice).as_deref(),
            Some("Asia/Taipei")
        );
        // Now resolve_tz picks it up.
        assert_eq!(
            resolve_tz(storage.user_timezone(&alice).as_deref(), None),
            Tz::Asia__Taipei
        );
        // Invalid zone → error, nothing stored.
        assert!(tool.call(json!({ "tz": "Not/AZone" })).await.is_err());
        // Guest → error.
        let guest = SetTimezone {
            storage,
            acting: ActingUser::Guest,
        };
        assert!(guest.call(json!({ "tz": "Asia/Taipei" })).await.is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn format_renders_in_zone() {
        // 2021-01-01T00:00:00Z = epoch 1609459200.
        let utc = format_for_user(1_609_459_200, Tz::UTC);
        assert!(utc.starts_with("2021-01-01 00:00:00"), "got {utc}");
        // Taipei is UTC+8 → 08:00 same day.
        let tpe = format_for_user(1_609_459_200, Tz::Asia__Taipei);
        assert!(tpe.starts_with("2021-01-01 08:00:00"), "got {tpe}");
    }
}
