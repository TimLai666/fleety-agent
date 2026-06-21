//! Skills runtime: discover and load `SKILL.md` skill packs.
//!
//! A skill is a directory containing `SKILL.md`. Built-in skills (shipped) and
//! installed skills (user) live in separate dirs; an installed skill overrides a
//! built-in of the same name. Read-only: `list_skills` enumerates, `use_skill`
//! returns a skill's instructions for the agent to follow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the skill tools, scanning `builtin` then `installed`.
pub fn register(registry: &mut ToolRegistry, builtin: &Path, installed: &Path) {
    registry.register(Box::new(ListSkills {
        builtin: builtin.to_path_buf(),
        installed: installed.to_path_buf(),
    }));
    registry.register(Box::new(UseSkill {
        builtin: builtin.to_path_buf(),
        installed: installed.to_path_buf(),
    }));
}

struct SkillInfo {
    description: String,
    path: PathBuf,
    source: &'static str,
}

/// Collect skills by name; installed overrides builtin.
fn collect(builtin: &Path, installed: &Path) -> BTreeMap<String, SkillInfo> {
    let mut map = BTreeMap::new();
    for (dir, source) in [(builtin, "builtin"), (installed, "installed")] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let skill_md = path.join("SKILL.md");
            if path.is_dir() && skill_md.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                map.insert(
                    name,
                    SkillInfo {
                        description: first_line(&skill_md),
                        path: skill_md,
                        source,
                    },
                );
            }
        }
    }
    map
}

fn first_line(skill_md: &Path) -> String {
    std::fs::read_to_string(skill_md)
        .ok()
        .and_then(|text| {
            text.lines()
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .find(|l| !l.is_empty())
        })
        .unwrap_or_default()
}

struct ListSkills {
    builtin: PathBuf,
    installed: PathBuf,
}

#[async_trait]
impl Tool for ListSkills {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_skills".to_string(),
            description: "List available skills (SKILL.md packs) the agent can load.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let skills: Vec<Value> = collect(&self.builtin, &self.installed)
            .into_iter()
            .map(|(name, info)| json!({ "name": name, "description": info.description, "source": info.source }))
            .collect();
        Ok(json!({ "skills": skills }))
    }
}

struct UseSkill {
    builtin: PathBuf,
    installed: PathBuf,
}

#[async_trait]
impl Tool for UseSkill {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "use_skill".to_string(),
            description: "Load a skill's instructions by name; follow them for the current task."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = args.get("name").and_then(Value::as_str).ok_or_else(|| {
            CoreError::Message("missing required string argument 'name'".to_string())
        })?;
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(CoreError::Message(format!("invalid skill name '{name}'")));
        }
        let skills = collect(&self.builtin, &self.installed);
        let info = skills
            .get(name)
            .ok_or_else(|| CoreError::ToolNotFound(format!("skill '{name}'")))?;
        let content = std::fs::read_to_string(&info.path)
            .map_err(|e| CoreError::Message(format!("cannot read skill '{name}': {e}")))?;
        Ok(json!({ "name": name, "source": info.source, "content": content }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    fn write_skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("mk skill dir");
        std::fs::write(dir.join("SKILL.md"), body).expect("write SKILL.md");
    }

    #[tokio::test]
    async fn list_and_use_with_installed_override() {
        let builtin = temp();
        let installed = temp();
        write_skill(&builtin, "esp32", "# ESP32\nflash firmware (builtin)");
        write_skill(&builtin, "docker", "# Docker\nbuild images");
        write_skill(
            &installed,
            "esp32",
            "# ESP32\nflash firmware (installed override)",
        );

        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &installed);

        let listed = registry.call("list_skills", json!({})).await.expect("list");
        let arr = listed["skills"].as_array().expect("arr");
        assert_eq!(arr.len(), 2); // esp32 (overridden) + docker

        let used = registry
            .call("use_skill", json!({ "name": "esp32" }))
            .await
            .expect("use");
        assert_eq!(used["source"], json!("installed"));
        assert!(used["content"]
            .as_str()
            .unwrap_or_default()
            .contains("installed override"));

        assert!(registry
            .call("use_skill", json!({ "name": "nope" }))
            .await
            .is_err());
        assert!(registry
            .call("use_skill", json!({ "name": "../etc" }))
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&builtin);
        let _ = std::fs::remove_dir_all(&installed);
    }
}
