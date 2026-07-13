//! Fetch a provider's model list from its OpenAI-compatible `/models` endpoint,
//! so the config wizard can offer the models to pick from instead of making the
//! user type an id. Parsing is pure (unit-tested); the fetch is best-effort and
//! its failure degrades the wizard to manual entry rather than erroring.

#[cfg(test)]
use serde_json::Value;

/// Extract model ids from a `/models` response body: the standard OpenAI shape
/// `{ "data": [ { "id": "…" }, … ] }`. Tolerates a bare array or a top-level
/// `models` array too. Returns them de-duplicated in first-seen order; a body
/// that isn't recognisable yields an empty list (the caller falls back to manual
/// entry). Pure.
#[cfg(test)]
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
