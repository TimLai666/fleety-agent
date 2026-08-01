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

/// Register the skill tools across the four tiers.
pub fn register(
    registry: &mut ToolRegistry,
    builtin: &Path,
    authored: &Path,
    installed: &Path,
    synced: &Path,
    conversation: &[PathBuf],
) {
    let tiers = || Tiers {
        builtin: builtin.to_path_buf(),
        authored: authored.to_path_buf(),
        installed: installed.to_path_buf(),
        synced: synced.to_path_buf(),
        conversation: conversation.to_vec(),
    };
    registry.register(Box::new(ListSkills(tiers())));
    registry.register(Box::new(UseSkill(tiers())));
    registry.register(Box::new(SkillValidate(tiers())));
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
    /// Runtime-synced from an external repo; lowest precedence.
    synced: PathBuf,
    /// Conversation-scoped project/user skill source dirs (highest precedence),
    /// ordered deep → shallow then user-global. Empty for a plain (non-scoped)
    /// registry.
    conversation: Vec<PathBuf>,
}

impl Tiers {
    /// Tiers in precedence order (highest first): installed, authored, builtin, synced.
    fn ordered(&self) -> [(&Path, &'static str); 4] {
        [
            (&self.installed, "installed"),
            (&self.authored, "authored"),
            (&self.builtin, "builtin"),
            (&self.synced, "synced"),
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
    /// Where the skill's `path` lives: `None` = the server host, `Some(id)` =
    /// that device (a conversation-scoped skill read from another device). A
    /// bare path is not a usable handle without its device.
    device: Option<String>,
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

/// Collect skills by name across the four tiers; installed > authored > builtin > synced.
/// Scan one skill source directory, inserting each skill dir (a dir with a
/// non-symlink `SKILL.md`) into `map` under `source`/`device`. Later calls
/// override earlier ones by name.
fn scan_dir(
    dir: &Path,
    source: &'static str,
    device: Option<&str>,
    map: &mut BTreeMap<String, SkillInfo>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
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
                    description: skill_description_of(&skill_md),
                    path: skill_md,
                    source,
                    device: device.map(str::to_string),
                },
            );
        }
    }
}

/// Collect skills by name across the four global tiers; installed > authored >
/// builtin > synced (lowest precedence scanned first so higher tiers override).
fn collect(t: &Tiers) -> BTreeMap<String, SkillInfo> {
    let mut map = BTreeMap::new();
    for (dir, source) in [
        (&t.synced, "synced"),
        (&t.builtin, "builtin"),
        (&t.authored, "authored"),
        (&t.installed, "installed"),
    ] {
        scan_dir(dir, source, None, &mut map);
    }
    map
}

/// Like [`collect`] but overlays the conversation-scoped sources on top
/// (highest precedence: project > user > installed > authored > builtin >
/// synced). Sources are scanned in reverse order so the earliest (deepest
/// project) source wins. Same-host sources carry `device: None`.
fn collect_scoped(t: &Tiers) -> BTreeMap<String, SkillInfo> {
    let mut map = collect(t);
    for dir in t.conversation.iter().rev() {
        scan_dir(dir, "conversation", None, &mut map);
    }
    map
}

/// Parse a leading YAML frontmatter block (`---` … `---`) into simple
/// `key: value` pairs, returning the pairs and the body after the block. Skills
/// follow the Agent Skills standard: SKILL.md opens with frontmatter carrying
/// `name` and `description`. If there is no well-formed frontmatter the map is
/// empty and the body is the whole text (legacy first-line skills still work).
fn split_frontmatter(text: &str) -> (BTreeMap<String, String>, &str) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = match t
        .strip_prefix("---\n")
        .or_else(|| t.strip_prefix("---\r\n"))
    {
        Some(r) => r,
        None => return (BTreeMap::new(), text),
    };
    let mut map = BTreeMap::new();
    let mut search = rest;
    let mut consumed = 0usize;
    loop {
        let (line, advance, last) = match search.find('\n') {
            Some(i) => (&search[..i], i + 1, false),
            None => (search, search.len(), true),
        };
        let trimmed = line.trim_end_matches('\r');
        if trimmed.trim() == "---" {
            return (map, &rest[consumed + advance..]);
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            let k = k.trim();
            if !k.is_empty() {
                map.insert(k.to_string(), v.trim().to_string());
            }
        }
        if last {
            // No closing fence — treat as no frontmatter.
            return (BTreeMap::new(), text);
        }
        consumed += advance;
        search = &search[advance..];
    }
}

/// The first non-empty line of a body with any leading `#` stripped — the legacy
/// description source for skills without frontmatter.
fn description_line(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

/// A skill's description for `list_skills`/triggering: the frontmatter
/// `description` (Agent Skills standard), falling back to the first body line.
fn skill_description(text: &str) -> String {
    let (fm, body) = split_frontmatter(text);
    if let Some(d) = fm.get("description") {
        let d = d.trim();
        if !d.is_empty() {
            return d.to_string();
        }
    }
    description_line(body)
}

fn skill_description_of(skill_md: &Path) -> String {
    std::fs::read_to_string(skill_md)
        .map(|text| skill_description(&text))
        .unwrap_or_default()
}

/// One validation finding (`severity` is "error" or "warning").
fn issue(severity: &str, message: String) -> Value {
    json!({ "severity": severity, "message": message })
}

/// Check a SKILL.md body against the Agent Skills format (YAML frontmatter with
/// `name` + `description`), returning findings (severity error/warning).
fn validate_body(text: &str) -> Vec<Value> {
    let mut issues = Vec::new();
    if text.trim().is_empty() {
        issues.push(issue(
            "error",
            "SKILL.md is empty; it needs YAML frontmatter with `name` and `description`"
                .to_string(),
        ));
        return issues;
    }
    let (fm, body) = split_frontmatter(text);
    if fm.is_empty() {
        issues.push(issue(
            "error",
            "missing YAML frontmatter: SKILL.md must open with a `---` block containing `name` \
             and `description`"
                .to_string(),
        ));
    } else {
        validate_name_field(fm.get("name"), &mut issues);
        validate_description_field(fm.get("description"), &mut issues);
    }
    let body_lines = body.lines().count();
    if body_lines > 500 {
        issues.push(issue(
            "warning",
            format!("SKILL.md body is {body_lines} lines; keep it lean and move detail into separate reference files (progressive disclosure)"),
        ));
    }
    issues
}

/// Validate the frontmatter `name` field per the Agent Skills rules.
fn validate_name_field(name: Option<&String>, issues: &mut Vec<Value>) {
    let Some(name) = name.map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        issues.push(issue(
            "error",
            "frontmatter is missing a non-empty `name`".to_string(),
        ));
        return;
    };
    if name.chars().count() > 64 {
        issues.push(issue(
            "error",
            format!("`name` is {} chars; the limit is 64", name.chars().count()),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        issues.push(issue(
            "error",
            "`name` may contain only lowercase letters, digits, and hyphens".to_string(),
        ));
    }
    let lower = name.to_ascii_lowercase();
    if lower.contains("anthropic") || lower.contains("claude") {
        issues.push(issue(
            "error",
            "`name` may not contain the reserved words 'anthropic' or 'claude'".to_string(),
        ));
    }
}

/// Validate the frontmatter `description` field per the Agent Skills rules.
fn validate_description_field(description: Option<&String>, issues: &mut Vec<Value>) {
    let Some(desc) = description.map(|s| s.trim()).filter(|s| !s.is_empty()) else {
        issues.push(issue(
            "error",
            "frontmatter is missing a non-empty `description`".to_string(),
        ));
        return;
    };
    if desc.chars().count() > 1024 {
        issues.push(issue(
            "error",
            format!(
                "`description` is {} chars; the limit is 1024",
                desc.chars().count()
            ),
        ));
    }
    if desc.chars().count() < 20 {
        issues.push(issue(
            "warning",
            "`description` is very short; it should say what the skill does AND when to use it"
                .to_string(),
        ));
    }
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
        let skills: Vec<Value> = collect_scoped(&self.0)
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
                    "device": info.device,
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
                 current task. Also returns the skill's `path` (its directory), so you can run a \
                 bundled `scripts/` tool via run_command on an absolute path. (Read other files in \
                 the skill with skill_read_file.)"
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
        let skills = collect_scoped(&self.0);
        let info = skills
            .get(name)
            .ok_or_else(|| CoreError::ToolNotFound(format!("skill '{name}'")))?;
        let content = std::fs::read_to_string(&info.path)
            .map_err(|e| CoreError::Message(format!("cannot read skill '{name}': {e}")))?;
        // The skill's directory (SKILL.md's parent), so the agent can run a
        // bundled `scripts/` tool via run_command using an absolute path.
        let dir = info
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Ok(
            json!({ "name": name, "source": info.source, "path": dir, "content": content, "device": info.device.clone() }),
        )
    }
}

struct SkillValidate(Tiers);

#[async_trait]
impl Tool for SkillValidate {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_validate".to_string(),
            description:
                "Check a skill's SKILL.md against the Agent Skills format. Pass `content` \
                 to validate a draft before writing, or `name` to validate an existing skill. \
                 Returns `ok` (no errors) plus `issues` (each with severity error/warning). \
                 Rules: SKILL.md opens with YAML frontmatter holding `name` and `description`. \
                 `name` ≤64 chars, lowercase letters/digits/hyphens only, no 'anthropic'/'claude', \
                 and should match the skill's directory name. `description` is non-empty, ≤1024 \
                 chars, and should say what the skill does AND when to use it. Keep the body lean \
                 (move detail into reference files). Directory names and in-skill file paths must \
                 also stay inside the skills store (no '..', no absolute paths)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "validate an existing skill by name" },
                    "content": { "type": "string", "description": "validate a draft SKILL.md body instead" }
                }
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let content = args.get("content").and_then(Value::as_str);
        let name = args.get("name").and_then(Value::as_str);
        if content.is_none() && name.is_none() {
            return Err(CoreError::Message(
                "provide 'name' (validate an existing skill) or 'content' (validate a draft)"
                    .to_string(),
            ));
        }
        let mut issues = Vec::new();
        // Resolve the SKILL.md body to check: a provided draft, or an existing skill.
        let body = match content {
            Some(c) => c.to_string(),
            None => {
                let name = name.unwrap_or_default();
                if let Err(e) = valid_name(name) {
                    issues.push(issue("error", e.report().message));
                    String::new()
                } else {
                    match self.0.locate(name) {
                        Some((dir, _)) => match std::fs::read_to_string(dir.join("SKILL.md")) {
                            Ok(t) => t,
                            Err(e) => {
                                issues.push(issue("error", format!("cannot read SKILL.md: {e}")));
                                String::new()
                            }
                        },
                        None => {
                            issues.push(issue("error", format!("no such skill '{name}'")));
                            String::new()
                        }
                    }
                }
            }
        };
        // Only run body checks when we actually have a body (no prior resolve error).
        if issues.is_empty() {
            issues.extend(validate_body(&body));
            // For an existing skill, the frontmatter name should match its dir.
            if content.is_none() {
                if let Some(dir_name) = name {
                    if let Some(fm_name) = split_frontmatter(&body).0.get("name") {
                        if fm_name.trim() != dir_name {
                            issues.push(issue(
                                "warning",
                                format!(
                                    "frontmatter `name` ('{}') doesn't match the skill's directory name ('{dir_name}')",
                                    fm_name.trim()
                                ),
                            ));
                        }
                    }
                }
            }
        }
        let ok = !issues.iter().any(|i| i["severity"] == "error");
        Ok(json!({ "ok": ok, "issues": issues }))
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
            description: "Read a file inside a skill (default SKILL.md). Returns a line-numbered \
                 `numbered` view of the slice and `line_count`; pass `start_line`/`end_line` \
                 for a slice. Works on any tier. The `NNN\\t` line-number prefix is NOT part \
                 of the file content — strip it before using text as an edit match."
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
        // One view of the slice, not two — mirrors `read_file`: the numbered view
        // already carries the content, and both tools share the same tool-result
        // character budget.
        Ok(json!({
            "name": name,
            "source": source,
            "file": file,
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
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(&s, "esp32", "# ESP32\n(synced)"); // lowest tier
        write_skill(&b, "esp32", "# ESP32\n(builtin)");
        write_skill(&a, "esp32", "# ESP32\n(authored)");
        write_skill(&i, "esp32", "# ESP32\n(installed)");
        write_skill(&b, "docker", "# Docker\nbuild");
        write_skill(&s, "synced-only", "# Synced Only\nfrom the repo"); // only in synced
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);

        let listed = reg.call("list_skills", json!({})).await.expect("list");
        let arr = listed["skills"].as_array().expect("arr");
        assert_eq!(arr.len(), 3); // esp32 (collapsed) + docker + synced-only
        let esp = arr
            .iter()
            .find(|s| s["name"] == json!("esp32"))
            .expect("esp");
        // installed beats authored/builtin/synced.
        assert_eq!(esp["source"], json!("installed"));
        assert!(esp["path"].as_str().unwrap_or_default().contains("esp32"));
        // A skill that exists only in the synced tier is served from synced.
        let synced_only = arr
            .iter()
            .find(|s| s["name"] == json!("synced-only"))
            .expect("synced-only");
        assert_eq!(synced_only["source"], json!("synced"));

        let used = reg
            .call("use_skill", json!({ "name": "esp32" }))
            .await
            .expect("use");
        assert_eq!(used["source"], json!("installed"));

        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// `skill_read_file` returns one view of the slice — the numbered one. A
    /// second, unnumbered copy of the same bytes would spend half the shared
    /// tool-result character budget on a duplicate.
    #[tokio::test]
    async fn skill_read_file_returns_only_the_numbered_view() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(&b, "triage", "alpha\nbeta\ngamma\n");
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);

        let r = reg
            .call(
                "skill_read_file",
                json!({ "name": "triage", "start_line": 2, "end_line": 3 }),
            )
            .await
            .expect("read skill file");

        assert!(
            r.get("content").is_none(),
            "skill_read_file must not also return an unnumbered copy of the same slice"
        );
        let numbered = r["numbered"].as_str().unwrap_or_default();
        assert!(numbered.contains("     2\tbeta"));
        assert!(numbered.contains("     3\tgamma"));
        assert!(!numbered.contains("alpha"));
        assert_eq!(r["start_line"], json!(2));
        assert_eq!(r["end_line"], json!(3));
        assert_eq!(r["line_count"], json!(3));
        assert_eq!(r["name"], json!("triage"));
        assert_eq!(r["file"], json!("SKILL.md"));
        assert_eq!(r["source"], json!("builtin"));

        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn author_multifile_and_builtin_is_readonly() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(&b, "shipped", "# Shipped\nbuiltin");
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);

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

        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn install_from_content_then_remove() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);

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

        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_names_and_files() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);
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
        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn validate_draft_content() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);

        // A well-formed draft (proper frontmatter) passes with no errors.
        let good = reg
            .call(
                "skill_validate",
                json!({ "content": "---\nname: triage\ndescription: Diagnose flaky CI runs; use when a build fails intermittently.\n---\n\n# Triage\n\nDo the thing.\n" }),
            )
            .await
            .expect("validate");
        assert_eq!(good["ok"], json!(true));

        // Empty body is an error.
        let empty = reg
            .call("skill_validate", json!({ "content": "   \n" }))
            .await
            .expect("validate");
        assert_eq!(empty["ok"], json!(false));

        // Missing frontmatter is an error (the Agent Skills format requires it).
        let no_fm = reg
            .call(
                "skill_validate",
                json!({ "content": "# just a heading\n\nbody\n" }),
            )
            .await
            .expect("validate");
        assert_eq!(no_fm["ok"], json!(false));
        assert!(no_fm["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .any(|m| m["message"]
                .as_str()
                .unwrap_or_default()
                .contains("frontmatter")));

        // An invalid name (uppercase) is an error.
        let bad_name = reg
            .call(
                "skill_validate",
                json!({ "content": "---\nname: BadName\ndescription: A long enough description of what and when.\n---\nbody\n" }),
            )
            .await
            .expect("validate");
        assert_eq!(bad_name["ok"], json!(false));

        // A too-short description warns but is not an error.
        let short = reg
            .call(
                "skill_validate",
                json!({ "content": "---\nname: tiny\ndescription: short\n---\nbody\n" }),
            )
            .await
            .expect("validate");
        assert_eq!(short["ok"], json!(true));
        assert!(short["issues"]
            .as_array()
            .expect("arr")
            .iter()
            .any(|m| m["severity"] == "warning"));

        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn validate_existing_and_missing_skill() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(
            &a,
            "good",
            "---\nname: good\ndescription: Does a useful thing; use when the user needs that thing.\n---\n\n# Good\n\nsteps\n",
        );
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);

        let ok = reg
            .call("skill_validate", json!({ "name": "good" }))
            .await
            .expect("validate");
        assert_eq!(ok["ok"], json!(true));

        // Unknown skill is an error.
        let missing = reg
            .call("skill_validate", json!({ "name": "nope" }))
            .await
            .expect("validate");
        assert_eq!(missing["ok"], json!(false));

        // Neither name nor content is an actionable error.
        assert!(reg.call("skill_validate", json!({})).await.is_err());

        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn use_skill_returns_path() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(
            &a,
            "demo",
            "---\nname: demo\ndescription: A demo skill that does a thing; use when demoing.\n---\n\n# Demo\n\nsteps\n",
        );
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);
        let used = reg
            .call("use_skill", json!({ "name": "demo" }))
            .await
            .expect("use");
        // New `path` field points at the skill's directory, alongside the
        // unchanged name/source/content.
        assert_eq!(used["source"], json!("authored"));
        assert!(used["path"].as_str().unwrap_or_default().contains("demo"));
        assert!(used["content"]
            .as_str()
            .unwrap_or_default()
            .contains("# Demo"));
        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn frontmatter_description_used_over_first_line() {
        let text = "---\nname: x\ndescription: real description here\n---\n# Heading\nbody\n";
        let (fm, body) = split_frontmatter(text);
        assert_eq!(
            fm.get("description").map(String::as_str),
            Some("real description here")
        );
        assert!(body.starts_with("# Heading"));
        assert_eq!(skill_description(text), "real description here");
        // Legacy (no frontmatter) falls back to the first body line.
        assert_eq!(skill_description("# legacy desc\nbody"), "legacy desc");
    }

    #[test]
    fn collect_carries_device() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(&a, "demo", "# Demo\nx");
        let t = Tiers {
            builtin: b.clone(),
            authored: a.clone(),
            installed: i.clone(),
            synced: s.clone(),
            conversation: Vec::new(),
        };
        // A global-tier skill carries device = None (server host).
        assert_eq!(collect(&t).get("demo").expect("demo").device, None);
        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn list_and_use_report_device() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(
            &a,
            "demo",
            "---\nname: demo\ndescription: A demo skill; use it when demoing here now.\n---\n# Demo\n",
        );
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);
        let listed = reg.call("list_skills", json!({})).await.expect("list");
        let entry = &listed["skills"].as_array().expect("arr")[0];
        assert!(entry.get("device").is_some(), "list reports a device field");
        assert!(entry["device"].is_null(), "a global skill's device is null");
        let used = reg
            .call("use_skill", json!({ "name": "demo" }))
            .await
            .expect("use");
        assert!(used.get("device").is_some() && used["device"].is_null());
        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn conversation_scoped_skill_is_isolated() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        let proj = temp();
        write_skill(
            &proj,
            "projskill",
            "---\nname: projskill\ndescription: A project-only skill for this conversation here.\n---\n# P\n",
        );
        let mut reg_a = ToolRegistry::new();
        register(&mut reg_a, &b, &a, &i, &s, std::slice::from_ref(&proj));
        let mut reg_b = ToolRegistry::new();
        register(&mut reg_b, &b, &a, &i, &s, &[]);
        let list_a = reg_a.call("list_skills", json!({})).await.expect("a");
        let list_b = reg_b.call("list_skills", json!({})).await.expect("b");
        let has = |v: &Value| {
            v["skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == json!("projskill"))
        };
        assert!(has(&list_a), "conversation A sees its project skill");
        assert!(
            !has(&list_b),
            "conversation B does not see A's project skill"
        );
        for d in [&b, &a, &i, &s, &proj] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn conversation_tier_overrides_global() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(
            &i,
            "dup",
            "---\nname: dup\ndescription: The installed version of the dup skill here now.\n---\ninstalled-body\n",
        );
        let proj = temp();
        write_skill(
            &proj,
            "dup",
            "---\nname: dup\ndescription: The project version of the dup skill here now.\n---\nproject-body\n",
        );
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, std::slice::from_ref(&proj));
        let listed = reg.call("list_skills", json!({})).await.expect("list");
        let dup = listed["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == json!("dup"))
            .expect("dup");
        assert_eq!(
            dup["source"],
            json!("conversation"),
            "the conversation (project) tier overrides installed"
        );
        let used = reg
            .call("use_skill", json!({ "name": "dup" }))
            .await
            .expect("use");
        assert!(used["content"].as_str().unwrap().contains("project-body"));
        for d in [&b, &a, &i, &s, &proj] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn agents_skill_overrides_claude_same_layer() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        // Two conversation sources for the same layer: .agents/skills then
        // .claude/skills (the order skill_sources now emits). collect_scoped
        // reverse-scans, so .agents is scanned last and wins.
        let agents_dir = temp();
        let claude_dir = temp();
        write_skill(
            &agents_dir,
            "dup",
            "---\nname: dup\ndescription: the agents version of the dup skill here now.\n---\nagents-body\n",
        );
        write_skill(
            &claude_dir,
            "dup",
            "---\nname: dup\ndescription: the claude version of the dup skill here now.\n---\nclaude-body\n",
        );
        let mut reg = ToolRegistry::new();
        register(
            &mut reg,
            &b,
            &a,
            &i,
            &s,
            &[agents_dir.clone(), claude_dir.clone()],
        );
        let used = reg
            .call("use_skill", json!({ "name": "dup" }))
            .await
            .expect("use");
        assert!(
            used["content"].as_str().unwrap().contains("agents-body"),
            ".agents/skills overrides .claude/skills at the same layer"
        );
        for d in [&b, &a, &i, &s, &agents_dir, &claude_dir] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn cross_device_or_absent_falls_back_to_global() {
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        write_skill(
            &i,
            "gg",
            "---\nname: gg\ndescription: A global installed skill present regardless of origin here.\n---\n# G\n",
        );
        // Cross-device / absent origin → empty conversation sources → only the
        // global tiers are served.
        let mut reg = ToolRegistry::new();
        register(&mut reg, &b, &a, &i, &s, &[]);
        let listed = reg.call("list_skills", json!({})).await.expect("list");
        let arr = listed["skills"].as_array().unwrap();
        assert!(
            arr.iter().all(|e| e["source"] != json!("conversation")),
            "no conversation-tier skills when sources are empty"
        );
        assert!(
            arr.iter().any(|e| e["name"] == json!("gg")),
            "global tiers are still served"
        );
        for d in [&b, &a, &i, &s] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn plugin_skill_joins_conversation() {
        // A plugin's skills dir, passed as a conversation source, surfaces in
        // list_skills — same mechanism the conn binding uses for enabled plugins.
        let (b, a, i, s) = (temp(), temp(), temp(), temp());
        let plugin_skills = temp();
        write_skill(
            &plugin_skills,
            "pskill",
            "---\nname: pskill\ndescription: a plugin-provided skill for this conversation now.\n---\n# P\n",
        );
        let mut reg = ToolRegistry::new();
        register(
            &mut reg,
            &b,
            &a,
            &i,
            &s,
            std::slice::from_ref(&plugin_skills),
        );
        let listed = reg.call("list_skills", json!({})).await.expect("list");
        assert!(listed["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == json!("pskill")));
        for d in [&b, &a, &i, &s, &plugin_skills] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
