//! Provider-independent request data assembled immediately before model dispatch.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use theway_llm_provider::{Message, ThinkingBudgets, ThinkingLevel, Tool};

/// Generation controls that every built-in provider adapter can receive through
/// [`theway_llm_provider::SimpleStreamOptions`]. Transport, authentication, retry,
/// and provider-specific fields are intentionally excluded from extension patches.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedGenerationOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ThinkingLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budgets: Option<ThinkingBudgets>,
}

/// Request-local, provider-independent draft exposed to `before_model_request`.
///
/// `executable_tool_names` are stable references into the immutable registry
/// snapshot captured while constructing this draft. The runtime resolves them
/// back to executable implementations only after the replacement is validated.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedModelRequestDraft {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_instructions: Option<String>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub visible_tools: Vec<Tool>,
    #[serde(default)]
    pub executable_tool_names: Vec<String>,
    #[serde(default)]
    pub generation_options: NormalizedGenerationOptions,
}

impl NormalizedModelRequestDraft {
    /// Validate an extension replacement against the immutable base snapshot.
    /// The caller keeps the base draft when validation fails, making the patch
    /// atomic and request-local.
    pub fn validate_replacement(&self, base: &Self, model_max_tokens: u32) -> Result<(), String> {
        if self.provider != base.provider || self.model != base.model {
            return Err("normalized request provider/model identity is immutable".into());
        }
        if self.provider.trim().is_empty() || self.model.trim().is_empty() {
            return Err("normalized request provider/model identity cannot be empty".into());
        }

        let base_names = base
            .executable_tool_names
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut visible_names = HashSet::new();
        for tool in &self.visible_tools {
            if tool.name.trim().is_empty() || !visible_names.insert(tool.name.as_str()) {
                return Err("visible tool names must be non-empty and unique".into());
            }
            if !tool.parameters.is_object() && !tool.parameters.is_boolean() {
                return Err(format!(
                    "visible tool '{}' parameters must be a JSON Schema object or boolean",
                    tool.name
                ));
            }
            if !base_names.contains(tool.name.as_str()) {
                return Err(format!(
                    "visible tool '{}' has no executable reference in the base request",
                    tool.name
                ));
            }
        }

        let mut executable_names = HashSet::new();
        for name in &self.executable_tool_names {
            if !executable_names.insert(name.as_str()) || !base_names.contains(name.as_str()) {
                return Err(
                    "executable tool references must be unique members of the base request".into(),
                );
            }
        }
        if executable_names != visible_names {
            return Err(
                "visible tool definitions and executable tool references must name the same catalog"
                    .into(),
            );
        }

        if self
            .generation_options
            .temperature
            .is_some_and(|temperature| !temperature.is_finite())
        {
            return Err("generation temperature must be finite".into());
        }
        if let Some(max_tokens) = self.generation_options.max_tokens {
            if max_tokens == 0 || (model_max_tokens > 0 && max_tokens > model_max_tokens) {
                return Err("generation maxTokens must be within the selected model limit".into());
            }
        }
        if self
            .generation_options
            .thinking_budgets
            .as_ref()
            .is_some_and(|budgets| {
                [budgets.minimal, budgets.low, budgets.medium, budgets.high]
                    .into_iter()
                    .flatten()
                    .any(|budget| budget == 0)
            })
        {
            return Err("thinking budgets must be greater than zero".into());
        }
        Ok(())
    }
}
