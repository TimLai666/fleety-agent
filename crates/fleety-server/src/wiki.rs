//! Knowledge wiki: a durable Obsidian-style markdown vault, separate from any
//! workspace. The agent distils general knowledge here (one note per concept,
//! `[[wikilinks]]`, tags) via these tools; the runtime just enforces that writes
//! stay inside the vault and use `.md`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the wiki tools rooted at `vault`. `models_dir` is the cache for the
/// semantic-search embedding model.
pub fn register(registry: &mut ToolRegistry, vault: &Path, models_dir: &Path, backups_dir: &Path) {
    registry.register(Box::new(WikiWrite {
        vault: vault.to_path_buf(),
        models_dir: models_dir.to_path_buf(),
        backups: backups_dir.to_path_buf(),
    }));
    registry.register(Box::new(WikiRead {
        vault: vault.to_path_buf(),
    }));
    registry.register(Box::new(WikiList {
        vault: vault.to_path_buf(),
    }));
    registry.register(Box::new(WikiSearch {
        vault: vault.to_path_buf(),
    }));
    registry.register(Box::new(crate::wiki_embed::WikiSemanticSearch {
        vault: vault.to_path_buf(),
        cache_dir: models_dir.to_path_buf(),
    }));
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Resolve a vault-relative `.md` path, refusing escapes.
fn resolve(vault: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty()
        || rel.starts_with('/')
        || rel.starts_with('\\')
        || rel.contains("..")
        || rel.contains(':')
        || rel.contains('\0')
    {
        return Err(CoreError::Message(format!(
            "invalid wiki path '{rel}': must be a relative path without '..' or ':'"
        )));
    }
    let with_ext = if rel.ends_with(".md") {
        rel.to_string()
    } else {
        format!("{rel}.md")
    };
    Ok(vault.join(with_ext))
}

/// Recursively collect `.md` files under `dir`, returning vault-relative paths.
fn collect_notes(vault: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_notes(vault, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(rel) = path.strip_prefix(vault) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

struct WikiWrite {
    vault: PathBuf,
    models_dir: PathBuf,
    /// Rollback store (outside the vault) — an overwrite backs up the old note here first.
    backups: PathBuf,
}

#[async_trait]
impl Tool for WikiWrite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wiki_write".to_string(),
            description: "Create or overwrite a knowledge-wiki note (markdown). Use frontmatter + [[wikilinks]]; one concept per note. Overwriting an existing note backs up the previous version (recoverable via rollback) and returns its backup_id.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "vault-relative path, e.g. concepts/esp32-flashing.md" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let content = require_str(&args, "content")?;
        let resolved = resolve(&self.vault, path)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("cannot create wiki dir: {e}")))?;
        }
        // Overwriting an existing note backs up the old content first (to the
        // rollback store, never inside the vault), so a clobber is recoverable
        // and reported — not silent.
        let mut backup_id: Option<String> = None;
        if resolved.exists() {
            if let Ok(rel) = resolved.strip_prefix(&self.vault) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                let info = fleety_tools::backup_existing(&self.backups, &rel, &resolved)?;
                backup_id = info.get("id").and_then(Value::as_str).map(str::to_string);
            }
        }
        std::fs::write(&resolved, content)
            .map_err(|e| CoreError::Message(format!("cannot write wiki note: {e}")))?;
        // Refresh the semantic index for just this note (best-effort, async).
        if let Ok(rel) = resolved.strip_prefix(&self.vault) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            crate::wiki_embed::reindex_note(
                self.vault.clone(),
                self.models_dir.clone(),
                rel,
                content.to_string(),
            );
        }
        Ok(json!({ "path": path, "bytes_written": content.len(), "backup_id": backup_id }))
    }
}

struct WikiRead {
    vault: PathBuf,
}

#[async_trait]
impl Tool for WikiRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wiki_read".to_string(),
            description: "Read a knowledge-wiki note by vault-relative path. Returns a \
                 line-numbered `numbered` view of the slice and `line_count`; pass \
                 `start_line`/`end_line` for a slice. The `NNN\\t` line-number prefix is NOT \
                 part of the note content — strip it before using text as an edit match."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["path"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let path = require_str(&args, "path")?;
        let start_line = args
            .get("start_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let end_line = args
            .get("end_line")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let resolved = resolve(&self.vault, path)?;
        let full = std::fs::read_to_string(&resolved)
            .map_err(|e| CoreError::Message(format!("cannot read wiki note '{path}': {e}")))?;
        let (slice, start, end, total) = fleety_tools::slice_lines(&full, start_line, end_line);
        // One view of the slice, not two — mirrors `read_file`: the numbered view
        // already carries the content, and all slice-returning read tools share
        // the same tool-result character budget.
        Ok(json!({
            "path": path,
            "numbered": fleety_tools::line_numbered(&slice, start.max(1)),
            "start_line": start,
            "end_line": end,
            "line_count": total,
        }))
    }
}

struct WikiList {
    vault: PathBuf,
}

#[async_trait]
impl Tool for WikiList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wiki_list".to_string(),
            description: "List all knowledge-wiki notes.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let mut notes = Vec::new();
        collect_notes(&self.vault, &self.vault, &mut notes);
        notes.sort();
        Ok(json!({ "notes": notes }))
    }
}

struct WikiSearch {
    vault: PathBuf,
}

#[async_trait]
impl Tool for WikiSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wiki_search".to_string(),
            description: "Search knowledge-wiki notes for a query (case-insensitive substring)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let query = require_str(&args, "query")?.to_lowercase();
        let mut notes = Vec::new();
        collect_notes(&self.vault, &self.vault, &mut notes);
        let mut matches = Vec::new();
        for rel in notes {
            if let Ok(content) = std::fs::read_to_string(self.vault.join(&rel)) {
                if let Some(pos) = content.to_lowercase().find(&query) {
                    let start = pos.saturating_sub(40);
                    let snippet: String = content.chars().skip(start).take(120).collect();
                    matches.push(json!({ "path": rel, "snippet": snippet }));
                }
            }
        }
        Ok(json!({ "matches": matches }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-wiki-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    #[tokio::test]
    async fn write_read_list_search() {
        let vault = temp();
        let models = temp();
        let backups = temp();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &vault, &models, &backups);

        registry
            .call("wiki_write", json!({ "path": "concepts/esp32", "content": "# ESP32\nflash via espflash [[serial-ports]]" }))
            .await
            .expect("write");
        // .md appended automatically
        let read = registry
            .call("wiki_read", json!({ "path": "concepts/esp32" }))
            .await
            .expect("read");
        assert!(read["numbered"]
            .as_str()
            .unwrap_or_default()
            .contains("espflash"));
        assert_eq!(read["line_count"], json!(2));
        // The slice is returned once, as the numbered view: no duplicate raw copy.
        assert!(
            read.get("content").is_none(),
            "wiki_read must not also return an unnumbered copy of the same slice"
        );

        // A slice reports its bounds and numbers lines from the slice start.
        let slice = registry
            .call(
                "wiki_read",
                json!({ "path": "concepts/esp32", "start_line": 2, "end_line": 2 }),
            )
            .await
            .expect("read slice");
        let numbered = slice["numbered"].as_str().unwrap_or_default();
        assert!(numbered.contains("     2\tflash via espflash"));
        assert!(!numbered.contains("# ESP32"));
        assert_eq!(slice["start_line"], json!(2));
        assert_eq!(slice["end_line"], json!(2));

        let listed = registry.call("wiki_list", json!({})).await.expect("list");
        assert_eq!(listed["notes"], json!(["concepts/esp32.md"]));

        let found = registry
            .call("wiki_search", json!({ "query": "ESPFLASH" }))
            .await
            .expect("search");
        assert_eq!(found["matches"].as_array().map(Vec::len).unwrap_or(0), 1);

        assert!(registry
            .call("wiki_read", json!({ "path": "../escape" }))
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&vault);
    }

    #[tokio::test]
    async fn overwrite_backs_up_old_note() {
        let vault = temp();
        let models = temp();
        let backups = temp();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &vault, &models, &backups);

        // First write: brand new note → no backup.
        let first = registry
            .call("wiki_write", json!({ "path": "n", "content": "original" }))
            .await
            .expect("first write");
        assert!(first["backup_id"].is_null(), "new note shouldn't back up");

        // Overwrite: the old content is backed up (reported + recoverable), not silent.
        let second = registry
            .call(
                "wiki_write",
                json!({ "path": "n", "content": "replacement" }),
            )
            .await
            .expect("overwrite");
        let id = second["backup_id"]
            .as_str()
            .expect("overwrite reports a backup id");
        // The backup holds the *previous* content and lives outside the vault.
        let backed = std::fs::read_to_string(backups.join(id).join("n.md")).expect("backup file");
        assert_eq!(backed, "original");
        assert_eq!(
            std::fs::read_to_string(vault.join("n.md")).expect("live note"),
            "replacement"
        );
        let listed = fleety_tools::list_backups(&backups).expect("list backups");
        assert_eq!(listed.len(), 1, "exactly one backup from the overwrite");

        let _ = std::fs::remove_dir_all(&vault);
        let _ = std::fs::remove_dir_all(&backups);
    }
}
