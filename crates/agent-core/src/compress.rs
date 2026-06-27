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
//! - [`CodeCompressor`] — AST-aware (tree-sitter): fold function bodies, keep
//!   the structure (signatures / imports / type decls). Rust, Python,
//!   JavaScript, TypeScript, Go.
//!
//! Tool-output budgeting + context-window compaction (see [`crate::agent`]) are
//! the other two headroom-style pieces already in the loop. The one remaining
//! headroom engine — an ML *prose* compressor (their Kompress model) — is kept
//! honest as not-done: that model is gated/unavailable on HuggingFace, so it
//! can't ship until a usable model exists. Prose is meanwhile compacted by the
//! LLM in [`crate::agent::run_turn`]'s context compaction.

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

/// A source language CodeCompressor can parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
}

impl Lang {
    fn language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    /// Guess the language from a file path / extension (case-insensitive).
    pub fn from_path(path: &str) -> Option<Lang> {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
            "ts" | "tsx" => Lang::TypeScript,
            "go" => Lang::Go,
            _ => return None,
        })
    }
}

/// AST-aware code compressor: parse with tree-sitter and **fold function bodies**
/// (replace them with `{ … }` / `…`), keeping the structure — signatures,
/// imports, type/struct/class declarations, comments. The agent still sees what
/// a file *contains* and how it's shaped, at a fraction of the tokens. Reversible
/// like the rest: the full source stays in the event log.
///
/// Standalone (it needs a language, so it isn't a blanket [`ContextCompressor`]):
/// a caller that knows the language — e.g. a file read keyed by extension via
/// [`Lang::from_path`] — folds large sources before they reach the model.
#[derive(Debug, Clone)]
pub struct CodeCompressor {
    /// Only fold bodies at least this many bytes (tiny fns stay inline).
    pub min_body_bytes: usize,
}

impl Default for CodeCompressor {
    fn default() -> Self {
        Self {
            min_body_bytes: 120,
        }
    }
}

impl CodeCompressor {
    /// Fold function bodies in `src`. Returns `src` unchanged if it can't parse.
    pub fn fold(&self, src: &str, lang: Lang) -> String {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&lang.language()).is_err() {
            return src.to_string();
        }
        let Some(tree) = parser.parse(src, None) else {
            return src.to_string();
        };
        let mut spans: Vec<(usize, usize)> = Vec::new();
        collect_bodies(tree.root_node(), self.min_body_bytes, &mut spans);
        if spans.is_empty() {
            return src.to_string();
        }
        spans.sort_by_key(|s| s.0);
        let mut out = String::with_capacity(src.len());
        let mut pos = 0;
        for (s, e) in spans {
            if s < pos {
                continue; // nested/overlapping (shouldn't happen — bodies aren't recursed)
            }
            out.push_str(&src[pos..s]);
            out.push_str(if src[s..e].starts_with('{') {
                "{ … }"
            } else {
                "…"
            });
            pos = e;
        }
        out.push_str(&src[pos..]);
        out
    }
}

/// Record the byte spans of function bodies (≥ `min` bytes); don't descend into
/// a folded body (its inner functions vanish with it).
fn collect_bodies(node: tree_sitter::Node, min: usize, out: &mut Vec<(usize, usize)>) {
    let kind = node.kind();
    if matches!(kind, "block" | "statement_block") {
        if let Some(parent) = node.parent() {
            let pk = parent.kind();
            let is_fn = pk.contains("function")
                || pk.contains("method")
                || pk == "closure_expression"
                || pk == "arrow_function";
            if is_fn && node.end_byte() - node.start_byte() >= min {
                out.push((node.start_byte(), node.end_byte()));
                return;
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_bodies(child, min, out);
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

    #[test]
    fn code_compressor_folds_rust_body_keeps_signature() {
        // Body must exceed the default 120-byte fold threshold.
        let src = "fn add(a: i32, b: i32) -> i32 {\n    let x = a + b; // a comfortably long body so it folds\n    let y = x * 2 + 7;\n    let z = y - a - b - x;\n    z + y - 1\n}\n";
        let out = CodeCompressor::default().fold(src, Lang::Rust);
        assert!(out.contains("fn add(a: i32, b: i32) -> i32"));
        assert!(out.contains("{ … }"));
        assert!(!out.contains("let y = x * 2"));
    }

    #[test]
    fn code_compressor_folds_python_body() {
        let src = "def greet(name):\n    msg = 'hello ' + name + ' welcome to a comfortably long body here'\n    extra = msg.upper() + ' / ' + msg.lower()\n    print(extra)\n    return msg\n";
        let out = CodeCompressor::default().fold(src, Lang::Python);
        assert!(out.contains("def greet(name):"));
        assert!(out.contains('…'));
        assert!(!out.contains("print(extra)"));
    }

    #[test]
    fn code_compressor_leaves_tiny_and_unparseable() {
        // tiny body stays inline
        let tiny = "fn one() -> i32 { 1 }\n";
        assert_eq!(CodeCompressor::default().fold(tiny, Lang::Rust), tiny);
        // not valid rust -> returned unchanged (best-effort)
        let prose = "this is not code at all";
        assert_eq!(CodeCompressor::default().fold(prose, Lang::Rust), prose);
    }

    #[test]
    fn lang_from_path() {
        assert_eq!(Lang::from_path("src/x.rs"), Some(Lang::Rust));
        assert_eq!(Lang::from_path("a/b.PY"), Some(Lang::Python));
        assert_eq!(Lang::from_path("x.tsx"), Some(Lang::TypeScript));
        assert_eq!(Lang::from_path("main.go"), Some(Lang::Go));
        assert_eq!(Lang::from_path("notes.md"), None);
    }
}
