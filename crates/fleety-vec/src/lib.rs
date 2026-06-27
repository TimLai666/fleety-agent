//! The single place in the workspace that needs `unsafe`: registering the
//! [sqlite-vec](https://github.com/asg017/sqlite-vec) extension so every SQLite
//! connection opened afterwards exposes the `vec0` virtual table. The rest of
//! the workspace keeps `#![forbid(unsafe_code)]`; this crate isolates and
//! audits the one unavoidable FFI call.
//!
//! Registration is process-wide and idempotent (guarded by `Once`). Call
//! [`register`] before opening the connections you'll run vector queries on,
//! then use plain `rusqlite` from safe code.
#![warn(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::sync::Once;

use rusqlite::ffi::sqlite3_auto_extension;
use sqlite_vec::sqlite3_vec_init;

/// Register sqlite-vec as a SQLite auto-extension for the whole process. Safe to
/// call repeatedly; only the first call does anything.
#[allow(clippy::missing_transmute_annotations)]
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is sqlite-vec's standard extension entry
        // point; `sqlite3_auto_extension` takes an
        // `Option<unsafe extern "C" fn(*mut sqlite3, *mut *mut i8, *const sqlite3_api_routines) -> i32>`.
        // The transmute reinterprets the init fn to that signature (inferred
        // from the call site), exactly as sqlite-vec's Rust example documents.
        // It just installs the extension for connections opened afterwards.
        unsafe {
            sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn vec0_table_available_after_register() {
        register();
        register(); // idempotent
        let conn = Connection::open_in_memory().expect("db");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE v USING vec0(key INTEGER PRIMARY KEY, embedding float[2] distance_metric=cosine);",
        )
        .expect("vec0 table");
        let bytes: Vec<u8> = [1.0f32, 0.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        conn.execute("INSERT INTO v(key, embedding) VALUES (1, ?1)", [bytes])
            .expect("insert");
        let q: Vec<u8> = [0.9f32, 0.1].iter().flat_map(|x| x.to_le_bytes()).collect();
        let key: i64 = conn
            .query_row(
                "SELECT key FROM v WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                [q],
                |r| r.get(0),
            )
            .expect("knn");
        assert_eq!(key, 1);
    }
}
