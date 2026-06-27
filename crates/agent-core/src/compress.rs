//! Context compression — a clean-room Rust take on the techniques from
//! [headroom](https://github.com/headroomlabs-ai/headroom), built into the
//! agent core (not an external proxy/sidecar). The goal is to shrink what
//! reaches the model — large tool results especially — while preserving meaning
//! and staying **reversible**.
//!
//! Engines (this module):
//! - [`SmartCrusher`] — structural compression of JSON tool results: head/tail-
//!   trim long arrays, truncate long strings, optionally drop empty fields,
//!   bound depth. Small results pass through unchanged.
//! - [`CacheAligner`] — order prompt segments most-stable-first so a change low
//!   in the prompt doesn't bust the KV-cache prefix above it.
//! - **CCR (reversible compression)** — not a separate type: the invariant is
//!   that the *full* original is always kept in the event log, so any
//!   compression here is recoverable (see [`compress_tool_result`] and the
//!   budget marker). The event stream is the source of truth.
//!
//! Tool-output budgeting + context-window compaction (see [`crate::agent`]) are
//! the other two headroom-style pieces already in the loop. Still to come (kept
//! honest, not stubbed): an AST `CodeCompressor` (tree-sitter) and an optional
//! ML prose compressor — the latter blocked on a freely-available model.

use serde_json::{Map, Value};

/// A pluggable context compressor: shrink content before it reaches the model
/// while preserving meaning. Reversibility is guaranteed by the caller keeping
/// the original in the event log, not by the compressor.
pub trait ContextCompressor: Send + Sync {
    /// Compress `text`. Implementations that only understand a specific shape
    /// (e.g. JSON) should return `text` unchanged when it doesn't apply.
    fn compress(&self, text: &str) -> String;
}

/// Structural JSON compressor. Defaults are generous so ordinary small tool
/// results are returned byte-for-byte; only genuinely large structures shrink.
#[derive(Debug, Clone)]
pub struct SmartCrusher {
    /// Arrays longer than this are trimmed to `head` + a marker + `tail`.
    pub max_array: usize,
    pub head: usize,
    pub tail: usize,
    /// Strings longer than this (chars) are truncated with a `+N chars` marker.
    pub max_string: usize,
    /// Drop object fields whose value is null / "" / [] / {}.
    pub drop_empty: bool,
    /// Collapse anything deeper than this to a placeholder.
    pub max_depth: usize,
}

impl Default for SmartCrusher {
    fn default() -> Self {
        Self {
            max_array: 50,
            head: 20,
            tail: 5,
            max_string: 4000,
            drop_empty: false,
            max_depth: 24,
        }
    }
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

impl SmartCrusher {
    /// Crush a JSON value structurally.
    pub fn crush(&self, value: &Value) -> Value {
        self.crush_at(value, 0)
    }

    fn crush_at(&self, value: &Value, depth: usize) -> Value {
        if depth > self.max_depth {
            return Value::String("…(depth limit)".to_string());
        }
        match value {
            Value::String(s) => {
                let n = s.chars().count();
                if n > self.max_string {
                    let kept: String = s.chars().take(self.max_string).collect();
                    Value::String(format!("{kept}…(+{} chars)", n - self.max_string))
                } else {
                    value.clone()
                }
            }
            Value::Array(items) => {
                if items.len() > self.max_array {
                    let mut out: Vec<Value> = Vec::with_capacity(self.head + self.tail + 1);
                    for it in items.iter().take(self.head) {
                        out.push(self.crush_at(it, depth + 1));
                    }
                    let omitted = items.len() - self.head - self.tail;
                    out.push(Value::String(format!("…{omitted} more items")));
                    for it in items.iter().skip(items.len() - self.tail) {
                        out.push(self.crush_at(it, depth + 1));
                    }
                    Value::Array(out)
                } else {
                    Value::Array(
                        items
                            .iter()
                            .map(|it| self.crush_at(it, depth + 1))
                            .collect(),
                    )
                }
            }
            Value::Object(map) => {
                let mut out = Map::new();
                for (k, v) in map {
                    let cv = self.crush_at(v, depth + 1);
                    if self.drop_empty && is_empty(&cv) {
                        continue;
                    }
                    out.insert(k.clone(), cv);
                }
                Value::Object(out)
            }
            _ => value.clone(),
        }
    }
}

impl ContextCompressor for SmartCrusher {
    fn compress(&self, text: &str) -> String {
        match serde_json::from_str::<Value>(text) {
            Ok(v) => self.crush(&v).to_string(),
            Err(_) => text.to_string(),
        }
    }
}

/// Truncate `text` to at most `max_chars`, appending a marker noting how much
/// was omitted. The full text lives in the event log, so this is reversible
/// (the CCR property).
pub(crate) fn budget_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    let omitted = text.chars().count() - max_chars;
    format!("{kept}\n... [truncated {omitted} chars; full result retained in the event log]")
}

/// Compress a tool result for the model: structurally crush the JSON, then
/// enforce the character budget. The caller keeps the full result in the event
/// log (reversible — CCR).
pub(crate) fn compress_tool_result(value: &Value, max_chars: usize) -> String {
    let crushed = SmartCrusher::default().crush(value).to_string();
    budget_text(&crushed, max_chars)
}

/// CacheAligner: assemble a prompt from labelled segments ordered **most stable
/// first**, so editing a late (volatile) segment doesn't invalidate the
/// KV-cache prefix built from the earlier (stable) ones. A KV cache is valid up
/// to the first changed token, so stable-first maximises reuse.
///
/// The server already follows this when it builds the system prompt
/// (protocol → rules → memory → policy → core memory); this helper makes the
/// ordering explicit and reusable.
pub struct CacheAligner;

impl CacheAligner {
    /// Join `segments` (already in stable→volatile order) with separators.
    pub fn assemble(segments: &[&str]) -> String {
        segments
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn small_json_passes_through_unchanged() {
        let v = json!({ "a": 1, "b": ["x", "y"], "c": "hello" });
        // Crushed value serializes identically (same serde_json ordering).
        assert_eq!(SmartCrusher::default().crush(&v), v);
        assert_eq!(
            SmartCrusher::default().compress(&v.to_string()),
            v.to_string()
        );
    }

    #[test]
    fn long_array_is_head_tail_trimmed() {
        let big: Vec<Value> = (0..200).map(|i| json!(i)).collect();
        let v = json!({ "items": big });
        let crushed = SmartCrusher::default().crush(&v);
        let arr = crushed["items"].as_array().expect("arr");
        // 20 head + 1 marker + 5 tail
        assert_eq!(arr.len(), 26);
        assert_eq!(arr[0], json!(0));
        assert_eq!(arr[20], json!("…175 more items"));
        assert_eq!(arr[25], json!(199));
    }

    #[test]
    fn long_string_is_truncated() {
        let s = "x".repeat(5000);
        let crushed = SmartCrusher::default().crush(&json!(s));
        let out = crushed.as_str().expect("str");
        assert!(out.ends_with("…(+1000 chars)"));
        assert!(out.chars().count() < 5000);
    }

    #[test]
    fn drop_empty_when_enabled() {
        let v = json!({ "keep": 1, "gone": null, "empty": "", "arr": [] });
        let c = SmartCrusher {
            drop_empty: true,
            ..Default::default()
        };
        let crushed = c.crush(&v);
        let obj = crushed.as_object().expect("obj");
        assert!(obj.contains_key("keep"));
        assert!(!obj.contains_key("gone"));
        assert!(!obj.contains_key("empty"));
        assert!(!obj.contains_key("arr"));
    }

    #[test]
    fn non_json_is_left_for_budgeting() {
        assert_eq!(SmartCrusher::default().compress("just prose"), "just prose");
    }

    #[test]
    fn cache_aligner_orders_and_skips_empty() {
        assert_eq!(CacheAligner::assemble(&["a", "", "b"]), "a\n\n---\n\nb");
    }

    #[test]
    fn budget_marks_truncation() {
        let out = budget_text(&"a".repeat(100), 10);
        assert!(out.contains("truncated 90 chars"));
    }
}
