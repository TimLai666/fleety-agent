//! Self-managed schedule CRUD tools (STATUS.md #8). v0: create/list/delete
//! persisted schedules. The fire loop (actually triggering runs at the cron/at/
//! every time, unattended-mandate enforcement) is a later milestone.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
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
/// `5m`, `2h`, `1d`, or bare seconds), or a 5-field cron expression. The fire
/// loop (a later milestone) consumes the same grammar.
fn validate_trigger(spec: &str) -> Result<()> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("at:") {
        return parse_unix(rest).map(|_| ());
    }
    if let Some(rest) = spec.strip_prefix("every:") {
        return parse_duration_secs(rest).map(|_| ());
    }
    if spec.split_whitespace().count() == 5 {
        return Ok(()); // cron expression
    }
    Err(CoreError::Message(format!(
        "invalid trigger '{spec}'; use 'at:<unix_secs>', 'every:<dur, e.g. 30s/5m/1h/1d>', or a 5-field cron expression"
    )))
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

/// Whether a trigger should fire now given its last run. Only `at:`/`every:` are
/// fired by the loop; cron is validated on create but not yet fired.
pub(crate) fn trigger_due(spec: &str, last_run: Option<u64>, now: u64) -> bool {
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
    false
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
        let last_run = value.get("last_run").and_then(Value::as_u64);
        if trigger_due(trigger, last_run, now) {
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

/// Record that a schedule fired at `now` (sets `last_run`).
pub(crate) fn mark_fired(dir: &Path, id: &str, now: u64) -> Result<()> {
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(CoreError::Message(format!("invalid schedule id '{id}'")));
    }
    let path = dir.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::Message(format!("cannot read schedule '{id}': {e}")))?;
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Message(format!("corrupt schedule '{id}': {e}")))?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("last_run".to_string(), json!(now));
    }
    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| CoreError::Message(format!("serialize schedule: {e}")))?;
    std::fs::write(&path, pretty)
        .map_err(|e| CoreError::Message(format!("write schedule: {e}")))?;
    Ok(())
}

struct ScheduleCreate {
    dir: PathBuf,
}

#[async_trait]
impl Tool for ScheduleCreate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_create".to_string(),
            description: "Create a schedule. `trigger` is a cron expr / `at:<time>` / `every:<dur>`; `mandate` describes the authorized scope; `allowed_tools` lists the non-read tools the unattended run may use (enforced at fire time).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger": { "type": "string" },
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
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Ok(text) = std::fs::read_to_string(entry.path()) {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
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
        assert!(validate_trigger("0 9 * * 1").is_ok()); // cron
        assert!(validate_trigger("at:notanumber").is_err());
        assert!(validate_trigger("every:0").is_err());
        assert!(validate_trigger("every:5x").is_err());
        assert!(validate_trigger("garbage").is_err());
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
        assert!(!trigger_due("at:100", None, 50));
        assert!(trigger_due("at:100", None, 100));
        assert!(!trigger_due("at:100", Some(100), 200));
        assert!(trigger_due("every:60", None, 0));
        assert!(!trigger_due("every:60", Some(100), 130));
        assert!(trigger_due("every:60", Some(100), 160));
        assert!(trigger_due("every:1m", Some(100), 160));
        assert!(!trigger_due("0 9 * * 1", None, 999999));
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
