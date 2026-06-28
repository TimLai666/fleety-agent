//! Per-user timezone rendering. Storage keeps timestamps as Unix epoch (UTC);
//! this module resolves the acting user's IANA timezone (profile → `FLEETY_TZ`
//! → UTC) and renders an epoch for display / "now". Reuses `chrono-tz` (the same
//! library schedules use for cron zones).

use chrono::TimeZone;
use chrono_tz::Tz;

fn parse(s: Option<&str>) -> Option<Tz> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<Tz>().ok())
}

/// Resolve the timezone: the acting user's configured zone, else `FLEETY_TZ`,
/// else UTC. An invalid zone string falls through to the next source.
pub fn resolve_tz(user_tz: Option<&str>, env_tz: Option<&str>) -> Tz {
    parse(user_tz).or_else(|| parse(env_tz)).unwrap_or(Tz::UTC)
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
    fn precedence_user_then_env_then_utc() {
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
        // Invalid/absent env → UTC.
        assert_eq!(resolve_tz(Some("bad"), Some("also-bad")), Tz::UTC);
        assert_eq!(resolve_tz(None, None), Tz::UTC);
        // Blank strings fall through.
        assert_eq!(resolve_tz(Some("  "), None), Tz::UTC);
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
