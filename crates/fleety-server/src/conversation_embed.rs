//! Per-user conversation vector index (sqlite-vec), powering real semantic
//! `conversation_semantic_search`. One embedding per message, stored in a small
//! SQLite db under the user's own directory (so it inherits the privacy
//! boundary). The storage/query layer takes precomputed vectors and is
//! unit-testable without the model; the embedding step reuses the shared local
//! model in [`crate::embed`]. Missing/failed embedding falls back to keyword
//! search at the call site — this index is purely an optimization.

use std::path::{Path, PathBuf};

use agent_core::{CoreError, Result};
use rusqlite::Connection;

use crate::embed;

/// A message hit from the index: which conversation/message it is, plus the
/// cosine similarity score.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    pub conversation_id: String,
    pub seq: u64,
    pub ts_secs: u64,
    pub role: String,
    pub snippet: String,
    pub score: f32,
}

/// The index db path for a user (beside their conversations).
pub fn index_path(home: &Path, user: &str) -> PathBuf {
    home.join("fleet")
        .join("users")
        .join(user)
        .join("conversations")
        .join(".index")
        .join("conversations.db")
}

// ---- storage/query layer (no model; unit-testable with synthetic vectors) ----

/// Open (creating if needed) the index db: register sqlite-vec, create the base
/// schema, and reset everything if it was built with a different model.
pub fn open_index(path: &Path) -> Result<Connection> {
    fleety_vec::register();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("create index dir: {e}")))?;
    }
    let conn = Connection::open(path)
        .map_err(|e| CoreError::Message(format!("open conversation index db: {e}")))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta(id INTEGER PRIMARY KEY CHECK (id = 0), model TEXT, dim INTEGER);
         CREATE TABLE IF NOT EXISTS chunks(key INTEGER PRIMARY KEY AUTOINCREMENT, conversation_id TEXT NOT NULL, seq INTEGER NOT NULL, ts_secs INTEGER NOT NULL, role TEXT NOT NULL, snippet TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS chunks_conv ON chunks(conversation_id);",
    )
    .map_err(|e| CoreError::Message(format!("init conversation index schema: {e}")))?;

    // A model change invalidates every vector; wipe and start fresh.
    let stored: Option<String> = conn
        .query_row("SELECT model FROM meta WHERE id = 0", [], |r| r.get(0))
        .ok();
    if stored.as_deref() != Some(embed::MODEL_TAG) {
        conn.execute_batch("DROP TABLE IF EXISTS vec_chunks; DELETE FROM chunks;")
            .map_err(|e| CoreError::Message(format!("reset conversation index: {e}")))?;
        conn.execute(
            "INSERT INTO meta(id, model, dim) VALUES (0, ?1, NULL)
             ON CONFLICT(id) DO UPDATE SET model = ?1, dim = NULL",
            [embed::MODEL_TAG],
        )
        .map_err(|e| CoreError::Message(format!("set model tag: {e}")))?;
    }
    Ok(conn)
}

/// Whether a user's index has no chunks yet (missing db or empty). Used to
/// trigger a one-time lazy backfill of pre-existing conversations on first search.
pub fn is_empty(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    match open_index(path) {
        Ok(conn) => conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))
            .map(|n| n == 0)
            .unwrap_or(true),
        Err(_) => true,
    }
}

fn stored_dim(conn: &Connection) -> Option<usize> {
    conn.query_row("SELECT dim FROM meta WHERE id = 0", [], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .ok()
    .flatten()
    .map(|d| d as usize)
}

/// Create the `vec_chunks` table at `dim` (idempotent) and record the dim.
pub fn ensure_dim(conn: &Connection, dim: usize) -> Result<()> {
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

/// Highest message `seq` already indexed for a conversation (0 if none) — so
/// indexing only appends genuinely new messages.
pub fn max_indexed_seq(conn: &Connection, conversation_id: &str) -> u64 {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM chunks WHERE conversation_id = ?1",
        [conversation_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as u64)
    .unwrap_or(0)
}

/// Insert one message's metadata + its precomputed embedding.
pub fn insert_chunk(
    conn: &Connection,
    conversation_id: &str,
    seq: u64,
    ts_secs: u64,
    role: &str,
    snippet: &str,
    embedding: &[f32],
) -> Result<()> {
    conn.execute(
        "INSERT INTO chunks(conversation_id, seq, ts_secs, role, snippet) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![conversation_id, seq as i64, ts_secs as i64, role, snippet],
    )
    .map_err(|e| CoreError::Message(format!("insert chunk: {e}")))?;
    let key = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO vec_chunks(key, embedding) VALUES (?1, ?2)",
        rusqlite::params![key, embed::vec_bytes(embedding)],
    )
    .map_err(|e| CoreError::Message(format!("insert vector: {e}")))?;
    Ok(())
}

/// K-nearest messages to `query_vec` by cosine similarity (highest first). Score
/// is `1 - distance`. Empty if the vector table doesn't exist yet.
pub fn knn(conn: &Connection, query_vec: &[f32], limit: usize) -> Result<Vec<ChunkHit>> {
    if stored_dim(conn).is_none() {
        return Ok(Vec::new());
    }
    // vec0 KNN: LIMIT must be on the MATCH scan; resolve metadata in a second step.
    let mut stmt = conn
        .prepare("SELECT key, distance FROM vec_chunks WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2")
        .map_err(|e| CoreError::Message(format!("prepare knn: {e}")))?;
    let rows = stmt
        .query_map(
            rusqlite::params![embed::vec_bytes(query_vec), limit as i64],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)),
        )
        .map_err(|e| CoreError::Message(format!("run knn: {e}")))?;
    let mut keyed: Vec<(i64, f32)> = Vec::new();
    for row in rows {
        let (key, dist) = row.map_err(|e| CoreError::Message(format!("knn row: {e}")))?;
        keyed.push((key, 1.0 - dist as f32));
    }
    let mut hits = Vec::with_capacity(keyed.len());
    for (key, score) in keyed {
        let hit = conn.query_row(
            "SELECT conversation_id, seq, ts_secs, role, snippet FROM chunks WHERE key = ?1",
            [key],
            |r| {
                Ok(ChunkHit {
                    conversation_id: r.get(0)?,
                    seq: r.get::<_, i64>(1)? as u64,
                    ts_secs: r.get::<_, i64>(2)? as u64,
                    role: r.get(3)?,
                    snippet: r.get(4)?,
                    score,
                })
            },
        );
        if let Ok(hit) = hit {
            hits.push(hit);
        }
    }
    Ok(hits)
}

// ---- model-backed operations (embedding step; live ranking is manual-verify) ----

/// One message to index: its sequence, timestamp, role, and text.
pub type IndexMsg = (u64, u64, String, String);

/// Embed and append a conversation's new messages (those past the current
/// watermark) to the user's index. Best-effort: returns the count added.
pub fn index_messages(
    path: &Path,
    cache_dir: &Path,
    conversation_id: &str,
    messages: &[IndexMsg],
) -> Result<usize> {
    if messages.is_empty() {
        return Ok(0);
    }
    let conn = open_index(path)?;
    let watermark = max_indexed_seq(&conn, conversation_id);
    let fresh: Vec<&IndexMsg> = messages
        .iter()
        .filter(|(seq, ..)| *seq > watermark)
        .collect();
    if fresh.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = fresh.iter().map(|(_, _, _, text)| text.clone()).collect();
    let vectors = embed::embed_docs(cache_dir, &texts)?;
    let dim = vectors.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 {
        return Ok(0);
    }
    ensure_dim(&conn, dim)?;
    for ((seq, ts, role, snippet), vec) in fresh.iter().zip(vectors.iter()) {
        insert_chunk(&conn, conversation_id, *seq, *ts, role, snippet, vec)?;
    }
    Ok(fresh.len())
}

/// Embed `query` and return the user's most similar messages.
pub fn search(path: &Path, cache_dir: &Path, query: &str, limit: usize) -> Result<Vec<ChunkHit>> {
    let conn = open_index(path)?;
    let qvec = embed::embed_query(cache_dir, query)?;
    knn(&conn, &qvec, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-convidx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir.join("conversations.db")
    }

    #[test]
    fn knn_returns_nearest_in_cosine_order() {
        let path = temp_db();
        let conn = open_index(&path).expect("open");
        ensure_dim(&conn, 3).expect("dim");
        // Three messages pointing in different directions.
        insert_chunk(&conn, "c1", 1, 100, "user", "x-axis", &[1.0, 0.0, 0.0]).expect("i1");
        insert_chunk(&conn, "c1", 2, 200, "assistant", "y-axis", &[0.0, 1.0, 0.0]).expect("i2");
        insert_chunk(&conn, "c1", 3, 300, "user", "x-ish", &[0.9, 0.1, 0.0]).expect("i3");

        // Query close to the x-axis → "x-axis" and "x-ish" rank first.
        let hits = knn(&conn, &[1.0, 0.05, 0.0], 2).expect("knn");
        assert_eq!(hits.len(), 2);
        assert!(hits[0].snippet == "x-axis" || hits[0].snippet == "x-ish");
        assert!(hits[1].snippet == "x-axis" || hits[1].snippet == "x-ish");
        // Scores are similarities (higher = closer), descending.
        assert!(hits[0].score >= hits[1].score);
        assert!(hits[0].score > 0.9);
        // Metadata round-trips.
        assert_eq!(hits[0].conversation_id, "c1");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn empty_index_yields_no_hits_and_dim_roundtrips() {
        let path = temp_db();
        let conn = open_index(&path).expect("open");
        // No vec table yet → no hits, no error.
        assert!(knn(&conn, &[1.0, 0.0, 0.0], 5).expect("knn").is_empty());
        ensure_dim(&conn, 4).expect("dim");
        assert_eq!(stored_dim(&conn), Some(4));
        // Watermark starts at 0.
        assert_eq!(max_indexed_seq(&conn, "c1"), 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn watermark_tracks_indexed_messages() {
        let path = temp_db();
        let conn = open_index(&path).expect("open");
        ensure_dim(&conn, 2).expect("dim");
        insert_chunk(&conn, "c1", 5, 10, "user", "hi", &[1.0, 0.0]).expect("i");
        insert_chunk(&conn, "c1", 9, 20, "assistant", "yo", &[0.0, 1.0]).expect("i");
        assert_eq!(max_indexed_seq(&conn, "c1"), 9);
        assert_eq!(max_indexed_seq(&conn, "other"), 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn is_empty_reflects_index_state() {
        let path = temp_db();
        // No db file yet → empty.
        assert!(is_empty(&path));
        let conn = open_index(&path).expect("open");
        ensure_dim(&conn, 2).expect("dim");
        // Created but no chunks → still empty.
        assert!(is_empty(&path));
        insert_chunk(&conn, "c1", 1, 10, "user", "hi", &[1.0, 0.0]).expect("i");
        drop(conn);
        // Has a chunk → not empty.
        assert!(!is_empty(&path));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn index_path_is_under_user_dir() {
        let p = index_path(Path::new("/home/.fleety"), "alice");
        assert!(p.ends_with("fleet/users/alice/conversations/.index/conversations.db"));
    }
}
