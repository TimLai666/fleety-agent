//! Self-managed schedule CRUD tools (STATUS.md #8). Persists schedules + fires
//! the `at:` / `every:` / `cron:` triggers from the scheduler loop. Cron
//! expressions are evaluated in an optional IANA timezone (`tz` field on the
//! schedule, default `UTC`); the `cron` crate handles the math and `chrono-tz`
//! supplies the zone database.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use async_trait::async_trait;
use chrono::TimeZone;
use chrono_tz::Tz;
use cron::Schedule as CronSchedule;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the schedule tools into `registry`, persisting under `dir`.
pub fn register(registry: &mut ToolRegistry, dir: &Path) {
    registry.register(Box::new(ScheduleCreate {
        dir: dir.to_path_buf(),
    }));
    registry.register(Box::new(ScheduleList {
        dir: dir.to_path_buf(),
    }));
    registry.register(Box::new(ScheduleDelete {
        dir: dir.to_path_buf(),
    }));
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Validate a schedule trigger: `at:<unix_secs>`, `every:<dur>` (e.g. `30s`,
/// `5m`, `2h`, `1d`, or bare seconds), or a 5-field cron expression (also
/// accepted with an explicit `cron:` prefix). Cron timezone is configured
/// separately via the schedule's `tz` field, not embedded in the trigger.
fn validate_trigger(spec: &str) -> Result<()> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("at:") {
        return parse_unix(rest).map(|_| ());
    }
    if let Some(rest) = spec.strip_prefix("every:") {
        return parse_duration_secs(rest).map(|_| ());
    }
    let cron_body = spec.strip_prefix("cron:").unwrap_or(spec).trim();
    if cron_body.split_whitespace().count() == 5 {
        // Round-trip through the parser so a syntactically broken cron expr
        // (e.g. `99 * * * *`) gets rejected at create time, not at fire time.
        parse_cron_expr(cron_body)?;
        return Ok(());
    }
    Err(CoreError::Message(format!(
        "invalid trigger '{spec}'; use 'at:<unix_secs>', 'every:<dur, e.g. 30s/5m/1h/1d>', or a 5-field cron expression"
    )))
}

/// `tz` defaults to UTC; anything else must be an IANA name parseable by
/// `chrono_tz` (e.g. `Asia/Taipei`, `America/New_York`).
fn validate_tz(tz: &str) -> Result<Tz> {
    Tz::from_str(tz).map_err(|_| {
        CoreError::Message(format!(
            "unknown timezone '{tz}'; use 'UTC' or an IANA name like 'Asia/Taipei'"
        ))
    })
}

/// Parse a 5-field cron expression into the 6-field shape the `cron` crate
/// expects (it always wants the seconds slot, so we prepend `0`).
fn parse_cron_expr(expr: &str) -> Result<CronSchedule> {
    let normalized = format!("0 {expr}");
    CronSchedule::from_str(&normalized)
        .map_err(|e| CoreError::Message(format!("invalid cron '{expr}': {e}")))
}

fn parse_unix(s: &str) -> Result<u64> {
    s.trim()
        .parse::<u64>()
        .map_err(|_| CoreError::Message(format!("invalid 'at:' time '{s}' (want unix seconds)")))
}

fn parse_duration_secs(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u64>() {
        return if n > 0 {
            Ok(n)
        } else {
            Err(CoreError::Message(
                "'every:' interval must be > 0".to_string(),
            ))
        };
    }
    let split = s.len().saturating_sub(1);
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| CoreError::Message(format!("invalid duration '{s}' (e.g. 30s/5m/1h/1d)")))?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => {
            return Err(CoreError::Message(format!(
                "invalid duration unit in '{s}' (use s/m/h/d)"
            )))
        }
    };
    if n == 0 {
        return Err(CoreError::Message("duration must be > 0".to_string()));
    }
    Ok(n * mult)
}

/// A schedule that is due to fire now.
pub(crate) struct DueSchedule {
    pub id: String,
    pub prompt: String,
    /// Non-read tools this unattended run is authorized to use (the mandate).
    pub allowed_tools: Vec<String>,
}

/// Whether a trigger should fire now given its last run. Handles `at:`,
/// `every:`, and 5-field cron (with `tz` defaulting to UTC).
pub(crate) fn trigger_due(spec: &str, tz: &str, last_run: Option<u64>, now: u64) -> bool {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("at:") {
        return matches!(parse_unix(rest), Ok(t) if last_run.is_none() && now >= t);
    }
    if let Some(rest) = spec.strip_prefix("every:") {
        return match parse_duration_secs(rest) {
            Ok(secs) => match last_run {
                None => true,
                Some(l) => now >= l.saturating_add(secs),
            },
            Err(_) => false,
        };
    }
    let cron_body = spec.strip_prefix("cron:").unwrap_or(spec).trim();
    if cron_body.split_whitespace().count() == 5 {
        return cron_due(cron_body, tz, last_run, now).unwrap_or(false);
    }
    false
}

/// True iff the cron expression has at least one firing in the open interval
/// `(last_run, now]` (or `[epoch, now]` when there's no last_run yet, capped
/// to "fire on the first matching minute that is <= now"). Any parse failure
/// short-circuits to `false` — the validator rejected it at create time, so
/// reaching this branch with a bad expression means the on-disk record was
/// corrupted; safer to never fire than to crash the scheduler tick.
fn cron_due(expr: &str, tz: &str, last_run: Option<u64>, now: u64) -> Option<bool> {
    let schedule = parse_cron_expr(expr).ok()?;
    let zone = Tz::from_str(tz).unwrap_or(chrono_tz::UTC);
    let now_dt = zone.timestamp_opt(now as i64, 0).single()?;
    let after = match last_run {
        Some(l) => zone.timestamp_opt(l as i64, 0).single()?,
        // No last_run: pretend the last firing was one minute before `now` so
        // any minute that matched in the immediate past still counts.
        None => zone
            .timestamp_opt((now as i64).saturating_sub(60), 0)
            .single()?,
    };
    let next = schedule.after(&after).next()?;
    Some(next <= now_dt)
}

/// Schedules that are due to fire at `now` (enabled, `at:`/`every:`).
pub(crate) fn due_schedules(dir: &Path, now: u64) -> Result<Vec<DueSchedule>> {
    let mut due = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(due),
        Err(e) => return Err(CoreError::Message(format!("cannot list schedules: {e}"))),
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if !value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            continue;
        }
        let trigger = value.get("trigger").and_then(Value::as_str).unwrap_or("");
        let tz = value.get("tz").and_then(Value::as_str).unwrap_or("UTC");
        let last_run = value.get("last_run").and_then(Value::as_u64);
        if trigger_due(trigger, tz, last_run, now) {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let prompt = value
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let allowed_tools = value
                .get("allowed_tools")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if !id.is_empty() {
                due.push(DueSchedule {
                    id,
                    prompt,
                    allowed_tools,
                });
            }
        }
    }
    Ok(due)
}

/// Upper bound (in chars) for an outcome summary persisted on a schedule.
const SUMMARY_MAX_CHARS: usize = 500;

/// Truncate `s` to at most [`SUMMARY_MAX_CHARS`] characters on a UTF-8 char
/// boundary (never splits a codepoint, never panics). Returns the leading
/// portion verbatim — nothing is appended.
pub(crate) fn truncate_summary(s: &str) -> String {
    s.chars().take(SUMMARY_MAX_CHARS).collect()
}

/// Read-modify-write a schedule's JSON record by `id`, applying `edit` to its
/// top-level object and persisting the result pretty-printed. Rejects ids with
/// path separators (same guard as the CRUD tools). The scheduler's book-keeping
/// fields (`last_run`, `last_outcome`, `last_notified`) are all written through
/// here so they share one id check and one write path.
fn update_schedule(
    dir: &Path,
    id: &str,
    edit: impl FnOnce(&mut serde_json::Map<String, Value>),
) -> Result<()> {
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(CoreError::Message(format!("invalid schedule id '{id}'")));
    }
    let path = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::Message(format!("cannot read schedule '{id}': {e}")))?;
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Message(format!("corrupt schedule '{id}': {e}")))?;
    if let Some(obj) = value.as_object_mut() {
        edit(obj);
    }
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| CoreError::Message(format!("serialize schedule: {e}")))?;
    std::fs::write(&path, pretty)
        .map_err(|e| CoreError::Message(format!("write schedule: {e}")))?;
    Ok(())
}

/// Record that a schedule fired at `now` (sets `last_run`).
pub(crate) fn mark_fired(dir: &Path, id: &str, now: u64) -> Result<()> {
    update_schedule(dir, id, |obj| {
        obj.insert("last_run".to_string(), json!(now));
    })
}

/// Additively record a run's outcome onto a schedule: sets `last_outcome`
/// (`{status, summary, ts}`) without disturbing `last_run` or any other field.
/// `status` is `"ok"` or `"error"`; `summary` should already be truncated
/// (see [`truncate_summary`]).
pub(crate) fn record_outcome(
    dir: &Path,
    id: &str,
    status: &str,
    summary: &str,
    ts: u64,
) -> Result<()> {
    update_schedule(dir, id, |obj| {
        obj.insert(
            "last_outcome".to_string(),
            json!({ "status": status, "summary": summary, "ts": ts }),
        );
    })
}

/// Advance a schedule's notification watermark to `ts` (sets `last_notified`),
/// so an already-delivered outcome is not delivered again.
pub(crate) fn mark_notified(dir: &Path, id: &str, ts: u64) -> Result<()> {
    update_schedule(dir, id, |obj| {
        obj.insert("last_notified".to_string(), json!(ts));
    })
}

/// A schedule outcome that has completed since the owner was last notified,
/// ready to deliver.
pub(crate) struct PendingNotification {
    pub id: String,
    pub status: String,
    pub summary: String,
    pub ts: u64,
}

/// Schedules whose latest `last_outcome.ts` is newer than their `last_notified`
/// watermark (i.e. finished since the owner was last notified), oldest first.
/// Best-effort: a missing dir, unreadable/corrupt files, schedules with no
/// outcome, and records with a blank id are all skipped rather than erroring.
pub(crate) fn pending_notifications(dir: &Path) -> Vec<PendingNotification> {
    let mut pending = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return pending;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(outcome) = value.get("last_outcome").and_then(Value::as_object) else {
            continue;
        };
        let ts = outcome.get("ts").and_then(Value::as_u64).unwrap_or(0);
        let last_notified = value
            .get("last_notified")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if ts <= last_notified {
            continue;
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let status = outcome
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let summary = outcome
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        pending.push(PendingNotification {
            id,
            status,
            summary,
            ts,
        });
    }
    pending.sort_by_key(|p| p.ts);
    pending
}

struct ScheduleCreate {
    dir: PathBuf,
}

#[async_trait]
impl Tool for ScheduleCreate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_create".to_string(),
            description: "Create a schedule. `trigger` is a cron expr / `at:<time>` / `every:<dur>`; `tz` (IANA name, default `UTC`) sets the timezone cron expressions are evaluated in; `mandate` describes the authorized scope; `allowed_tools` lists the non-read tools the unattended run may use (enforced at fire time).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger": { "type": "string" },
                    "tz": { "type": "string", "description": "IANA timezone for cron evaluation; default UTC" },
                    "prompt": { "type": "string" },
                    "mandate": { "type": "string", "description": "human-readable authorized scope, agreed now" },
                    "allowed_tools": { "type": "array", "items": { "type": "string" }, "description": "non-read tool names the unattended run may use; others are denied" }
                },
                "required": ["trigger", "prompt"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let trigger = require_str(&args, "trigger")?;
        validate_trigger(trigger)?;
        let tz = args.get("tz").and_then(Value::as_str).unwrap_or("UTC");
        validate_tz(tz)?;
        let prompt = require_str(&args, "prompt")?;
        let mandate = args.get("mandate").and_then(Value::as_str).unwrap_or("");
        let allowed_tools: Vec<String> = args
            .get("allowed_tools")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let id = uuid::Uuid::new_v4().to_string();
        let record = json!({
            "id": id,
            "trigger": trigger,
            "tz": tz,
            "prompt": prompt,
            "mandate": mandate,
            "allowed_tools": allowed_tools,
            "enabled": true,
        });
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| CoreError::Message(format!("cannot create schedules dir: {e}")))?;
        let pretty = serde_json::to_string_pretty(&record)
            .map_err(|e| CoreError::Message(format!("serialize schedule: {e}")))?;
        std::fs::write(self.dir.join(format!("{id}.json")), pretty)
            .map_err(|e| CoreError::Message(format!("write schedule: {e}")))?;
        Ok(json!({ "id": id, "created": true }))
    }
}

struct ScheduleList {
    dir: PathBuf,
}

#[async_trait]
impl Tool for ScheduleList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_list".to_string(),
            description: "List the agent's schedules.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let mut schedules = Vec::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Ok(text) = std::fs::read_to_string(entry.path()) {
                            if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
                                annotate_next_fire(&mut value, now);
                                // Where this schedule's runs are recorded — without
                                // this the results are undiscoverable (`fleety
                                // resume <conversation_id>` replays them).
                                if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                                    let conv = format!("schedule-{id}");
                                    if let Some(obj) = value.as_object_mut() {
                                        obj.insert("conversation_id".to_string(), json!(conv));
                                    }
                                }
                                schedules.push(value);
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CoreError::Message(format!("cannot list schedules: {e}"))),
        }
        Ok(json!({ "schedules": schedules }))
    }
}

/// Annotate one schedule with `next_fire_secs` (unix) so `schedule_list`
/// shows when each one is due. Best-effort: a fire time we can't compute is
/// omitted entirely rather than guessed.
fn annotate_next_fire(value: &mut Value, now: u64) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let trigger = obj
        .get("trigger")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let tz = obj
        .get("tz")
        .and_then(Value::as_str)
        .unwrap_or("UTC")
        .to_string();
    let last_run = obj.get("last_run").and_then(Value::as_u64);
    if let Some(next) = next_fire_secs(&trigger, &tz, last_run, now) {
        obj.insert("next_fire_secs".to_string(), json!(next));
    }
}

/// Predict the next firing time for a trigger, in unix seconds. None for
/// triggers we can't compute (corrupt cron, `at:` already fired, …).
fn next_fire_secs(spec: &str, tz: &str, last_run: Option<u64>, now: u64) -> Option<u64> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("at:") {
        let t = parse_unix(rest).ok()?;
        if last_run.is_some() {
            return None; // `at:` already fired.
        }
        return Some(t.max(now));
    }
    if let Some(rest) = spec.strip_prefix("every:") {
        let secs = parse_duration_secs(rest).ok()?;
        let next = match last_run {
            Some(l) => l.saturating_add(secs),
            None => now,
        };
        return Some(next.max(now));
    }
    let cron_body = spec.strip_prefix("cron:").unwrap_or(spec).trim();
    if cron_body.split_whitespace().count() == 5 {
        let schedule = parse_cron_expr(cron_body).ok()?;
        let zone = Tz::from_str(tz).unwrap_or(chrono_tz::UTC);
        let now_dt = zone.timestamp_opt(now as i64, 0).single()?;
        let next = schedule.after(&now_dt).next()?;
        return Some(next.timestamp().max(0) as u64);
    }
    None
}

struct ScheduleDelete {
    dir: PathBuf,
}

#[async_trait]
impl Tool for ScheduleDelete {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_delete".to_string(),
            description: "Delete a schedule by id.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "id")?;
        if id.contains('/') || id.contains('\\') || id.contains("..") {
            return Err(CoreError::Message(format!("invalid schedule id '{id}'")));
        }
        let path = self.dir.join(format!("{id}.json"));
        if !path.exists() {
            return Err(CoreError::Message(format!("no such schedule '{id}'")));
        }
        std::fs::remove_file(&path)
            .map_err(|e| CoreError::Message(format!("cannot delete schedule: {e}")))?;
        Ok(json!({ "id": id, "deleted": true }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-sched-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    #[tokio::test]
    async fn create_list_delete() {
        let dir = temp_dir();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &dir);

        let created = registry
            .call(
                "schedule_create",
                json!({ "trigger": "every:1h", "prompt": "check", "mandate": "read logs" }),
            )
            .await
            .expect("create");
        let id = created["id"].as_str().expect("id").to_string();

        let listed = registry
            .call("schedule_list", json!({}))
            .await
            .expect("list");
        assert_eq!(listed["schedules"].as_array().map(Vec::len).unwrap_or(0), 1);

        registry
            .call("schedule_delete", json!({ "id": id }))
            .await
            .expect("delete");
        let after = registry
            .call("schedule_list", json!({}))
            .await
            .expect("list2");
        assert_eq!(after["schedules"].as_array().map(Vec::len).unwrap_or(0), 0);

        let missing = registry
            .call("schedule_delete", json!({ "id": "nope" }))
            .await;
        assert!(missing.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trigger_validation() {
        assert!(validate_trigger("at:1700000000").is_ok());
        assert!(validate_trigger("every:30s").is_ok());
        assert!(validate_trigger("every:5m").is_ok());
        assert!(validate_trigger("every:1h").is_ok());
        assert!(validate_trigger("every:1d").is_ok());
        assert!(validate_trigger("every:90").is_ok());
        assert!(validate_trigger("0 9 * * 1").is_ok()); // cron, bare
        assert!(validate_trigger("cron:0 9 * * 1").is_ok()); // cron, prefixed
        assert!(validate_trigger("at:notanumber").is_err());
        assert!(validate_trigger("every:0").is_err());
        assert!(validate_trigger("every:5x").is_err());
        assert!(validate_trigger("99 9 * * 1").is_err()); // syntactically invalid cron
        assert!(validate_trigger("garbage").is_err());
    }

    #[test]
    fn next_fire_predicts_at_every_and_cron() {
        // `at:` future fires once, past fires now (no last_run).
        assert_eq!(next_fire_secs("at:2000", "UTC", None, 1000), Some(2000));
        assert_eq!(next_fire_secs("at:500", "UTC", None, 1000), Some(1000));
        assert_eq!(next_fire_secs("at:2000", "UTC", Some(1500), 1500), None);

        // `every:` is last_run + dur, floored at now.
        assert_eq!(
            next_fire_secs("every:60", "UTC", Some(1000), 1500),
            Some(1500)
        );
        assert_eq!(
            next_fire_secs("every:60", "UTC", Some(1000), 1010),
            Some(1060)
        );

        // Cron: 9am-Mon UTC next firing after a Sunday should be Monday 9am UTC.
        // 2026-06-21 12:00 UTC (Sun) -> 2026-06-22 09:00 UTC (Mon) = 1_750_582_800.
        let sunday_noon = 1_750_507_200; // 2026-06-21 12:00 UTC
        let monday_9am = 1_750_582_800; // 2026-06-22 09:00 UTC
        assert_eq!(
            next_fire_secs("0 9 * * 1", "UTC", None, sunday_noon),
            Some(monday_9am)
        );
    }

    #[test]
    fn tz_validation() {
        assert!(validate_tz("UTC").is_ok());
        assert!(validate_tz("Asia/Taipei").is_ok());
        assert!(validate_tz("America/New_York").is_ok());
        assert!(validate_tz("Atlantis/Lost_City").is_err());
    }

    #[tokio::test]
    async fn create_rejects_bad_trigger() {
        let dir = temp_dir();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &dir);
        let bad = registry
            .call(
                "schedule_create",
                json!({ "trigger": "garbage", "prompt": "x" }),
            )
            .await;
        assert!(bad.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trigger_due_logic() {
        assert!(!trigger_due("at:100", "UTC", None, 50));
        assert!(trigger_due("at:100", "UTC", None, 100));
        assert!(!trigger_due("at:100", "UTC", Some(100), 200));
        assert!(trigger_due("every:60", "UTC", None, 0));
        assert!(!trigger_due("every:60", "UTC", Some(100), 130));
        assert!(trigger_due("every:60", "UTC", Some(100), 160));
        assert!(trigger_due("every:1m", "UTC", Some(100), 160));
    }

    #[test]
    fn cron_fires_in_specified_timezone() {
        // Sun 2026-06-21 09:00 in Asia/Taipei is 2026-06-21 01:00 UTC =
        // unix 1750467600. At that unix second a 9am-Mon-Fri cron in Taipei
        // should fire if we just rolled into Monday 9am Taipei.
        // Pick a Monday 9am Taipei: 2026-06-22 09:00 +08 = 2026-06-22 01:00 UTC.
        // unix(2026-06-22 01:00 UTC) = 1_750_554_000.
        let now = 1_750_554_000;
        assert!(trigger_due("0 9 * * 1", "Asia/Taipei", None, now));
        // At the same wall-clock interpreted as UTC, 9am Monday wouldn't have
        // fired yet (UTC time at unix 1_750_554_000 is 01:00).
        assert!(!trigger_due("0 9 * * 1", "UTC", None, now));
    }

    #[test]
    fn cron_does_not_fire_twice_within_a_minute() {
        let trigger = "*/5 * * * *"; // every 5 minutes UTC
        let now = 1_750_554_000; // 01:00 UTC (a 5-minute boundary)
        assert!(trigger_due(trigger, "UTC", None, now));
        // Just fired this minute — shouldn't re-fire 30 s later.
        assert!(!trigger_due(trigger, "UTC", Some(now), now + 30));
    }

    #[test]
    fn truncate_summary_is_char_boundary_safe() {
        // ASCII under the cap is returned verbatim.
        assert_eq!(truncate_summary("hello"), "hello");
        // Over the cap: exactly SUMMARY_MAX_CHARS chars, no panic.
        let long = "a".repeat(SUMMARY_MAX_CHARS + 100);
        assert_eq!(truncate_summary(&long).chars().count(), SUMMARY_MAX_CHARS);
        // Multi-byte chars are never split mid-codepoint.
        let wide = "你".repeat(SUMMARY_MAX_CHARS + 50);
        let cut = truncate_summary(&wide);
        assert_eq!(cut.chars().count(), SUMMARY_MAX_CHARS);
        assert!(cut.chars().all(|c| c == '你')); // valid UTF-8, whole chars
    }

    #[test]
    fn mark_fired_records_last_outcome() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("s1.json"),
            r#"{"id":"s1","trigger":"at:1","prompt":"go","enabled":true}"#,
        )
        .expect("write schedule");

        mark_fired(&dir, "s1", 2000).expect("mark fired");
        record_outcome(&dir, "s1", "ok", "did the thing", 2000).expect("record outcome");

        let text = std::fs::read_to_string(dir.join("s1.json")).expect("read back");
        let value: Value = serde_json::from_str(&text).expect("parse");
        // last_run set by mark_fired is preserved alongside the additive outcome.
        assert_eq!(value.get("last_run").and_then(Value::as_u64), Some(2000));
        let outcome = value.get("last_outcome").expect("last_outcome present");
        assert_eq!(outcome.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(
            outcome.get("summary").and_then(Value::as_str),
            Some("did the thing")
        );
        assert_eq!(outcome.get("ts").and_then(Value::as_u64), Some(2000));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn schedule_list_surfaces_last_outcome() {
        let dir = temp_dir();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &dir);
        let created = registry
            .call(
                "schedule_create",
                json!({ "trigger": "at:1", "prompt": "check" }),
            )
            .await
            .expect("create");
        let id = created["id"].as_str().expect("id").to_string();

        mark_fired(&dir, &id, 2000).expect("mark fired");
        record_outcome(&dir, &id, "error", "boom", 2000).expect("record outcome");

        let listed = registry
            .call("schedule_list", json!({}))
            .await
            .expect("list");
        let entry = listed["schedules"]
            .as_array()
            .and_then(|a| a.first())
            .expect("one schedule");
        assert_eq!(entry.get("last_run").and_then(Value::as_u64), Some(2000));
        assert_eq!(
            entry
                .get("last_outcome")
                .and_then(|o| o.get("status"))
                .and_then(Value::as_str),
            Some("error")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_and_mark_notified_roundtrip() {
        let dir = temp_dir();
        // Two schedules with outcomes, one already notified.
        std::fs::write(dir.join("a.json"), r#"{"id":"a"}"#).expect("write a");
        std::fs::write(dir.join("b.json"), r#"{"id":"b"}"#).expect("write b");
        record_outcome(&dir, "a", "ok", "a ran", 100).expect("outcome a");
        record_outcome(&dir, "b", "error", "b failed", 200).expect("outcome b");
        // b already notified at its outcome ts -> not pending; a is pending.
        mark_notified(&dir, "b", 200).expect("mark b notified");

        let pending = pending_notifications(&dir);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "a");
        assert_eq!(pending[0].status, "ok");
        assert_eq!(pending[0].ts, 100);

        // After notifying a, nothing is pending.
        mark_notified(&dir, "a", 100).expect("mark a notified");
        assert!(pending_notifications(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_notifications_orders_oldest_first() {
        let dir = temp_dir();
        std::fs::write(dir.join("x.json"), r#"{"id":"x"}"#).expect("write x");
        std::fs::write(dir.join("y.json"), r#"{"id":"y"}"#).expect("write y");
        record_outcome(&dir, "x", "ok", "later", 300).expect("outcome x");
        record_outcome(&dir, "y", "ok", "earlier", 100).expect("outcome y");
        let pending = pending_notifications(&dir);
        let ids: Vec<&str> = pending.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["y", "x"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn due_and_mark_fired_roundtrip() {
        let dir = temp_dir();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &dir);
        let created = registry
            .call(
                "schedule_create",
                json!({ "trigger": "at:1000", "prompt": "go" }),
            )
            .await
            .expect("create");
        let id = created["id"].as_str().expect("id").to_string();

        let due = due_schedules(&dir, 2000).expect("due");
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].prompt, "go");

        mark_fired(&dir, &id, 2000).expect("mark");
        assert!(due_schedules(&dir, 3000).expect("due2").is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
