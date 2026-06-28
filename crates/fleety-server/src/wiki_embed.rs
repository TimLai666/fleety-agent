//! Semantic search over the knowledge wiki, backed by a local **EmbeddingGemma**
//! model (fastembed / ONNX, CPU — downloaded once from the ungated
//! `onnx-community/embeddinggemma-300m-ONNX`, then offline) and an **on-disk
//! sqlite-vec** vector store.
//!
//! Everything lives in one SQLite file, `<vault>/.index/wiki.db` — vectors
//! included. There is **no in-RAM vector index**: the `vec0` virtual table holds
//! the embeddings on disk and KNN runs as a SQL query (`WHERE embedding MATCH ?
//! ORDER BY distance`), so memory doesn't grow with the wiki. Tables:
//! `meta(model, dim)`, `notes(rel, hash)`, `chunks(key, rel, text)`, and the
//! `vec_chunks` vec0 table keyed by `chunks.key` (cosine distance).
//!
//! The index stays current automatically: each search re-embeds notes whose
//! content hash changed and `DELETE`s the rows of changed/deleted notes (real
//! incremental updates). `wiki_write` re-embeds the edited note immediately. A
//! boot-time `warm()` syncs in the background. `FLEETY_WIKI_EMBED=0` disables it
//! (no download; the tool returns an actionable error pointing at `wiki_search`).
//!
//! The model + DB connection are process-global singletons (the tool registry
//! is rebuilt per connection). All model/DB work runs on a blocking thread
//! (`spawn_blocking`); fastembed and rusqlite are synchronous.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolSpec};
use async_trait::async_trait;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// Shared embedding access (one model + one gate for wiki and conversation recall).
use crate::embed::{
    embed_texts, enabled, vec_bytes, with_model, DOC_PREFIX, MODEL_TAG, QUERY_PREFIX,
};

/// Rough chunk size in bytes; notes are split on blank lines and packed up to this.
const CHUNK_BYTES: usize = 1200;

/// Process-global SQLite connection (one wiki vault per server).
fn conn_cell() -> &'static Mutex<Option<Connection>> {
    static C: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn db_path(vault: &Path) -> PathBuf {
    vault.join(".index").join("wiki.db")
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

/// `(note rel, content hash, [(chunk text, embedding)])` for one (re)embedded note.
type EmbeddedNote = (String, String, Vec<(String, Vec<f32>)>);

/// Open the DB (creating base tables); reset everything if the model changed.
fn open_db(vault: &Path) -> Result<Connection> {
    fleety_vec::register();
    if let Some(parent) = db_path(vault).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("create index dir: {e}")))?;
    }
    let conn = Connection::open(db_path(vault))
        .map_err(|e| CoreError::Message(format!("open wiki index db: {e}")))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta(id INTEGER PRIMARY KEY CHECK (id = 0), model TEXT, dim INTEGER);
         CREATE TABLE IF NOT EXISTS notes(rel TEXT PRIMARY KEY, hash TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS chunks(key INTEGER PRIMARY KEY AUTOINCREMENT, rel TEXT NOT NULL, text TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS chunks_rel ON chunks(rel);",
    )
    .map_err(|e| CoreError::Message(format!("init wiki index schema: {e}")))?;

    // A model change invalidates every vector; wipe and start fresh.
    let stored: Option<String> = conn
        .query_row("SELECT model FROM meta WHERE id = 0", [], |r| r.get(0))
        .ok();
    if stored.as_deref() != Some(MODEL_TAG) {
        conn.execute_batch(
            "DROP TABLE IF EXISTS vec_chunks; DELETE FROM chunks; DELETE FROM notes;",
        )
        .map_err(|e| CoreError::Message(format!("reset wiki index: {e}")))?;
        conn.execute(
            "INSERT INTO meta(id, model, dim) VALUES (0, ?1, NULL)
             ON CONFLICT(id) DO UPDATE SET model = ?1, dim = NULL",
            [MODEL_TAG],
        )
        .map_err(|e| CoreError::Message(format!("set model tag: {e}")))?;
    }
    Ok(conn)
}

/// Run `f` with the global connection, opening it on first use. Blocking.
fn with_conn<T>(vault: &Path, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let mut guard = conn_cell()
        .lock()
        .map_err(|_| CoreError::Message("wiki db lock poisoned".to_string()))?;
    if guard.is_none() {
        *guard = Some(open_db(vault)?);
    }
    let conn = guard
        .as_ref()
        .ok_or_else(|| CoreError::Message("wiki db unavailable".to_string()))?;
    f(conn)
}

/// The vector dimensionality recorded in `meta`, if any.
fn stored_dim(conn: &Connection) -> Option<usize> {
    conn.query_row("SELECT dim FROM meta WHERE id = 0", [], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .ok()
    .flatten()
    .map(|d| d as usize)
}

/// Create the `vec_chunks` virtual table at `dim` (idempotent) and record the dim.
fn ensure_vec_table(conn: &Connection, dim: usize) -> Result<()> {
    if stored_dim(conn) == Some(dim) {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(\
            key INTEGER PRIMARY KEY, embedding float[{dim}] distance_metric=cosine);"
    ))
    .map_err(|e| CoreError::Message(format!("create vec table: {e}")))?;
    conn.execute("UPDATE meta SET dim = ?1 WHERE id = 0", [dim as i64])
        .map_err(|e| CoreError::Message(format!("set dim: {e}")))?;
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

/// Delete a note's chunks + vectors (best-effort) and its `notes` row.
fn delete_note_rows(conn: &Connection, rel: &str) -> Result<()> {
    // vec_chunks may not exist yet (no dim); ignore that error.
    let _ = conn.execute(
        "DELETE FROM vec_chunks WHERE key IN (SELECT key FROM chunks WHERE rel = ?1)",
        [rel],
    );
    conn.execute("DELETE FROM chunks WHERE rel = ?1", [rel])
        .map_err(|e| CoreError::Message(format!("delete chunks: {e}")))?;
    conn.execute("DELETE FROM notes WHERE rel = ?1", [rel])
        .map_err(|e| CoreError::Message(format!("delete note: {e}")))?;
    Ok(())
}

/// Insert a note's chunks + vectors and its hash. Requires the vec table.
fn insert_note_rows(
    conn: &Connection,
    rel: &str,
    hash: &str,
    chunks: &[(String, Vec<f32>)],
) -> Result<()> {
    for (text, vec) in chunks {
        conn.execute(
            "INSERT INTO chunks(rel, text) VALUES (?1, ?2)",
            rusqlite::params![rel, text],
        )
        .map_err(|e| CoreError::Message(format!("insert chunk: {e}")))?;
        let key = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO vec_chunks(key, embedding) VALUES (?1, ?2)",
            rusqlite::params![key, vec_bytes(vec)],
        )
        .map_err(|e| CoreError::Message(format!("insert vector: {e}")))?;
    }
    conn.execute(
        "INSERT INTO notes(rel, hash) VALUES (?1, ?2)
         ON CONFLICT(rel) DO UPDATE SET hash = ?2",
        rusqlite::params![rel, hash],
    )
    .map_err(|e| CoreError::Message(format!("upsert note: {e}")))?;
    Ok(())
}

/// Re-embed notes whose content changed and drop deleted ones. Blocking.
fn sync_index(vault: &Path, cache_dir: &Path) -> Result<()> {
    let mut current: Vec<(String, String, String)> = Vec::new();
    collect_notes(vault, vault, &mut current);

    // Phase 1: deletions + work list (DB lock, no embedding).
    let to_embed: Vec<(String, String, Vec<String>)> = with_conn(vault, |conn| {
        let present: std::collections::HashSet<&str> =
            current.iter().map(|(r, _, _)| r.as_str()).collect();
        let known: Vec<(String, String)> = conn
            .prepare("SELECT rel, hash FROM notes")
            .and_then(|mut s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            })
            .map_err(|e| CoreError::Message(format!("read notes: {e}")))?;
        let known_map: std::collections::HashMap<&str, &str> = known
            .iter()
            .map(|(r, h)| (r.as_str(), h.as_str()))
            .collect();

        // Deleted notes.
        for (rel, _) in &known {
            if !present.contains(rel.as_str()) {
                delete_note_rows(conn, rel)?;
            }
        }
        // Changed / new notes.
        let mut to_embed = Vec::new();
        for (rel, hash, content) in &current {
            if known_map.get(rel.as_str()) != Some(&hash.as_str()) {
                delete_note_rows(conn, rel)?;
                to_embed.push((rel.clone(), hash.clone(), chunk_note(content)));
            }
        }
        Ok(to_embed)
    })?;

    if to_embed.is_empty() {
        return Ok(());
    }

    // Phase 2: embed (no DB lock).
    let embedded = embed_notes(cache_dir, to_embed)?;
    let dim = embedded
        .iter()
        .find_map(|(_, _, cs)| cs.first().map(|(_, v)| v.len()))
        .unwrap_or(0);

    // Phase 3: insert (DB lock).
    with_conn(vault, |conn| {
        if dim > 0 {
            ensure_vec_table(conn, dim)?;
        }
        for (rel, hash, chunks) in &embedded {
            insert_note_rows(conn, rel, hash, chunks)?;
        }
        Ok(())
    })
}

/// Embed each note's chunks in one model session; returns `(rel, hash, [(text, vec)])`.
fn embed_notes(
    cache_dir: &Path,
    to_embed: Vec<(String, String, Vec<String>)>,
) -> Result<Vec<EmbeddedNote>> {
    if to_embed.is_empty() {
        return Ok(Vec::new());
    }
    let prefixed: Vec<String> = to_embed
        .iter()
        .flat_map(|(_, _, chunks)| chunks.iter().map(|c| format!("{DOC_PREFIX}{c}")))
        .collect();
    let vectors = with_model(cache_dir, |m| embed_texts(m, prefixed))?;
    let mut out = Vec::with_capacity(to_embed.len());
    let mut vi = 0usize;
    for (rel, hash, chunks) in to_embed {
        let mut cs = Vec::with_capacity(chunks.len());
        for text in chunks {
            if vi < vectors.len() {
                cs.push((text, vectors[vi].clone()));
                vi += 1;
            }
        }
        out.push((rel, hash, cs));
    }
    Ok(out)
}

/// Run a semantic search end-to-end (blocking).
fn search_blocking(vault: &Path, cache_dir: &Path, query: &str, top_k: usize) -> Result<Value> {
    sync_index(vault, cache_dir)?;
    let want = top_k.max(1);

    let has_vectors = with_conn(vault, |conn| Ok(stored_dim(conn).is_some()))?;
    if !has_vectors {
        return Ok(json!({ "matches": [] })); // nothing indexed yet
    }

    let qvec = with_model(cache_dir, |m| {
        Ok(embed_texts(m, vec![format!("{QUERY_PREFIX}{query}")])?
            .into_iter()
            .next()
            .unwrap_or_default())
    })?;

    let matches = with_conn(vault, |conn| {
        // KNN over-fetches (want*4) so we can keep the best chunk per note.
        let knn: Vec<(i64, f32)> = conn
            .prepare(
                "SELECT key, distance FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
            )
            .and_then(|mut s| {
                s.query_map(
                    rusqlite::params![vec_bytes(&qvec), (want * 4) as i64],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)? as f32)),
                )
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
            })
            .map_err(|e| CoreError::Message(format!("vector search: {e}")))?;

        // Best (smallest cosine distance) chunk per note.
        let mut best: std::collections::HashMap<String, (f32, String)> =
            std::collections::HashMap::new();
        for (key, dist) in knn {
            let row: rusqlite::Result<(String, String)> =
                conn.query_row("SELECT rel, text FROM chunks WHERE key = ?1", [key], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                });
            if let Ok((rel, text)) = row {
                let entry = best.entry(rel).or_insert((f32::MAX, text.clone()));
                if dist < entry.0 {
                    *entry = (dist, text);
                }
            }
        }
        let mut scored: Vec<(f32, String, String)> = best
            .into_iter()
            .map(|(rel, (dist, text))| {
                let score = 1.0 - dist; // cosine distance -> similarity
                (score, rel, text.chars().take(200).collect())
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(want);
        Ok(scored)
    })?;

    let out: Vec<Value> = matches
        .into_iter()
        .map(|(score, path, snippet)| json!({ "path": path, "score": score, "snippet": snippet }))
        .collect();
    Ok(json!({ "matches": out }))
}

/// Re-embed a single note immediately (blocking). Skips work if its hash is
/// already current; otherwise removes its old rows and inserts fresh chunks.
fn reindex_note_blocking(vault: &Path, cache_dir: &Path, rel: &str, content: &str) -> Result<()> {
    let hash = hash_str(content);
    let unchanged = with_conn(vault, |conn| {
        let cur: Option<String> = conn
            .query_row("SELECT hash FROM notes WHERE rel = ?1", [rel], |r| r.get(0))
            .ok();
        Ok(cur.as_deref() == Some(hash.as_str()))
    })?;
    if unchanged {
        return Ok(());
    }
    let embedded = embed_notes(
        cache_dir,
        vec![(rel.to_string(), hash, chunk_note(content))],
    )?;
    let dim = embedded
        .iter()
        .find_map(|(_, _, cs)| cs.first().map(|(_, v)| v.len()))
        .unwrap_or(0);
    with_conn(vault, |conn| {
        delete_note_rows(conn, rel)?;
        if dim > 0 {
            ensure_vec_table(conn, dim)?;
        }
        for (r, h, chunks) in &embedded {
            insert_note_rows(conn, r, h, chunks)?;
        }
        Ok(())
    })
}

/// Fire-and-forget re-embed of one note after a wiki_write. Best-effort.
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
pub async fn warm(vault: PathBuf, cache_dir: PathBuf) {
    if !enabled() || !vault.is_dir() {
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
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("para three"));
        let big = format!("{}\n\n{}", "x".repeat(1300), "tail");
        assert!(chunk_note(&big).len() >= 2);
    }

    #[test]
    fn hash_is_stable_and_sensitive() {
        assert_eq!(hash_str("abc"), hash_str("abc"));
        assert_ne!(hash_str("abc"), hash_str("abd"));
    }

    #[test]
    fn sqlite_vec_store_roundtrip() {
        // Exercise the on-disk store directly: insert two vectors, the query
        // nearest the second returns it; deleting it leaves the first.
        fleety_vec::register();
        let conn = Connection::open_in_memory().expect("db");
        conn.execute_batch(
            "CREATE TABLE chunks(key INTEGER PRIMARY KEY AUTOINCREMENT, rel TEXT, text TEXT);
             CREATE VIRTUAL TABLE vec_chunks USING vec0(key INTEGER PRIMARY KEY, embedding float[3] distance_metric=cosine);",
        )
        .expect("schema");
        for (rel, v) in [("a.md", [1.0f32, 0.0, 0.0]), ("b.md", [0.0f32, 1.0, 0.0])] {
            conn.execute("INSERT INTO chunks(rel, text) VALUES (?1, ?1)", [rel])
                .expect("chunk");
            let key = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO vec_chunks(key, embedding) VALUES (?1, ?2)",
                rusqlite::params![key, vec_bytes(&v)],
            )
            .expect("vec");
        }
        let q = vec_bytes(&[0.0f32, 0.9, 0.1]);
        // vec0 KNN needs the LIMIT on the MATCH scan itself, then look up the row.
        let nearest: i64 = conn
            .query_row(
                "SELECT key FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                rusqlite::params![q],
                |r| r.get(0),
            )
            .expect("knn");
        let rel: String = conn
            .query_row("SELECT rel FROM chunks WHERE key=?1", [nearest], |r| {
                r.get(0)
            })
            .expect("rel");
        assert_eq!(rel, "b.md");

        // remove the nearest -> the other becomes nearest
        conn.execute("DELETE FROM vec_chunks WHERE key=?1", [nearest])
            .expect("del");
        let next: i64 = conn
            .query_row(
                "SELECT key FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                rusqlite::params![q],
                |r| r.get(0),
            )
            .expect("knn2");
        let rel2: String = conn
            .query_row("SELECT rel FROM chunks WHERE key=?1", [next], |r| r.get(0))
            .expect("rel2");
        assert_eq!(rel2, "a.md");
    }
}
