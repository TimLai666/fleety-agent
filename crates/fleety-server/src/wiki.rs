//! Knowledge wiki: a durable Obsidian-style markdown vault, separate from any
//! workspace. The agent distils general knowledge here (one note per concept,
//! `[[wikilinks]]`, tags) via these tools; the runtime just enforces that writes
//! stay inside the vault and use `.md`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the wiki tools rooted at `vault`.
pub fn register(registry: &mut ToolRegistry, vault: &Path) {
    registry.register(Box::new(WikiWrite {
        vault: vault.to_path_buf(),
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
}

#[async_trait]
impl Tool for WikiWrite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wiki_write".to_string(),
            description: "Create or overwrite a knowledge-wiki note (markdown). Use frontmatter + [[wikilinks]]; one concept per note.".to_string(),
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
        std::fs::write(&resolved, content)
            .map_err(|e| CoreError::Message(format!("cannot write wiki note: {e}")))?;
        Ok(json!({ "path": path, "bytes_written": content.len() }))
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
            description: "Read a knowledge-wiki note by vault-relative path. Returns raw `content` \
                 plus a line-numbered `numbered` view and `line_count`; pass `start_line`/`end_line` \
                 for a slice."
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
        Ok(json!({
            "path": path,
            "content": slice,
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
        let mut registry = ToolRegistry::new();
        register(&mut registry, &vault);

        registry
            .call("wiki_write", json!({ "path": "concepts/esp32", "content": "# ESP32\nflash via espflash [[serial-ports]]" }))
            .await
            .expect("write");
        // .md appended automatically
        let read = registry
            .call("wiki_read", json!({ "path": "concepts/esp32" }))
            .await
            .expect("read");
        assert!(read["content"]
            .as_str()
            .unwrap_or_default()
            .contains("espflash"));

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
}
