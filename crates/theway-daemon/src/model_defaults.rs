//! Built-in local model defaults registered before model resolution.
//!
//! DS4 is a local OpenAI-compatible server whose base URL is user- or
//! environment-specific, so the conventional `ds4:deepseek-v4-flash` entry is
//! registered only when a URL is explicit (`--base-url`, `DS4_BASE_URL`, or
//! `DS4_URL`). `[[model.custom]]` entries and auto-fetched models register
//! afterwards and override it by `(provider, id)`.

use theway_llm_provider::{
    Api, InputModality, Model, ModelCost, ModelThinkingLevel, Provider, ThinkingLevelMap,
};

/// Register controller-provisioned custom model descriptors (issue #136) in
/// the process catalog. Same-key entries replace earlier ones.
pub fn register_models(models: &[Model]) {
    for model in models {
        theway_llm_provider::register_custom_model(model.clone());
    }
}

/// Register the built-in DS4 default when a base URL is explicit.
pub fn register_ds4_default(cli_base_url: Option<&str>) {
    if let Some(base_url) = ds4_base_url(cli_base_url) {
        theway_llm_provider::register_custom_model(ds4_model(base_url));
    }
}

fn ds4_base_url(cli_base_url: Option<&str>) -> Option<String> {
    cli_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(ds4_base_url_from_env)
}

fn ds4_base_url_from_env() -> Option<String> {
    ["DS4_BASE_URL", "DS4_URL"].into_iter().find_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn ds4_model(base_url: String) -> Model {
    let thinking_level_map = [
        (ModelThinkingLevel::Off, None),
        (ModelThinkingLevel::Minimal, Some("low".into())),
        (ModelThinkingLevel::Low, Some("low".into())),
        (ModelThinkingLevel::Medium, Some("medium".into())),
        (ModelThinkingLevel::High, Some("high".into())),
        (ModelThinkingLevel::Xhigh, Some("xhigh".into())),
        (ModelThinkingLevel::Max, Some("max".into())),
    ]
    .into_iter()
    .collect::<ThinkingLevelMap>();
    Model {
        id: "deepseek-v4-flash".into(),
        name: "DeepSeek V4 Flash (local DS4)".into(),
        api: Api::from("openai-responses"),
        provider: Provider::from("ds4"),
        base_url,
        reasoning: true,
        thinking_level_map: Some(thinking_level_map),
        input: vec![InputModality::Text],
        cost: ModelCost::default(),
        context_window: 100_000,
        max_tokens: 384_000,
        headers: None,
        compat: Some(serde_json::json!({
            "supportsStore": false,
            "supportsDeveloperRole": false,
            "supportsReasoningEffort": true,
            "supportsUsageInStreaming": true,
            "maxTokensField": "max_tokens",
            "supportsStrictMode": false,
            "thinkingFormat": "deepseek",
            "requiresReasoningContentOnAssistantMessages": true
        })),
    }
}

#[cfg(test)]
mod model_defaults_tests {
    //! Mirrored tests live in `tests/model_defaults/`.
    tests_bridge_macro::tests_bridge!("model_defaults");
}
