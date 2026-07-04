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

/// The default per-string truncation threshold: strings longer than this are
/// head/tail-trimmed. Exposed as a single source so retrieval
/// (`fetch_tool_result`) can cap a page to a size that survives being fed back
/// through compression without being re-truncated.
pub const DEFAULT_MAX_STRING: usize = 4000;

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
            max_string: DEFAULT_MAX_STRING,
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
        self.crush_tracked(value).0
    }

    /// Crush a value and report whether any *content* was truncated (a long
    /// string or a long array). Dropping empty fields does not count — nothing
    /// retrievable is lost. Callers use the flag to attach a fetch-id marker so
    /// truncated content stays locatable even when the whole result fits the
    /// character budget.
    pub fn crush_tracked(&self, value: &Value) -> (Value, bool) {
        let mut truncated = false;
        let crushed = self.crush_at(value, 0, &mut truncated);
        (crushed, truncated)
    }

    fn crush_at(&self, value: &Value, depth: usize, truncated: &mut bool) -> Value {
        if depth > self.max_depth {
            return Value::String("…(depth limit)".to_string());
        }
        match value {
            Value::String(s) => {
                let n = s.chars().count();
                if n > self.max_string {
                    *truncated = true;
                    // Keep a head and a tail (like long arrays), so the end of a
                    // long single-string output — where errors/summaries often
                    // sit — stays visible. Head ~3/4, tail ~1/4; char-wise slices
                    // are UTF-8 safe.
                    let head_len = self.max_string * 3 / 4;
                    let tail_len = self.max_string - head_len;
                    let omitted = n - head_len - tail_len;
                    let head: String = s.chars().take(head_len).collect();
                    let tail: String = s.chars().skip(n - tail_len).collect();
                    Value::String(format!("{head}…(+{omitted} chars omitted){tail}"))
                } else {
                    value.clone()
                }
            }
            Value::Array(items) => {
                if items.len() > self.max_array {
                    *truncated = true;
                    let mut out: Vec<Value> = Vec::with_capacity(self.head + self.tail + 1);
                    for it in items.iter().take(self.head) {
                        out.push(self.crush_at(it, depth + 1, truncated));
                    }
                    let omitted = items.len() - self.head - self.tail;
                    out.push(Value::String(format!("…{omitted} more items")));
                    for it in items.iter().skip(items.len() - self.tail) {
                        out.push(self.crush_at(it, depth + 1, truncated));
                    }
                    Value::Array(out)
                } else {
                    Value::Array(
                        items
                            .iter()
                            .map(|it| self.crush_at(it, depth + 1, truncated))
                            .collect(),
                    )
                }
            }
            Value::Object(map) => {
                let mut out = Map::new();
                for (k, v) in map {
                    let cv = self.crush_at(v, depth + 1, truncated);
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
pub(crate) fn budget_text(text: &str, max_chars: usize, id: Option<&str>) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars).collect();
    let omitted = text.chars().count() - max_chars;
    match id {
        // The full result lives in the event log keyed by this id; the marker
        // names exactly how to retrieve it (in bounded segments).
        Some(id) => format!(
            "{kept}\n... [truncated {omitted} chars; fetch the full result with fetch_tool_result id=\"{id}\"]"
        ),
        None => {
            format!("{kept}\n... [truncated {omitted} chars; full result retained in the event log]")
        }
    }
}

/// Compress a tool result for the model: structurally crush the JSON, then
/// enforce the character budget. The caller keeps the full result in the event
/// log keyed by `id` (reversible — CCR). Whenever anything is dropped — either
/// the structural crush truncated content, or the budget truncated the text —
/// the output names `id` so the agent can fetch the full result. A result that
/// fits the budget and lost nothing is returned unchanged, with no marker.
pub(crate) fn compress_tool_result(value: &Value, max_chars: usize, id: &str) -> String {
    let (crushed, truncated) = SmartCrusher::default().crush_tracked(value);
    let text = crushed.to_string();
    if text.chars().count() > max_chars {
        // Over budget: budget_text truncates and names the fetch id.
        return budget_text(&text, max_chars, Some(id));
    }
    if truncated {
        // Within budget, but the structural crush dropped array items / string
        // chars. Attach the fetch id so that content is still retrievable.
        return format!(
            "{text}\n[compressed; fetch the full result with fetch_tool_result id=\"{id}\"]"
        );
    }
    // Nothing dropped and within budget — untouched, no marker.
    text
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
        // Head+tail retained with an omission marker (not head-only).
        assert!(out.contains("chars omitted"));
        assert!(out.chars().count() < 5000);
    }

    #[test]
    fn long_string_keeps_head_and_tail() {
        // Spec example: 10000 identical head chars followed by a distinct
        // 4-char tail, threshold 4000. Both ends must survive.
        let s = format!("{}WXYZ", "A".repeat(10000));
        let crushed = SmartCrusher::default().crush(&json!(s));
        let out = crushed.as_str().expect("str");
        assert!(out.starts_with("AAAA"), "head retained");
        assert!(out.ends_with("WXYZ"), "tail retained");
        assert!(out.contains("omitted"), "omission marker present");
        assert!(
            out.chars().count() < s.chars().count(),
            "shorter than the original"
        );
    }

    #[test]
    fn short_string_unchanged() {
        let s = "x".repeat(100);
        let crushed = SmartCrusher::default().crush(&json!(s));
        assert_eq!(crushed, json!(s));
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
        let out = budget_text(&"a".repeat(100), 10, None);
        assert!(out.contains("truncated 90 chars"));
        assert!(out.contains("retained in the event log"));
    }

    #[test]
    fn budget_with_id_names_fetch() {
        let out = budget_text(&"a".repeat(100), 10, Some("call_7"));
        assert!(out.contains("truncated 90 chars"));
        assert!(out.contains("fetch_tool_result id=\"call_7\""));
    }

    #[test]
    fn compress_tool_result_carries_id() {
        let big = serde_json::json!("a".repeat(100));
        let out = compress_tool_result(&big, 10, "abc");
        assert!(out.contains("fetch_tool_result id=\"abc\""));
    }

    #[test]
    fn small_result_within_budget_is_untouched_no_marker() {
        let v = json!({ "a": 1, "b": ["x", "y"], "c": "hello" });
        let out = compress_tool_result(&v, 8000, "id1");
        // Returned unchanged, no fetch marker.
        assert_eq!(out, v.to_string());
        assert!(!out.contains("fetch_tool_result"));
    }

    #[test]
    fn within_budget_but_crushed_result_names_fetch_id() {
        // A small-in-chars result whose inner array exceeds max_array (50): the
        // crush drops items even though the whole thing fits the char budget, so
        // the fetch id must be attached so the dropped items stay retrievable.
        let big: Vec<Value> = (0..60).map(|i| json!(i)).collect();
        let v = json!({ "items": big });
        let out = compress_tool_result(&v, 8000, "call_9");
        assert!(out.chars().count() <= 8000, "the crushed result fits the budget");
        assert!(out.contains("more items"), "content was actually trimmed");
        assert!(
            out.contains("fetch_tool_result id=\"call_9\""),
            "trimmed-but-within-budget results must still name the fetch id"
        );
    }

    #[test]
    fn fetch_page_survives_compression() {
        // A fetched page is capped to the string threshold, so feeding it back
        // through compression neither re-truncates its content nor grows a
        // self-referential fetch marker.
        let page = json!({ "content": "a".repeat(DEFAULT_MAX_STRING), "total_chars": 9999 });
        let out = compress_tool_result(&page, 8000, "call_x");
        assert!(!out.contains("chars omitted"), "content not re-truncated");
        assert!(
            !out.contains("fetch_tool_result"),
            "no self-referential fetch marker"
        );
    }

    #[test]
    fn dropping_only_empty_fields_is_not_marked() {
        // drop_empty removes empties losslessly — that is not a truncation, so no
        // fetch marker should be attached.
        let (v, truncated) = SmartCrusher::default().crush_tracked(&json!({ "keep": 1, "empty": [] }));
        assert!(!truncated, "dropping empty fields is lossless");
        let _ = v;
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
