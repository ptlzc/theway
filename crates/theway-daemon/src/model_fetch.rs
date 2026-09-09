//! Fetch an OpenAI-compatible model catalog for `[model] auto_fetch_models`
//! (issue #136).
//!
//! `GET {base_url}/models` is the only endpoint; a successful response turns
//! every entry into a custom model descriptor so the model picker and
//! `get_model` see the server's catalog without a `[model] model` value or a
//! hand-written `[[model.custom]]` entry. Failures are returned to the caller,
//! which logs them and keeps the daemon running model-less.

use std::time::Duration;

use serde_json::Value;
use theway_llm_provider::{Api, InputModality, Model, ModelCost, Provider};

/// Request budget for one catalog fetch. Bounded so a hung server cannot wedge
/// the serialized command loop.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Default API family for synthesized entries. Local OpenAI-compatible servers
/// speak chat completions; a `[[model.custom]]` entry overrides this per model.
const DEFAULT_API: &str = "openai-completions";

/// Convert a `/models` response body into model descriptors.
///
/// Accepts the OpenAI shape (`{"data":[{"id": "..."}]}`), a bare array, and the
/// Ollama-style `{"models":[{"name": "..."}]}`. Entries without an id/name are
/// skipped; duplicates keep the first occurrence. Fields the endpoint does not
/// report (context window, reasoning, cost) get documented defaults that a
/// `[[model.custom]]` entry with the same `(provider, id)` overrides.
pub fn models_from_catalog(body: &Value, provider: &str, base_url: &str, api: &str) -> Vec<Model> {
    let entries = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
        .or_else(|| body.get("models").and_then(Value::as_array));
    let Some(entries) = entries else {
        return Vec::new();
    };
    let mut models = Vec::<Model>::new();
    for entry in entries {
        let Some(id) = entry
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| entry.get("name").and_then(Value::as_str))
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if models.iter().any(|model| model.id == id) {
            continue;
        }
        models.push(Model {
            id: id.to_string(),
            name: id.to_string(),
            api: Api::from(api),
            provider: Provider::from(provider),
            base_url: base_url.to_string(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![InputModality::Text],
            cost: ModelCost::default(),
            context_window: 128_000,
            max_tokens: 8_192,
            headers: None,
            compat: None,
        });
    }
    models
}

/// Fetch and convert `GET {base_url}/models`.
pub async fn fetch_models(
    base_url: &str,
    api_key: Option<&str>,
    provider: &str,
) -> Result<Vec<Model>, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("base_url is required to fetch the model catalog".into());
    }
    let url = format!("{base}/models");
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|error| format!("build HTTP client: {error}"))?;
    let mut request = client.get(&url);
    if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("GET {url}: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("GET {url}: {error}"))?;
    if !status.is_success() {
        let preview: String = text.chars().take(200).collect();
        return Err(format!("GET {url}: HTTP {status}: {preview}"));
    }
    let body: Value =
        serde_json::from_str(&text).map_err(|error| format!("GET {url}: invalid JSON: {error}"))?;
    Ok(models_from_catalog(&body, provider, base, DEFAULT_API))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_shape_converts_and_dedupes() {
        let body = json!({
            "object": "list",
            "data": [
                {"id": "qwen3-local"},
                {"id": "llama-4-local", "owned_by": "local"},
                {"id": "qwen3-local"},
                {"id": ""},
                {"no_id": true}
            ]
        });
        let models = models_from_catalog(&body, "ds4", "http://127.0.0.1:8000/v1", DEFAULT_API);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "qwen3-local");
        assert_eq!(models[0].provider.0, "ds4");
        assert_eq!(models[0].base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(models[0].api.0, DEFAULT_API);
        assert_eq!(models[1].id, "llama-4-local");
        assert_eq!(models[1].name, "llama-4-local");
        assert_eq!(models[0].context_window, 128_000);
        assert_eq!(models[0].max_tokens, 8_192);
    }

    #[test]
    fn bare_array_and_ollama_shapes_are_accepted() {
        let bare = json!([{"id": "a"}, {"id": "b"}]);
        assert_eq!(
            models_from_catalog(&bare, "p", "http://x", DEFAULT_API)
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        let ollama = json!({"models": [{"name": "c"}]});
        let models = models_from_catalog(&ollama, "p", "http://x", DEFAULT_API);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "c");
    }

    #[test]
    fn unknown_shape_yields_no_models() {
        assert!(
            models_from_catalog(&json!({"error": "nope"}), "p", "http://x", DEFAULT_API).is_empty()
        );
        assert!(models_from_catalog(&json!("text"), "p", "http://x", DEFAULT_API).is_empty());
    }
}

#[cfg(test)]
mod model_fetch_tests {
    //! Mirrored tests live in `tests/model_fetch/`; wrapped because the
    //! top-level `mod tests` slot is already used by the inline unit tests.
    tests_bridge_macro::tests_bridge!("model_fetch");
}
