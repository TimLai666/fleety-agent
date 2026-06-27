//! Semantic search over the knowledge wiki, backed by a local **EmbeddingGemma**
//! model (fastembed / ONNX, CPU — downloaded once from the ungated
//! `onnx-community/embeddinggemma-300m-ONNX`, then offline).
//!
//! `wiki_search` (substring) is unchanged; this adds `wiki_semantic_search`,
//! which embeds the query and ranks note chunks by cosine similarity. The index
//! (`<vault>/.index/embeddings.json`) is kept in sync lazily: each search
//! re-embeds notes whose content hash changed and drops deleted ones, so it
//! never goes stale. A boot-time `warm()` does the same in the background so the
//! first search is fast.
//!
//! The model is a process-global singleton (the tool registry is rebuilt per
//! connection, so per-registry state would reload the weights every time). All
//! model/index work runs on a blocking thread (`spawn_blocking`) since fastembed
//! is synchronous and CPU-bound. `FLEETY_WIKI_EMBED=0` disables it (no download,
//! the tool returns an actionable error pointing at `wiki_search`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolSpec};
use async_trait::async_trait;
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// EmbeddingGemma's documented retrieval prompts (improve query/document
/// alignment). Applied manually — fastembed runs the tokenizer on raw text.
const QUERY_PREFIX: &str = "task: search result | query: ";
const DOC_PREFIX: &str = "title: none | text: ";
/// Q8 (int8) build — the chosen quality/size balance (~300MB). fastembed names
/// the int8 quantization `...Q`; `...Q4` is 4-bit, no suffix is fp32.
const MODEL: EmbeddingModel = EmbeddingModel::EmbeddingGemma300MQ;
const MODEL_TAG: &str = "embeddinggemma-300m-q8";
/// Rough chunk size in bytes; notes are split on blank lines and packed up to this.
const CHUNK_BYTES: usize = 1200;

pub fn enabled() -> bool {
    std::env::var("FLEETY_WIKI_EMBED").as_deref() != Ok("0")
}

/// Process-global model handle. `embed` needs `&mut self`, so a Mutex, not Arc.
fn model_cell() -> &'static Mutex<Option<TextEmbedding>> {
    static M: OnceLock<Mutex<Option<TextEmbedding>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// Process-global in-memory index cache (one wiki vault per server).
fn index_cell() -> &'static Mutex<Option<Index>> {
    static I: OnceLock<Mutex<Option<Index>>> = OnceLock::new();
    I.get_or_init(|| Mutex::new(None))
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    model: String,
    notes: HashMap<String, NoteIndex>,
}

#[derive(Serialize, Deserialize)]
struct NoteIndex {
    hash: String,
    chunks: Vec<Chunk>,
}

#[derive(Serialize, Deserialize)]
struct Chunk {
    text: String,
    vec: Vec<f32>,
}

fn index_path(vault: &Path) -> PathBuf {
    vault.join(".index").join("embeddings.json")
}

fn hash_str(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Split a note into chunks: blank-line paragraphs packed up to ~CHUNK_BYTES.
fn chunk_note(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for para in content.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if !cur.is_empty() && cur.len() + para.len() + 2 > CHUNK_BYTES {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
        // A single huge paragraph becomes its own (over-size) chunk.
        if cur.len() >= CHUNK_BYTES {
            chunks.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Initialise the model if needed, returning a guard error on failure. Blocking.
fn with_model<T>(cache_dir: &Path, f: impl FnOnce(&mut TextEmbedding) -> Result<T>) -> Result<T> {
    let mut guard = model_cell()
        .lock()
        .map_err(|_| CoreError::Message("embedding model lock poisoned".to_string()))?;
    if guard.is_none() {
        std::fs::create_dir_all(cache_dir).ok();
        let opts = TextInitOptions::new(MODEL)
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(false);
        let model = TextEmbedding::try_new(opts).map_err(|e| {
            CoreError::Message(format!(
                "could not load the EmbeddingGemma model (first use downloads ~300MB from \
                 huggingface; needs network). Set FLEETY_WIKI_EMBED=0 to disable semantic \
                 search and use wiki_search instead. Cause: {e}"
            ))
        })?;
        *guard = Some(model);
    }
    let model = guard
        .as_mut()
        .ok_or_else(|| CoreError::Message("embedding model unavailable".to_string()))?;
    f(model)
}

/// Embed a batch of texts (already prefixed). Blocking.
fn embed_texts(model: &mut TextEmbedding, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    model
        .embed(texts, None)
        .map_err(|e| CoreError::Message(format!("embedding failed: {e}")))
}

/// Load the on-disk index into the global cache if not already loaded.
fn load_index(vault: &Path) -> Result<()> {
    let mut guard = index_cell()
        .lock()
        .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
    if guard.is_some() {
        return Ok(());
    }
    let path = index_path(vault);
    let idx = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Index>(&text).unwrap_or_default(),
        Err(_) => Index::default(),
    };
    // A model change invalidates every vector.
    let idx = if idx.model == MODEL_TAG {
        idx
    } else {
        Index {
            model: MODEL_TAG.to_string(),
            notes: HashMap::new(),
        }
    };
    *guard = Some(idx);
    Ok(())
}

fn save_index(vault: &Path) -> Result<()> {
    let guard = index_cell()
        .lock()
        .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
    if let Some(idx) = guard.as_ref() {
        let path = index_path(vault);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CoreError::Message(format!("create index dir: {e}")))?;
        }
        let body = serde_json::to_string(idx)
            .map_err(|e| CoreError::Message(format!("serialize index: {e}")))?;
        std::fs::write(&path, body).map_err(|e| CoreError::Message(format!("write index: {e}")))?;
    }
    Ok(())
}

/// Re-embed notes whose content changed and drop deleted ones. Blocking.
fn sync_index(vault: &Path, cache_dir: &Path) -> Result<()> {
    load_index(vault)?;
    // Collect current notes + hashes (read outside the model lock).
    let mut current: Vec<(String, String, String)> = Vec::new(); // (rel, hash, content)
    collect_notes(vault, vault, &mut current);

    // Figure out which notes need (re)embedding.
    let mut to_embed: Vec<(String, Vec<String>)> = Vec::new();
    let present: std::collections::HashSet<String> =
        current.iter().map(|(rel, _, _)| rel.clone()).collect();
    {
        let mut guard = index_cell()
            .lock()
            .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
        let idx = guard
            .as_mut()
            .ok_or_else(|| CoreError::Message("index unavailable".to_string()))?;
        idx.notes.retain(|rel, _| present.contains(rel));
        for (rel, hash, content) in &current {
            let fresh = idx.notes.get(rel).map(|n| &n.hash == hash).unwrap_or(false);
            if !fresh {
                to_embed.push((rel.clone(), chunk_note(content)));
            }
        }
    }

    if to_embed.is_empty() {
        return Ok(());
    }

    // Embed all changed chunks in one model session.
    let prefixed: Vec<String> = to_embed
        .iter()
        .flat_map(|(_, chunks)| chunks.iter().map(|c| format!("{DOC_PREFIX}{c}")))
        .collect();
    let vectors = with_model(cache_dir, |m| embed_texts(m, prefixed))?;

    // Stitch vectors back per note and update the index.
    let mut guard = index_cell()
        .lock()
        .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
    let idx = guard
        .as_mut()
        .ok_or_else(|| CoreError::Message("index unavailable".to_string()))?;
    let mut vi = 0usize;
    let hashes: HashMap<&str, &str> = current
        .iter()
        .map(|(rel, hash, _)| (rel.as_str(), hash.as_str()))
        .collect();
    for (rel, chunks) in &to_embed {
        let mut note_chunks = Vec::with_capacity(chunks.len());
        for text in chunks {
            if vi < vectors.len() {
                note_chunks.push(Chunk {
                    text: text.clone(),
                    vec: vectors[vi].clone(),
                });
                vi += 1;
            }
        }
        let hash = hashes.get(rel.as_str()).copied().unwrap_or("").to_string();
        idx.notes.insert(
            rel.clone(),
            NoteIndex {
                hash,
                chunks: note_chunks,
            },
        );
    }
    drop(guard);
    save_index(vault)?;
    Ok(())
}

/// Collect `(rel, hash, content)` for every `.md` note under the vault.
fn collect_notes(vault: &Path, dir: &Path, out: &mut Vec<(String, String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            // Skip the index dir itself.
            if path.file_name().and_then(|n| n.to_str()) == Some(".index") {
                continue;
            }
            collect_notes(vault, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let (Ok(rel), Ok(content)) =
                (path.strip_prefix(vault), std::fs::read_to_string(&path))
            {
                let rel = rel.to_string_lossy().replace('\\', "/");
                let hash = hash_str(&content);
                out.push((rel, hash, content));
            }
        }
    }
}

/// Run a semantic search end-to-end (blocking).
fn search_blocking(vault: &Path, cache_dir: &Path, query: &str, top_k: usize) -> Result<Value> {
    sync_index(vault, cache_dir)?;
    let qvec = with_model(cache_dir, |m| {
        Ok(embed_texts(m, vec![format!("{QUERY_PREFIX}{query}")])?
            .into_iter()
            .next()
            .unwrap_or_default())
    })?;

    let guard = index_cell()
        .lock()
        .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
    let idx = guard
        .as_ref()
        .ok_or_else(|| CoreError::Message("index unavailable".to_string()))?;
    // Best chunk per note.
    let mut scored: Vec<(f32, String, String)> = Vec::new(); // (score, rel, snippet)
    for (rel, note) in &idx.notes {
        let mut best: Option<(f32, &str)> = None;
        for chunk in &note.chunks {
            let s = cosine(&qvec, &chunk.vec);
            if best.map(|(bs, _)| s > bs).unwrap_or(true) {
                best = Some((s, chunk.text.as_str()));
            }
        }
        if let Some((s, text)) = best {
            let snippet: String = text.chars().take(200).collect();
            scored.push((s, rel.clone(), snippet));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k.max(1));
    let matches: Vec<Value> = scored
        .into_iter()
        .map(|(score, path, snippet)| json!({ "path": path, "score": score, "snippet": snippet }))
        .collect();
    Ok(json!({ "matches": matches }))
}

/// Re-embed a single note immediately (blocking). Called right after wiki_write
/// so the semantic index reflects the new content without waiting for the next
/// search's lazy sync. Skips work if the note's hash is already current.
fn reindex_note_blocking(vault: &Path, cache_dir: &Path, rel: &str, content: &str) -> Result<()> {
    load_index(vault)?;
    let hash = hash_str(content);
    {
        let guard = index_cell()
            .lock()
            .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
        if let Some(idx) = guard.as_ref() {
            if idx.notes.get(rel).map(|n| n.hash == hash).unwrap_or(false) {
                return Ok(()); // unchanged
            }
        }
    }
    let chunks = chunk_note(content);
    let prefixed: Vec<String> = chunks.iter().map(|c| format!("{DOC_PREFIX}{c}")).collect();
    let vectors = with_model(cache_dir, |m| embed_texts(m, prefixed))?;
    {
        let mut guard = index_cell()
            .lock()
            .map_err(|_| CoreError::Message("index lock poisoned".to_string()))?;
        let idx = guard
            .as_mut()
            .ok_or_else(|| CoreError::Message("index unavailable".to_string()))?;
        let note_chunks = chunks
            .into_iter()
            .zip(vectors)
            .map(|(text, vec)| Chunk { text, vec })
            .collect();
        idx.notes.insert(
            rel.to_string(),
            NoteIndex {
                hash,
                chunks: note_chunks,
            },
        );
    }
    save_index(vault)
}

/// Fire-and-forget re-embed of one note after a wiki_write. Best-effort: a
/// failure (e.g. model not yet downloaded) just leaves the next search's lazy
/// sync to catch up. No-op when semantic search is disabled.
pub fn reindex_note(vault: PathBuf, cache_dir: PathBuf, rel: String, content: String) {
    if !enabled() {
        return;
    }
    tokio::spawn(async move {
        let res = tokio::task::spawn_blocking(move || {
            reindex_note_blocking(&vault, &cache_dir, &rel, &content)
        })
        .await;
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::debug!(report = ?e.report(), "wiki note reindex deferred to lazy sync")
            }
            Err(e) => tracing::debug!("wiki reindex task join error: {e}"),
        }
    });
}

/// Background warm at boot: build/refresh the index so the first search is fast.
/// Best-effort and non-fatal.
pub async fn warm(vault: PathBuf, cache_dir: PathBuf) {
    if !enabled() {
        return;
    }
    if !vault.is_dir() {
        return;
    }
    let res = tokio::task::spawn_blocking(move || sync_index(&vault, &cache_dir)).await;
    match res {
        Ok(Ok(())) => tracing::info!("wiki semantic index warmed"),
        Ok(Err(e)) => {
            tracing::warn!(report = ?e.report(), "wiki index warm failed (semantic search will retry on demand)")
        }
        Err(e) => tracing::warn!("wiki index warm task join error: {e}"),
    }
}

pub struct WikiSemanticSearch {
    pub vault: PathBuf,
    pub cache_dir: PathBuf,
}

#[async_trait]
impl Tool for WikiSemanticSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "wiki_semantic_search".to_string(),
            description:
                "Search the knowledge wiki by meaning (local EmbeddingGemma vectors), not \
                 keywords — finds notes related to the query even with different wording. Returns \
                 the top notes with a cosine `score` and snippet. Complements `wiki_search` \
                 (exact substring). First use downloads the model (~300MB)."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "description": "max results (default 5)" }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        if !enabled() {
            return Err(CoreError::Message(
                "semantic search is disabled (FLEETY_WIKI_EMBED=0); use wiki_search for substring \
                 search instead"
                    .to_string(),
            ));
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CoreError::Message("missing required string argument 'query'".to_string())
            })?
            .to_string();
        let top_k = args.get("top_k").and_then(Value::as_u64).unwrap_or(5) as usize;
        let vault = self.vault.clone();
        let cache = self.cache_dir.clone();
        tokio::task::spawn_blocking(move || search_blocking(&vault, &cache, &query, top_k))
            .await
            .map_err(|e| CoreError::Message(format!("semantic search task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_packs_paragraphs() {
        let note = "# Title\n\npara one\n\npara two\n\npara three";
        let chunks = chunk_note(note);
        assert!(!chunks.is_empty());
        // small note fits in one chunk
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("para three"));

        // a big paragraph forces a split
        let big = format!("{}\n\n{}", "x".repeat(1300), "tail");
        let c2 = chunk_note(&big);
        assert!(c2.len() >= 2);
    }

    #[test]
    fn cosine_basic() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn hash_is_stable_and_sensitive() {
        assert_eq!(hash_str("abc"), hash_str("abc"));
        assert_ne!(hash_str("abc"), hash_str("abd"));
    }
}
