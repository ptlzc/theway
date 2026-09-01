//! Model resolution for an explicitly-selected provider/model pair.
//!
//! Model selection is session-level and owned by the client (the daemon starts
//! model-less and the TUI injects a model per-session via `SetModel`). The
//! daemon therefore never auto-detects a model from environment variables or
//! an auth store here — it only resolves a pair the caller explicitly passed.

use anyhow::{Result, bail};
use theway_llm_provider::{Model, Provider, get_model};

/// Resolve an explicitly-selected `(provider, model)` pair against the model
/// catalog. Neither may be omitted: the daemon no longer auto-detects a model
/// from env credentials, so a missing override is an error that tells the
/// caller the model must come from the client (session-level).
pub fn auto_detect_model(
    override_provider: Option<&str>,
    override_model: Option<&str>,
) -> Result<Model> {
    let Some((provider, id)) = override_provider.zip(override_model) else {
        bail!(
            "no model selected. The daemon starts model-less: select a model in the TUI so it can be injected into the session"
        );
    };
    let provider_obj = Provider::from(provider);
    if let Some(m) = get_model(&provider_obj, id) {
        return Ok(m);
    }
    bail!("{}", explicit_model_not_found_message(provider, id, true));
}

fn explicit_model_not_found_message(provider: &str, id: &str, show_local_hint: bool) -> String {
    let mut by_provider = std::collections::BTreeMap::<String, Vec<String>>::new();
    for model in theway_llm_provider::list_models() {
        by_provider
            .entry(model.provider.0)
            .or_default()
            .push(model.id);
    }
    let Some(models) = by_provider.get_mut(provider) else {
        let providers = by_provider
            .iter()
            .map(|(provider, models)| format!("{provider}({})", models.len()))
            .collect::<Vec<_>>()
            .join(", ");
        let hint = if show_local_hint && provider == "ds4" {
            " For local DS4, pass --base-url http://127.0.0.1:8000/v1, set DS4_BASE_URL, or add ds4 to ~/.theway/models.json."
        } else {
            ""
        };
        return format!(
            "model provider not found in catalog: provider={provider}. Known providers: {providers}"
        ) + hint;
    };
    models.sort();
    let candidates = models
        .iter()
        .take(12)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let more = if models.len() > 12 {
        format!(
            "; run `/model list {provider}` inside theway for all {} models",
            models.len()
        )
    } else {
        String::new()
    };
    format!(
        "model not found in catalog: provider={provider} id={id}. Candidates: {candidates}{more}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_model(provider: &str, id: &str) -> Model {
        Model {
            id: id.into(),
            name: format!("Local {id}"),
            api: theway_llm_provider::Api::from("openai-responses"),
            provider: Provider::from(provider),
            base_url: "http://127.0.0.1:8000/v1".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![theway_llm_provider::InputModality::Text],
            cost: theway_llm_provider::ModelCost::default(),
            context_window: 100_000,
            max_tokens: 100_000,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn explicit_override_resolves_custom_model_registered_before_detection() {
        let provider = Provider::from("local-test-model-detect");
        let id = "deepseek-v4-flash";
        theway_llm_provider::register_custom_model(local_model(&provider.0, id));

        let resolved = auto_detect_model(Some(&provider.0), Some(id)).unwrap();
        assert_eq!(resolved.provider, provider);
        assert_eq!(resolved.id, id);

        theway_llm_provider::unregister_custom_model(
            &Provider::from("local-test-model-detect"),
            id,
        );
    }
}

#[cfg(test)]
mod commands_model_tests {
    tests_bridge_macro::tests_bridge!("commands/model");
}
