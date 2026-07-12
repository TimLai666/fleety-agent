//! Fetch a provider's model list from its OpenAI-compatible `/models` endpoint,
//! so the config wizard can offer the models to pick from instead of making the
//! user type an id. Parsing is pure (unit-tested); the fetch is best-effort and
//! its failure degrades the wizard to manual entry rather than erroring.

use agent_core::{CoreError, Result};
use serde_json::Value;

/// Extract model ids from a `/models` response body: the standard OpenAI shape
/// `{ "data": [ { "id": "…" }, … ] }`. Tolerates a bare array or a top-level
/// `models` array too. Returns them de-duplicated in first-seen order; a body
/// that isn't recognisable yields an empty list (the caller falls back to manual
/// entry). Pure.
pub fn parse_model_ids(v: &Value) -> Vec<String> {
    let arr = v
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| v.get("models").and_then(Value::as_array))
        .or_else(|| v.as_array());
    let mut out: Vec<String> = Vec::new();
    if let Some(items) = arr {
        for item in items {
            // Each item is usually an object with `id`; some list endpoints
            // return bare strings.
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| item.as_str());
            if let Some(id) = id {
                let id = id.trim();
                if !id.is_empty() && !out.iter().any(|e| e == id) {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
}

/// Fetch `{base_url}/models` and return the model ids. `key` (when present) is
/// sent as a Bearer token. A non-2xx, a non-JSON body, or a network error is an
/// `Err` the caller turns into "type the model id manually".
pub async fn fetch_models(base_url: &str, key: Option<&str>) -> Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut req = reqwest::Client::new().get(&url);
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("fetching {url} failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CoreError::Provider(format!(
            "{url} returned HTTP {}",
            status.as_u16()
        )));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("reading {url} failed: {e}")))?;
    let v: Value = serde_json::from_str(&body)
        .map_err(|e| CoreError::Provider(format!("{url}: response was not JSON ({e})")))?;
    Ok(parse_model_ids(&v))
}

/// Case-insensitive substring filter over model ids, keeping order.
pub fn filter_models<'a>(models: &'a [String], needle: &str) -> Vec<&'a String> {
    let n = needle.trim().to_ascii_lowercase();
    models
        .iter()
        .filter(|m| n.is_empty() || m.to_ascii_lowercase().contains(&n))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_ids_from_openai_data_shape() {
        let v = json!({ "data": [ { "id": "gpt-4o" }, { "id": "gpt-4o-mini" } ] });
        assert_eq!(parse_model_ids(&v), vec!["gpt-4o", "gpt-4o-mini"]);
    }

    #[test]
    fn parse_ids_tolerates_bare_array_and_strings_and_dedups() {
        assert_eq!(
            parse_model_ids(&json!([{ "id": "a" }, { "id": "a" }, "b"])),
            vec!["a", "b"]
        );
        assert_eq!(
            parse_model_ids(&json!({ "models": ["x", "y"] })),
            vec!["x", "y"]
        );
    }

    #[test]
    fn unrecognisable_body_yields_empty() {
        assert!(parse_model_ids(&json!({ "error": "nope" })).is_empty());
        assert!(parse_model_ids(&json!("plain string")).is_empty());
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        let ms = vec![
            "gpt-4o".to_string(),
            "claude-3".to_string(),
            "GPT-4o-mini".to_string(),
        ];
        let got: Vec<&String> = filter_models(&ms, "gpt");
        assert_eq!(got, vec!["gpt-4o", "GPT-4o-mini"]);
        assert_eq!(filter_models(&ms, "").len(), 3);
    }
}
