//! Local/custom model definitions loaded by the CLI before model resolution.
//!
//! This is intentionally a `coding-agent` concern: `theway-llm-provider` already has the in-process custom
//! registry, while the CLI owns user/project configuration and user-visible diagnostics.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use theway_llm_provider::{
    Api, InputModality, Model, ModelCost, ModelThinkingLevel, Provider, ThinkingLevelMap,
};

#[derive(Debug, Clone)]
pub struct LoadedLocalModels {
    pub models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
struct ModelsFile {
    #[serde(default)]
    models: Vec<Model>,
}

pub async fn load_all(cwd: &Path, cli_base_url: Option<&str>) -> Result<LoadedLocalModels> {
    let paths = [
        crate::config::base_dir().join("models.json"),
        cwd.join(".theway").join("models.json"),
    ];
    load_all_from_paths_with_base_url(&paths, cli_base_url)
}

#[cfg(test)]
fn load_all_from_paths(paths: &[PathBuf]) -> Result<LoadedLocalModels> {
    load_all_from_paths_with_base_url(paths, None)
}

pub fn load_all_from_paths_with_base_url(
    paths: &[PathBuf],
    cli_base_url: Option<&str>,
) -> Result<LoadedLocalModels> {
    let mut models = Vec::<Model>::new();
    register_builtin_local_defaults(cli_base_url);
    for path in paths {
        if !path.exists() {
            continue;
        }
        let file = load_file(path)?;
        for model in file.models {
            if let Some(existing) = models
                .iter()
                .position(|m| m.provider == model.provider && m.id == model.id)
            {
                models[existing] = model;
            } else {
                models.push(model);
            }
        }
    }
    for model in &models {
        theway_llm_provider::register_custom_model(model.clone());
    }
    Ok(LoadedLocalModels { models })
}

fn register_builtin_local_defaults(cli_base_url: Option<&str>) {
    // DS4 is a local OpenAI-compatible server, so its base URL is user/environment specific.
    // Register the conventional provider/model only when the URL is explicit; user/project
    // `models.json` entries with the same provider/id are loaded afterwards and override it.
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

fn load_file(path: &Path) -> Result<ModelsFile> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file: ModelsFile =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(file)
}

#[cfg(test)]
// Test files live in `tests/local_models/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("local_models");
