//! Skills runtime: discover, load, and manage `SKILL.md` skill packs.
//!
//! A skill is a directory containing `SKILL.md`. Three tiers live in separate
//! dirs and merge by name with **installed > authored > builtin** precedence:
//!
//! - **builtin** — shipped in the binary, seeded at boot, read-only.
//! - **authored** — the agent writes these for itself from experience
//!   (Hermes-style). It may create / edit / delete them autonomously
//!   (`skill_author` / `skill_author_edit` / `skill_author_delete`).
//! - **installed** — user-chosen packs. The agent installs/removes these
//!   **only at the user's request** (`skill_install` / `skill_uninstall`);
//!   they shadow a builtin or authored skill of the same name.
//!
//! `list_skills` enumerates; `use_skill` returns a skill's instructions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the skill tools across the three tiers.
pub fn register(registry: &mut ToolRegistry, builtin: &Path, authored: &Path, installed: &Path) {
    let tiers = || Tiers {
        builtin: builtin.to_path_buf(),
        authored: authored.to_path_buf(),
        installed: installed.to_path_buf(),
    };
    registry.register(Box::new(ListSkills(tiers())));
    registry.register(Box::new(UseSkill(tiers())));
    registry.register(Box::new(SkillInstall {
        installed: installed.to_path_buf(),
    }));
    registry.register(Box::new(SkillUninstall {
        installed: installed.to_path_buf(),
    }));
    registry.register(Box::new(SkillAuthor {
        authored: authored.to_path_buf(),
    }));
    registry.register(Box::new(SkillAuthorEdit {
        authored: authored.to_path_buf(),
    }));
    registry.register(Box::new(SkillAuthorDelete {
        authored: authored.to_path_buf(),
    }));
}

#[derive(Clone)]
struct Tiers {
    builtin: PathBuf,
    authored: PathBuf,
    installed: PathBuf,
}

struct SkillInfo {
    description: String,
    path: PathBuf,
    source: &'static str,
}

/// Reject names that could escape the skills store.
fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        return Err(CoreError::Message(format!("invalid skill name '{name}'")));
    }
    Ok(())
}

/// Collect skills by name across the three tiers; later tiers override earlier
/// ones, so the precedence is installed > authored > builtin.
fn collect(t: &Tiers) -> BTreeMap<String, SkillInfo> {
    let mut map = BTreeMap::new();
    for (dir, source) in [
        (&t.builtin, "builtin"),
        (&t.authored, "authored"),
        (&t.installed, "installed"),
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // Skip symlinked skill dirs (could point outside the skills store).
            if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let skill_md = path.join("SKILL.md");
            let md_is_symlink = skill_md
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if path.is_dir() && skill_md.is_file() && !md_is_symlink {
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

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

// --- read-only: list / use ---

struct ListSkills(Tiers);

#[async_trait]
impl Tool for ListSkills {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_skills".to_string(),
            description: "List available skills (SKILL.md packs). Each entry's `source` is \
                 `\"builtin\"`, `\"authored\"` (you wrote it), or `\"installed\"` (user-chosen). \
                 Precedence when names collide: installed > authored > builtin."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let skills: Vec<Value> = collect(&self.0)
            .into_iter()
            .map(|(name, info)| {
                json!({ "name": name, "description": info.description, "source": info.source })
            })
            .collect();
        Ok(json!({ "skills": skills }))
    }
}

struct UseSkill(Tiers);

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
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let skills = collect(&self.0);
        let info = skills
            .get(name)
            .ok_or_else(|| CoreError::ToolNotFound(format!("skill '{name}'")))?;
        let content = std::fs::read_to_string(&info.path)
            .map_err(|e| CoreError::Message(format!("cannot read skill '{name}': {e}")))?;
        Ok(json!({ "name": name, "source": info.source, "content": content }))
    }
}

// --- shared write helpers ---

/// Write a skill's `SKILL.md` into `<tier_dir>/<name>/SKILL.md`, overwriting.
fn write_skill_md(tier_dir: &Path, name: &str, content: &str) -> Result<PathBuf> {
    let dir = tier_dir.join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| CoreError::Message(format!("cannot create skill dir '{name}': {e}")))?;
    let md = dir.join("SKILL.md");
    std::fs::write(&md, content)
        .map_err(|e| CoreError::Message(format!("cannot write skill '{name}': {e}")))?;
    Ok(md)
}

/// Remove `<tier_dir>/<name>` entirely. `Ok(false)` if it didn't exist.
fn remove_skill_dir(tier_dir: &Path, name: &str) -> Result<bool> {
    let dir = tier_dir.join(name);
    let md = dir.join("SKILL.md");
    if !md.is_file() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| CoreError::Message(format!("cannot remove skill '{name}': {e}")))?;
    Ok(true)
}

/// Resolve the SKILL.md body for an install request: inline `content`, a
/// `from_url` fetch (SSRF-guarded), or a `from_path` (a local SKILL.md file or
/// a skill directory containing one). Returns `(content, copied_dir_from)`.
async fn resolve_install_body(args: &Value) -> Result<(String, Option<PathBuf>)> {
    if let Some(content) = args.get("content").and_then(Value::as_str) {
        return Ok((content.to_string(), None));
    }
    if let Some(url) = args.get("from_url").and_then(Value::as_str) {
        let guarded = crate::web::guard_url_ex(url, &[])?;
        let body = reqwest::get(guarded)
            .await
            .map_err(|e| CoreError::Provider(format!("fetch skill from '{url}' failed: {e}")))?
            .error_for_status()
            .map_err(|e| CoreError::Provider(format!("fetch skill from '{url}': {e}")))?
            .text()
            .await
            .map_err(|e| CoreError::Provider(format!("reading skill body from '{url}': {e}")))?;
        return Ok((body, None));
    }
    if let Some(p) = args.get("from_path").and_then(Value::as_str) {
        let path = PathBuf::from(p);
        let md = if path.is_dir() {
            path.join("SKILL.md")
        } else {
            path.clone()
        };
        let body = std::fs::read_to_string(&md)
            .map_err(|e| CoreError::Message(format!("cannot read skill from '{p}': {e}")))?;
        let dir_src = if path.is_dir() { Some(path) } else { None };
        return Ok((body, dir_src));
    }
    Err(CoreError::Message(
        "provide one of: content, from_url, from_path".to_string(),
    ))
}

/// Copy the non-SKILL.md files of a skill directory alongside the SKILL.md we
/// already wrote (best-effort, one level deep — no nested dirs, no symlinks).
fn copy_extra_files(src_dir: &Path, dest_dir: &Path) -> Result<usize> {
    let mut copied = 0;
    let Ok(entries) = std::fs::read_dir(src_dir) else {
        return Ok(0);
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            let name = entry.file_name();
            if name == "SKILL.md" {
                continue;
            }
            let dest = dest_dir.join(&name);
            if std::fs::copy(entry.path(), &dest).is_ok() {
                copied += 1;
            }
        }
    }
    Ok(copied)
}

// --- user-installed lifecycle (consent required: only at user request) ---

struct SkillInstall {
    installed: PathBuf,
}

#[async_trait]
impl Tool for SkillInstall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_install".to_string(),
            description: "Install (or replace) a USER skill — only do this when the user asks you \
                 to install a skill. Provide the SKILL.md body via one of: `content` (inline), \
                 `from_url` (fetched, public hosts only), or `from_path` (a local SKILL.md file \
                 or a skill directory). Installed skills shadow builtin/authored of the same name. \
                 Use `skill_author` instead for skills you create for yourself."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "content": { "type": "string", "description": "inline SKILL.md body" },
                    "from_url": { "type": "string", "description": "URL to fetch SKILL.md from (public hosts only)" },
                    "from_path": { "type": "string", "description": "local SKILL.md file or skill directory to copy" }
                },
                "required": ["name"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let (body, dir_src) = resolve_install_body(&args).await?;
        write_skill_md(&self.installed, name, &body)?;
        let mut extra = 0;
        if let Some(src) = dir_src {
            extra = copy_extra_files(&src, &self.installed.join(name))?;
        }
        Ok(json!({ "name": name, "source": "installed", "installed": true, "extra_files": extra }))
    }
}

struct SkillUninstall {
    installed: PathBuf,
}

#[async_trait]
impl Tool for SkillUninstall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_uninstall".to_string(),
            description: "Remove a USER-installed skill by name — only at the user's request. \
                 Cannot remove builtin skills (shipped) or authored skills (use \
                 skill_author_delete for those)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        if !remove_skill_dir(&self.installed, name)? {
            return Err(CoreError::Message(format!(
                "no user-installed skill '{name}' (builtin and authored skills aren't removed here)"
            )));
        }
        Ok(json!({ "name": name, "removed": true }))
    }
}

// --- agent-authored lifecycle (autonomous: no user consent needed) ---

struct SkillAuthor {
    authored: PathBuf,
}

#[async_trait]
impl Tool for SkillAuthor {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_author".to_string(),
            description: "Create or replace a skill you author for yourself from experience \
                 (Hermes-style) — a whole SKILL.md in one shot. You own authored skills and may \
                 do this autonomously. To MERGE skills, author the combined one then \
                 skill_author_delete the originals; to SPLIT, author the new pieces then delete \
                 the original. For surgical changes use skill_author_edit."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "content": { "type": "string", "description": "the full SKILL.md body" }
                },
                "required": ["name", "content"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let content = require_str(&args, "content")?;
        write_skill_md(&self.authored, name, content)?;
        Ok(json!({ "name": name, "source": "authored", "saved": true, "bytes": content.len() }))
    }
}

struct SkillAuthorEdit {
    authored: PathBuf,
}

#[async_trait]
impl Tool for SkillAuthorEdit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_author_edit".to_string(),
            description: "Edit one of YOUR authored skills by exact substring replacement — the \
                 precise alternative to rewriting the whole SKILL.md with skill_author. `old` \
                 must be unique unless replace_all:true. Only works on authored skills."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["name", "old", "new"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let old = require_str(&args, "old")?;
        let new = require_str(&args, "new")?;
        if old.is_empty() {
            return Err(CoreError::Message(
                "'old' must be non-empty; use skill_author to create or replace the skill"
                    .to_string(),
            ));
        }
        let replace_all = args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let md = self.authored.join(name).join("SKILL.md");
        let content = match std::fs::read_to_string(&md) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::Message(format!(
                    "no authored skill '{name}'; create it with skill_author first"
                )))
            }
            Err(e) => {
                return Err(CoreError::Message(format!(
                    "cannot read skill '{name}': {e}"
                )))
            }
        };
        let count = content.matches(old).count();
        if count == 0 {
            return Err(CoreError::Message(format!(
                "the 'old' text was not found in skill '{name}'"
            )));
        }
        if count > 1 && !replace_all {
            return Err(CoreError::Message(format!(
                "the 'old' text appears {count} times in skill '{name}'; add context or set replace_all:true"
            )));
        }
        let (updated, replaced) = if replace_all {
            (content.replace(old, new), count)
        } else {
            (content.replacen(old, new, 1), 1)
        };
        std::fs::write(&md, &updated)
            .map_err(|e| CoreError::Message(format!("cannot write skill '{name}': {e}")))?;
        Ok(json!({ "name": name, "source": "authored", "replaced": replaced }))
    }
}

struct SkillAuthorDelete {
    authored: PathBuf,
}

#[async_trait]
impl Tool for SkillAuthorDelete {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_author_delete".to_string(),
            description: "Delete one of YOUR authored skills by name. You own authored skills and \
                 may do this autonomously. Does not touch builtin or user-installed skills."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        if !remove_skill_dir(&self.authored, name)? {
            return Err(CoreError::Message(format!("no authored skill '{name}'")));
        }
        Ok(json!({ "name": name, "removed": true }))
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
    async fn list_and_use_with_precedence() {
        let builtin = temp();
        let authored = temp();
        let installed = temp();
        write_skill(&builtin, "esp32", "# ESP32\nflash (builtin)");
        write_skill(&builtin, "docker", "# Docker\nbuild images");
        write_skill(&authored, "esp32", "# ESP32\nflash (authored)");
        write_skill(&authored, "habits", "# Habits\nlearned routine");
        write_skill(&installed, "esp32", "# ESP32\nflash (installed)");

        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &authored, &installed);

        let listed = registry.call("list_skills", json!({})).await.expect("list");
        let arr = listed["skills"].as_array().expect("arr");
        // esp32 (collapsed) + docker + habits = 3
        assert_eq!(arr.len(), 3);

        // installed wins over authored wins over builtin
        let used = registry
            .call("use_skill", json!({ "name": "esp32" }))
            .await
            .expect("use");
        assert_eq!(used["source"], json!("installed"));
        assert!(used["content"]
            .as_str()
            .unwrap_or_default()
            .contains("installed"));

        let habit = registry
            .call("use_skill", json!({ "name": "habits" }))
            .await
            .expect("use habits");
        assert_eq!(habit["source"], json!("authored"));

        for d in [&builtin, &authored, &installed] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn install_uninstall_and_builtin_protection() {
        let builtin = temp();
        let authored = temp();
        let installed = temp();
        write_skill(&builtin, "shipped", "# Shipped\nbuiltin");

        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &authored, &installed);

        // Install from inline content.
        registry
            .call(
                "skill_install",
                json!({ "name": "deploy", "content": "# Deploy\nuser skill" }),
            )
            .await
            .expect("install");
        let listed = registry.call("list_skills", json!({})).await.expect("list");
        let arr = listed["skills"].as_array().expect("arr");
        assert_eq!(arr.len(), 2);

        // Uninstall removes only from installed; a builtin name can't be uninstalled.
        assert!(registry
            .call("skill_uninstall", json!({ "name": "shipped" }))
            .await
            .is_err());
        registry
            .call("skill_uninstall", json!({ "name": "deploy" }))
            .await
            .expect("uninstall");
        assert!(registry
            .call("skill_uninstall", json!({ "name": "deploy" }))
            .await
            .is_err());

        // install with no source is rejected.
        assert!(registry
            .call("skill_install", json!({ "name": "x" }))
            .await
            .is_err());

        for d in [&builtin, &authored, &installed] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn author_create_edit_delete() {
        let builtin = temp();
        let authored = temp();
        let installed = temp();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &authored, &installed);

        // Author a whole skill.
        registry
            .call(
                "skill_author",
                json!({ "name": "triage", "content": "# Triage\nstep one\nstep two\n" }),
            )
            .await
            .expect("author");
        let used = registry
            .call("use_skill", json!({ "name": "triage" }))
            .await
            .expect("use");
        assert_eq!(used["source"], json!("authored"));

        // Fragment edit.
        registry
            .call(
                "skill_author_edit",
                json!({ "name": "triage", "old": "step two", "new": "step two (refined)" }),
            )
            .await
            .expect("edit");
        let after = registry
            .call("use_skill", json!({ "name": "triage" }))
            .await
            .expect("use2");
        assert!(after["content"]
            .as_str()
            .unwrap_or_default()
            .contains("refined"));

        // Editing a non-existent authored skill errors.
        assert!(registry
            .call(
                "skill_author_edit",
                json!({ "name": "ghost", "old": "a", "new": "b" })
            )
            .await
            .is_err());

        // Delete.
        registry
            .call("skill_author_delete", json!({ "name": "triage" }))
            .await
            .expect("delete");
        assert!(registry
            .call("use_skill", json!({ "name": "triage" }))
            .await
            .is_err());

        for d in [&builtin, &authored, &installed] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_names() {
        let builtin = temp();
        let authored = temp();
        let installed = temp();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &authored, &installed);
        for bad in ["../etc", "a/b", "a\\b", ".hidden", ""] {
            assert!(registry
                .call("skill_author", json!({ "name": bad, "content": "x" }))
                .await
                .is_err());
        }
        for d in [&builtin, &authored, &installed] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
