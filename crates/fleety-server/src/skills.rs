//! Skills runtime: discover, load, and manage `SKILL.md` skill packs.
//!
//! A skill is a **directory** (it may hold `SKILL.md` plus scripts / reference
//! files). Three tiers live in separate dirs and merge by name with
//! **installed > authored > builtin** precedence:
//!
//! - **builtin** — shipped in the binary, seeded at boot, **read-only**.
//! - **authored** — skills the agent writes for itself from experience
//!   (Hermes-style). The agent owns these and edits them **autonomously**.
//! - **installed** — user-chosen packs. The agent installs/removes/edits these
//!   **only at the user's request**; they shadow a builtin/authored of the
//!   same name.
//!
//! Skills can be multi-file, so editing is file-level: `skill_list_files` /
//! `skill_read_file` / `skill_write_file` / `skill_edit_file` /
//! `skill_delete_file` operate on individual files within a skill, enforcing the
//! tier rules (builtin never mutated; a write to a not-yet-existing skill lands
//! in `authored`). `skill_install` / `skill_remove` manage whole packs.

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
    registry.register(Box::new(SkillRemove(tiers())));
    registry.register(Box::new(SkillListFiles(tiers())));
    registry.register(Box::new(SkillReadFile(tiers())));
    registry.register(Box::new(SkillWriteFile(tiers())));
    registry.register(Box::new(SkillEditFile(tiers())));
    registry.register(Box::new(SkillDeleteFile(tiers())));
}

#[derive(Clone)]
struct Tiers {
    builtin: PathBuf,
    authored: PathBuf,
    installed: PathBuf,
}

impl Tiers {
    /// Tiers in precedence order (highest first): installed, authored, builtin.
    fn ordered(&self) -> [(&Path, &'static str); 3] {
        [
            (&self.installed, "installed"),
            (&self.authored, "authored"),
            (&self.builtin, "builtin"),
        ]
    }

    /// Where the skill `name` currently lives (highest-precedence tier that has
    /// a directory for it), or `None` if it doesn't exist yet.
    fn locate(&self, name: &str) -> Option<(PathBuf, &'static str)> {
        for (dir, source) in self.ordered() {
            let d = dir.join(name);
            if d.is_dir() {
                return Some((d, source));
            }
        }
        None
    }
}

struct SkillInfo {
    description: String,
    path: PathBuf, // the SKILL.md path
    source: &'static str,
}

/// Reject skill names that could escape the skills store.
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

/// Reject in-skill file paths that could escape the skill directory.
fn valid_file(file: &str) -> Result<String> {
    let f = file.trim().replace('\\', "/");
    if f.is_empty()
        || f.starts_with('/')
        || f.split('/').any(|c| c == ".." || c.is_empty())
        || f.contains(':')
        || f.contains('\0')
    {
        return Err(CoreError::Message(format!(
            "invalid file path '{file}': must be relative inside the skill, no '..'"
        )));
    }
    Ok(f)
}

/// Collect skills by name across the three tiers; installed > authored > builtin.
fn collect(t: &Tiers) -> BTreeMap<String, SkillInfo> {
    let mut map = BTreeMap::new();
    // Insert builtin first, then authored, then installed so later overrides win.
    for (dir, source) in [
        (&t.builtin, "builtin"),
        (&t.authored, "authored"),
        (&t.installed, "installed"),
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
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

/// Recursively list files (relative paths) under a skill directory.
fn list_files_rec(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            list_files_rec(base, &path, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

// --- read-only: list / use ---

struct ListSkills(Tiers);

#[async_trait]
impl Tool for ListSkills {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_skills".to_string(),
            description: "List available skills (SKILL.md packs). Each entry's `source` is \
                 `\"builtin\"` (read-only), `\"authored\"` (you wrote it), or `\"installed\"` \
                 (user-chosen), plus its `path`. Precedence when names collide: installed > \
                 authored > builtin."
                .to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let skills: Vec<Value> = collect(&self.0)
            .into_iter()
            .map(|(name, info)| {
                let dir = info
                    .path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                json!({
                    "name": name,
                    "description": info.description,
                    "source": info.source,
                    "path": dir,
                })
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
            description: "Load a skill's SKILL.md instructions by name; follow them for the \
                 current task. (Read other files in the skill with skill_read_file.)"
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

// --- whole-pack lifecycle ---

struct SkillInstall {
    installed: PathBuf,
}

#[async_trait]
impl Tool for SkillInstall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_install".to_string(),
            description: "Install (or replace) a USER skill into the installed tier — only when \
                 the user asks you to install a skill. Provide the SKILL.md body via `content`, \
                 `from_url` (fetched, public hosts only), or `from_path` (a local SKILL.md file \
                 or a whole skill directory, copied including extra files). To create a skill for \
                 yourself, use skill_write_file instead (it lands in your authored tier)."
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
        let dir = self.installed.join(name);
        let (body, dir_src) = resolve_install_body(&args).await?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Message(format!("cannot create skill dir '{name}': {e}")))?;
        std::fs::write(dir.join("SKILL.md"), body)
            .map_err(|e| CoreError::Message(format!("cannot write skill '{name}': {e}")))?;
        let mut extra = 0;
        if let Some(src) = dir_src {
            extra = copy_extra_files(&src, &dir)?;
        }
        Ok(json!({ "name": name, "source": "installed", "installed": true, "extra_files": extra }))
    }
}

/// Resolve the SKILL.md body for an install: inline `content`, a `from_url`
/// fetch (SSRF-guarded), or a `from_path` (a local SKILL.md file or a skill
/// directory). Returns `(content, copied_dir_from)`.
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

/// Copy the non-SKILL.md files of a skill directory (best-effort, recursive,
/// no symlinks) alongside the SKILL.md we already wrote.
fn copy_extra_files(src_dir: &Path, dest_dir: &Path) -> Result<usize> {
    fn walk(src: &Path, base_src: &Path, base_dest: &Path, n: &mut usize) {
        let Ok(entries) = std::fs::read_dir(src) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false) {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                walk(&p, base_src, base_dest, n);
            } else if let Ok(rel) = p.strip_prefix(base_src) {
                if rel.to_string_lossy() == "SKILL.md" {
                    continue;
                }
                let dest = base_dest.join(rel);
                if let Some(parent) = dest.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::copy(&p, &dest).is_ok() {
                    *n += 1;
                }
            }
        }
    }
    let mut n = 0;
    walk(src_dir, src_dir, dest_dir, &mut n);
    Ok(n)
}

struct SkillRemove(Tiers);

#[async_trait]
impl Tool for SkillRemove {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_remove".to_string(),
            description: "Remove a whole skill by name. Authored skills: remove freely. Installed \
                 skills: only at the user's request. Built-in skills cannot be removed (to \
                 override one, install or author a skill of the same name — it shadows the \
                 built-in)."
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
        match self.0.locate(name) {
            Some((_, "builtin")) => Err(CoreError::Message(format!(
                "built-in skill '{name}' cannot be removed; install or author one of the same name to shadow it"
            ))),
            Some((dir, source)) => {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| CoreError::Message(format!("cannot remove skill '{name}': {e}")))?;
                Ok(json!({ "name": name, "source": source, "removed": true }))
            }
            None => Err(CoreError::Message(format!("no such skill '{name}'"))),
        }
    }
}

// --- file-level editing within a skill ---

/// Resolve the directory + source for a file op. `for_write` decides what to do
/// when the skill doesn't exist yet (create in `authored`) and refuses builtin.
fn resolve_skill_dir(t: &Tiers, name: &str, for_write: bool) -> Result<(PathBuf, &'static str)> {
    match t.locate(name) {
        Some((_, "builtin")) if for_write => Err(CoreError::Message(format!(
            "built-in skill '{name}' is read-only; install or author a skill of the same name to customise it"
        ))),
        Some((dir, source)) => Ok((dir, source)),
        None if for_write => Ok((t.authored.join(name), "authored")),
        None => Err(CoreError::Message(format!("no such skill '{name}'"))),
    }
}

struct SkillListFiles(Tiers);

#[async_trait]
impl Tool for SkillListFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_list_files".to_string(),
            description: "List the files inside a skill (relative paths) and its tier.".to_string(),
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
        let (dir, source) = resolve_skill_dir(&self.0, name, false)?;
        let mut files = Vec::new();
        list_files_rec(&dir, &dir, &mut files);
        files.sort();
        Ok(json!({ "name": name, "source": source, "files": files }))
    }
}

struct SkillReadFile(Tiers);

#[async_trait]
impl Tool for SkillReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_read_file".to_string(),
            description:
                "Read a file inside a skill (default SKILL.md). Returns raw `content` plus \
                 a line-numbered `numbered` view and `line_count`; pass `start_line`/`end_line` \
                 for a slice. Works on any tier."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "file": { "type": "string", "description": "skill-relative file (default SKILL.md)" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["name"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let file = valid_file(
            args.get("file")
                .and_then(Value::as_str)
                .unwrap_or("SKILL.md"),
        )?;
        let start_line = args
            .get("start_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let end_line = args
            .get("end_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let (dir, source) = resolve_skill_dir(&self.0, name, false)?;
        let full = std::fs::read_to_string(dir.join(&file)).map_err(|e| {
            CoreError::Message(format!("cannot read '{file}' in skill '{name}': {e}"))
        })?;
        let (slice, start, end, total) = fleety_tools::slice_lines(&full, start_line, end_line);
        Ok(json!({
            "name": name,
            "source": source,
            "file": file,
            "content": slice,
            "numbered": fleety_tools::line_numbered(&slice, start.max(1)),
            "start_line": start,
            "end_line": end,
            "line_count": total,
        }))
    }
}

struct SkillWriteFile(Tiers);

#[async_trait]
impl Tool for SkillWriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_write_file".to_string(),
            description: "Create or overwrite a file inside a skill (e.g. SKILL.md, a script, a \
                 reference). A write to a skill that doesn't exist yet creates it in your AUTHORED \
                 tier. Editing an installed skill's files: only at the user's request. Built-in \
                 skills are read-only. This is how you author multi-file skills."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "file": { "type": "string", "description": "skill-relative file (default SKILL.md)" },
                    "content": { "type": "string" }
                },
                "required": ["name", "content"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let file = valid_file(
            args.get("file")
                .and_then(Value::as_str)
                .unwrap_or("SKILL.md"),
        )?;
        let content = require_str(&args, "content")?;
        let (dir, source) = resolve_skill_dir(&self.0, name, true)?;
        let target = dir.join(&file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create dir for '{file}': {e}")))?;
        }
        std::fs::write(&target, content).map_err(|e| {
            CoreError::Message(format!("cannot write '{file}' in skill '{name}': {e}"))
        })?;
        Ok(json!({ "name": name, "source": source, "file": file, "bytes": content.len() }))
    }
}

struct SkillEditFile(Tiers);

#[async_trait]
impl Tool for SkillEditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_edit_file".to_string(),
            description: "Precise edit of a file inside a skill, two modes: (1) substring — \
                 replace `old` with `new` (`old` unique unless replace_all:true); (2) line-range \
                 — replace lines `start_line`..`end_line` (from skill_read_file) with `new`. \
                 Returns the post-edit `applied` region. Installed skills: only at the user's \
                 request. Built-in skills are read-only."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "file": { "type": "string", "description": "skill-relative file (default SKILL.md)" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                    "replace_all": { "type": "boolean" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["name", "new"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let file = valid_file(
            args.get("file")
                .and_then(Value::as_str)
                .unwrap_or("SKILL.md"),
        )?;
        let new = require_str(&args, "new")?;
        let (dir, source) = resolve_skill_dir(&self.0, name, true)?;
        let path = dir.join(&file);
        let content = std::fs::read_to_string(&path).map_err(|e| {
            CoreError::Message(format!("cannot read '{file}' in skill '{name}': {e}"))
        })?;

        let start_arg = args
            .get("start_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let (updated, replaced, change_start, change_len) = if let Some(start) = start_arg {
            let end = args
                .get("end_line")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(start);
            let (updated, inserted) = fleety_tools::replace_line_range(&content, start, end, new)?;
            (updated, 1usize, start, inserted)
        } else {
            let old = require_str(&args, "old")?;
            if old.is_empty() {
                return Err(CoreError::Message(
                    "provide 'old' (substring mode) or 'start_line'/'end_line' (line-range mode)"
                        .to_string(),
                ));
            }
            let replace_all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let count = content.matches(old).count();
            if count == 0 {
                return Err(CoreError::Message(format!(
                    "the 'old' text was not found in '{file}'"
                )));
            }
            if count > 1 && !replace_all {
                return Err(CoreError::Message(format!(
                    "the 'old' text appears {count} times in '{file}'; add context or set replace_all:true"
                )));
            }
            let pos = content.find(old).unwrap_or(0);
            let (updated, replaced) = if replace_all {
                (content.replace(old, new), count)
            } else {
                (content.replacen(old, new, 1), 1)
            };
            (
                updated,
                replaced,
                fleety_tools::line_of_offset(&content, pos),
                new.lines().count().max(1),
            )
        };

        std::fs::write(&path, &updated).map_err(|e| {
            CoreError::Message(format!("cannot write '{file}' in skill '{name}': {e}"))
        })?;
        Ok(json!({
            "name": name,
            "source": source,
            "file": file,
            "replaced": replaced,
            "applied": fleety_tools::region_view(&updated, change_start, change_len, 3),
            "line_count": updated.lines().count(),
        }))
    }
}

struct SkillDeleteFile(Tiers);

#[async_trait]
impl Tool for SkillDeleteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_delete_file".to_string(),
            description: "Delete a file inside a skill (not SKILL.md — use skill_remove for the \
                 whole pack). Installed skills: only at the user's request. Built-in skills are \
                 read-only."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "file": { "type": "string" }
                },
                "required": ["name", "file"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        valid_name(name)?;
        let file = valid_file(require_str(&args, "file")?)?;
        if file == "SKILL.md" {
            return Err(CoreError::Message(
                "won't delete SKILL.md (that unmakes the skill); use skill_remove to delete the whole skill".to_string(),
            ));
        }
        let (dir, source) = resolve_skill_dir(&self.0, name, true)?;
        let target = dir.join(&file);
        if !target.is_file() {
            return Err(CoreError::Message(format!(
                "no file '{file}' in skill '{name}'"
            )));
        }
        std::fs::remove_file(&target).map_err(|e| {
            CoreError::Message(format!("cannot delete '{file}' in skill '{name}': {e}"))
        })?;
        Ok(json!({ "name": name, "source": source, "file": file, "removed": true }))
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
    async fn list_use_precedence_and_path() {
        let (b, a, i) = (temp(), temp(), temp());
        write_skill(&b, "esp32", "# ESP32\n(builtin)");
        write_skill(&a, "esp32", "# ESP32\n(authored)");
        write_skill(&i, "esp32", "# ESP32\n(installed)");
        write_skill(&b, "docker", "# Docker\nbuild");
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i);

        let listed = reg.call("list_skills", json!({})).await.expect("list");
        let arr = listed["skills"].as_array().expect("arr");
        assert_eq!(arr.len(), 2); // esp32 (collapsed) + docker
        let esp = arr
            .iter()
            .find(|s| s["name"] == json!("esp32"))
            .expect("esp");
        assert_eq!(esp["source"], json!("installed"));
        assert!(esp["path"].as_str().unwrap_or_default().contains("esp32"));

        let used = reg
            .call("use_skill", json!({ "name": "esp32" }))
            .await
            .expect("use");
        assert_eq!(used["source"], json!("installed"));

        for d in [&b, &a, &i] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn author_multifile_and_builtin_is_readonly() {
        let (b, a, i) = (temp(), temp(), temp());
        write_skill(&b, "shipped", "# Shipped\nbuiltin");
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i);

        // Writing a new skill's SKILL.md creates it in authored.
        reg.call(
            "skill_write_file",
            json!({ "name": "triage", "file": "SKILL.md", "content": "# Triage\nstep one\nstep two\n" }),
        )
        .await
        .expect("author skill");
        // Add a second file (script) to the same authored skill.
        reg.call(
            "skill_write_file",
            json!({ "name": "triage", "file": "scripts/run.sh", "content": "echo hi\n" }),
        )
        .await
        .expect("author script");
        let files = reg
            .call("skill_list_files", json!({ "name": "triage" }))
            .await
            .expect("files");
        let list = files["files"].as_array().expect("arr");
        assert!(list.iter().any(|f| f == "SKILL.md"));
        assert!(list.iter().any(|f| f == "scripts/run.sh"));
        assert_eq!(files["source"], json!("authored"));

        // Line-range edit of the authored SKILL.md, with post-edit region.
        let e = reg
            .call(
                "skill_edit_file",
                json!({ "name": "triage", "start_line": 2, "end_line": 2, "new": "step ONE" }),
            )
            .await
            .expect("edit");
        assert!(e["applied"]
            .as_str()
            .unwrap_or_default()
            .contains("step ONE"));

        // Built-in skills are read-only for every mutation.
        assert!(reg
            .call(
                "skill_write_file",
                json!({ "name": "shipped", "file": "x.txt", "content": "y" })
            )
            .await
            .is_err());
        assert!(reg
            .call(
                "skill_edit_file",
                json!({ "name": "shipped", "old": "builtin", "new": "z" })
            )
            .await
            .is_err());
        assert!(reg
            .call("skill_remove", json!({ "name": "shipped" }))
            .await
            .is_err());

        // delete a non-SKILL file; refuse SKILL.md
        reg.call(
            "skill_delete_file",
            json!({ "name": "triage", "file": "scripts/run.sh" }),
        )
        .await
        .expect("del file");
        assert!(reg
            .call(
                "skill_delete_file",
                json!({ "name": "triage", "file": "SKILL.md" })
            )
            .await
            .is_err());

        // remove the whole authored skill
        reg.call("skill_remove", json!({ "name": "triage" }))
            .await
            .expect("remove");
        assert!(reg
            .call("use_skill", json!({ "name": "triage" }))
            .await
            .is_err());

        for d in [&b, &a, &i] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn install_from_content_then_remove() {
        let (b, a, i) = (temp(), temp(), temp());
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i);

        reg.call(
            "skill_install",
            json!({ "name": "deploy", "content": "# Deploy\nuser skill" }),
        )
        .await
        .expect("install");
        let used = reg
            .call("use_skill", json!({ "name": "deploy" }))
            .await
            .expect("use");
        assert_eq!(used["source"], json!("installed"));

        // install with no source errors
        assert!(reg
            .call("skill_install", json!({ "name": "x" }))
            .await
            .is_err());

        reg.call("skill_remove", json!({ "name": "deploy" }))
            .await
            .expect("remove");
        assert!(reg
            .call("use_skill", json!({ "name": "deploy" }))
            .await
            .is_err());

        for d in [&b, &a, &i] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_names_and_files() {
        let (b, a, i) = (temp(), temp(), temp());
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i);
        for bad in ["../etc", "a/b", ".hidden", ""] {
            assert!(reg
                .call("skill_write_file", json!({ "name": bad, "content": "x" }))
                .await
                .is_err());
        }
        // path escape in file
        assert!(reg
            .call(
                "skill_write_file",
                json!({ "name": "ok", "file": "../escape.txt", "content": "x" })
            )
            .await
            .is_err());
        for d in [&b, &a, &i] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
