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

struct ScheduleCreate {
    dir: PathBuf,
}

#[async_trait]
impl Tool for ScheduleCreate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "schedule_create".to_string(),
            description: "Create a schedule. `trigger` is a cron expr / `at:<time>` / `every:<dur>`; `mandate` records what the unattended run is authorized to do.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "trigger": { "type": "string" },
                    "prompt": { "type": "string" },
                    "mandate": { "type": "string", "description": "authorized scope (incl. critical actions), agreed now" }
                },
                "required": ["trigger", "prompt"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let trigger = require_str(&args, "trigger")?;
        let prompt = require_str(&args, "prompt")?;
        let mandate = args.get("mandate").and_then(Value::as_str).unwrap_or("");
        let id = uuid::Uuid::new_v4().to_string();
        let record = json!({
            "id": id,
            "trigger": trigger,
            "prompt": prompt,
            "mandate": mandate,
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
}
