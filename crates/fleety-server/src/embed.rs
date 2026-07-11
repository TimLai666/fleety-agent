//! Shared local text embedding.
//!
//! One process-global EmbeddingGemma model (fastembed / ONNX, CPU; downloaded
//! once from the ungated `onnx-community/embeddinggemma-300m-ONNX`, offline
//! thereafter), plus the retrieval prefixes and the f32→bytes helper for
//! sqlite-vec storage. Both the wiki index and the conversation index use this,
//! so there is exactly one model in memory and one `FLEETY_WIKI_EMBED` switch.
//!
//! Intel macOS exception: the ort/ONNX runtime ships no prebuilt
//! x86_64-apple-darwin library, so fastembed is compiled out there (see
//! Cargo.toml) and this module swaps in a stub whose `enabled()` is always
//! false — every caller then takes the same path as `FLEETY_WIKI_EMBED=0`,
//! and anything reaching the embed calls anyway gets a clear error.

use std::path::Path;

use agent_core::{CoreError, Result};
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

/// EmbeddingGemma's documented retrieval prompts (improve query/document
/// alignment). Applied manually — fastembed runs the tokenizer on raw text.
pub const QUERY_PREFIX: &str = "task: search result | query: ";
pub const DOC_PREFIX: &str = "title: none | text: ";
/// Q8 (int8) build — the chosen quality/size balance (~300MB).
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
const MODEL: EmbeddingModel = EmbeddingModel::EmbeddingGemma300MQ;
/// Identifies which model an index was built with (stored in each index's meta,
/// so an index built by a different model is rebuilt).
pub const MODEL_TAG: &str = "embeddinggemma-300m-q8";

/// Whether local embedding is enabled (`FLEETY_WIKI_EMBED != "0"`). The single
/// switch for every embedding feature (wiki + conversation recall).
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
pub fn enabled() -> bool {
    std::env::var("FLEETY_WIKI_EMBED").as_deref() != Ok("0")
}

/// Intel macOS: embedding is compiled out, so the single gate is always off.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub fn enabled() -> bool {
    false
}

/// Stand-in for fastembed's `TextEmbedding` on the platform where it cannot
/// exist. Uninhabited — `with_model` never produces one, so the closures the
/// callers pass still type-check but can never run.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub enum TextEmbedding {}

/// Process-global model handle. `embed` needs `&mut self`, so a Mutex, not Arc.
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
fn model_cell() -> &'static std::sync::Mutex<Option<TextEmbedding>> {
    static M: std::sync::OnceLock<std::sync::Mutex<Option<TextEmbedding>>> =
        std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(None))
}

/// f32 vector → little-endian bytes (the form sqlite-vec stores).
pub fn vec_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Initialise the model if needed and run `f` with it. Blocking.
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
pub fn with_model<T>(
    cache_dir: &Path,
    f: impl FnOnce(&mut TextEmbedding) -> Result<T>,
) -> Result<T> {
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
                 search. Cause: {e}"
            ))
        })?;
        *guard = Some(model);
    }
    let model = guard
        .as_mut()
        .ok_or_else(|| CoreError::Message("embedding model unavailable".to_string()))?;
    f(model)
}

/// Intel macOS: no runtime to load — a clear error instead of a model.
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub fn with_model<T>(
    _cache_dir: &Path,
    _f: impl FnOnce(&mut TextEmbedding) -> Result<T>,
) -> Result<T> {
    Err(embedding_unavailable())
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn embedding_unavailable() -> CoreError {
    CoreError::Message(
        "semantic embedding is unavailable on Intel macOS (the ONNX runtime ships no prebuilt \
         x86_64-apple-darwin library); wiki and conversation search fall back to keyword matching"
            .to_string(),
    )
}

/// Embed raw texts (no prefix added).
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
pub fn embed_texts(model: &mut TextEmbedding, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    model
        .embed(texts, None)
        .map_err(|e| CoreError::Message(format!("embedding failed: {e}")))
}

/// Intel macOS: unreachable in practice (`TextEmbedding` is uninhabited).
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub fn embed_texts(model: &mut TextEmbedding, _texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    match *model {}
}

/// Embed documents (DOC_PREFIX applied), loading the model as needed.
pub fn embed_docs(cache_dir: &Path, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let prefixed = texts.iter().map(|t| format!("{DOC_PREFIX}{t}")).collect();
    with_model(cache_dir, |m| embed_texts(m, prefixed))
}

/// Embed one query (QUERY_PREFIX applied), loading the model as needed.
pub fn embed_query(cache_dir: &Path, query: &str) -> Result<Vec<f32>> {
    let mut out = with_model(cache_dir, |m| {
        embed_texts(m, vec![format!("{QUERY_PREFIX}{query}")])
    })?;
    out.pop()
        .ok_or_else(|| CoreError::Message("empty embedding result".to_string()))
}
