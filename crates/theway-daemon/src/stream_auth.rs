//! The auth-store-backed stream function wrapper for LLM calls.

use theway_core::AgentMessage;
use theway_llm_provider::Message as PiMessage;

pub fn stream_fn_with_auth_store() -> theway_core::StreamFn {
    std::sync::Arc::new(|model, context, options| {
        let merged = apply_auth_to_simple_options(model, options, |provider| {
            theway_transport::auth::AuthStore::load()
                .ok()
                .and_then(|store| store.resolve_for_provider(provider))
        });
        theway_llm_provider::stream_simple(model, context, Some(&merged))
    })
}

fn apply_auth_to_simple_options<F>(
    model: &theway_llm_provider::Model,
    options: Option<&theway_llm_provider::SimpleStreamOptions>,
    resolve_api_key: F,
) -> theway_llm_provider::SimpleStreamOptions
where
    F: FnOnce(&str) -> Option<String>,
{
    let mut merged = options.cloned().unwrap_or_default();
    let needs_api_key = merged
        .base
        .api_key
        .as_deref()
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true);
    if needs_api_key {
        if let Some(api_key) = resolve_api_key(&model.provider.0).filter(|k| !k.trim().is_empty()) {
            merged.base.api_key = Some(api_key);
        }
    }
    merged
}

/// Helper for callers that want to feed a Message (raw theway-llm-provider role variant) into the agent. Not
/// directly used by the REPL but kept here for the tests.
pub fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(text.into()),
        timestamp: chrono::Utc::now().timestamp_millis(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str) -> theway_llm_provider::Model {
        theway_llm_provider::Model {
            id: "deepseek-v4-flash".into(),
            name: "DeepSeek V4 Flash".into(),
            api: theway_llm_provider::Api::from("openai-responses"),
            provider: theway_llm_provider::Provider::from(provider),
            base_url: "http://127.0.0.1:8000/v1".into(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![theway_llm_provider::InputModality::Text],
            cost: theway_llm_provider::ModelCost::default(),
            context_window: 100_000,
            max_tokens: 384_000,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn auth_wrapper_injects_provider_scoped_stored_key() {
        let opts = apply_auth_to_simple_options(&model("ds4"), None, |provider| {
            assert_eq!(provider, "ds4");
            Some("stored-ds4-key".into())
        });
        assert_eq!(opts.base.api_key.as_deref(), Some("stored-ds4-key"));
    }

    #[test]
    fn auth_wrapper_keeps_explicit_api_key() {
        let mut existing = theway_llm_provider::SimpleStreamOptions::default();
        existing.base.api_key = Some("explicit-key".into());
        let opts = apply_auth_to_simple_options(&model("ds4"), Some(&existing), |_| {
            Some("stored-ds4-key".into())
        });
        assert_eq!(opts.base.api_key.as_deref(), Some("explicit-key"));
    }

    #[test]
    fn auth_wrapper_fails_closed_without_provider_scoped_key() {
        let opts = apply_auth_to_simple_options(&model("ds4"), None, |_| None);
        assert_eq!(opts.base.api_key, None);
    }
}
