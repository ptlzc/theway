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

#[cfg(test)]
mod coverage_gap {
    use super::*;
    use theway_llm_provider::Tool;

    fn tool(name: &str, parameters: serde_json::Value) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            parameters,
        }
    }

    fn draft(provider: &str, model: &str, tools: &[&str]) -> NormalizedModelRequestDraft {
        NormalizedModelRequestDraft {
            provider: provider.to_string(),
            model: model.to_string(),
            system_instructions: None,
            messages: Vec::new(),
            visible_tools: tools
                .iter()
                .map(|name| tool(name, serde_json::json!({"type": "object"})))
                .collect(),
            executable_tool_names: tools.iter().map(|name| name.to_string()).collect(),
            generation_options: NormalizedGenerationOptions::default(),
        }
    }

    #[test]
    fn validates_identical_draft_and_model_identity() {
        let base = draft("p", "m", &["t"]);
        assert!(
            draft("p", "m", &["t"])
                .validate_replacement(&base, 100)
                .is_ok()
        );

        let model_changed = draft("p", "other", &["t"])
            .validate_replacement(&base, 100)
            .unwrap_err();
        assert!(model_changed.contains("identity is immutable"));

        let empty_provider = draft("", "m", &["t"])
            .validate_replacement(&draft("", "m", &["t"]), 100)
            .unwrap_err();
        assert!(empty_provider.contains("cannot be empty"));

        let empty_model = draft("p", "", &["t"])
            .validate_replacement(&draft("p", "", &["t"]), 100)
            .unwrap_err();
        assert!(empty_model.contains("cannot be empty"));
    }

    #[test]
    fn validates_visible_tool_names_and_parameters() {
        let base = draft("p", "m", &["t"]);

        let mut empty_name = draft("p", "m", &["t"]);
        empty_name.visible_tools = vec![tool("", serde_json::json!({"type": "object"}))];
        assert!(
            empty_name
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("non-empty and unique")
        );

        let mut duplicate = draft("p", "m", &["t", "t"]);
        duplicate.visible_tools = vec![
            tool("t", serde_json::json!({"type": "object"})),
            tool("t", serde_json::json!({"type": "object"})),
        ];
        assert!(
            duplicate
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("non-empty and unique")
        );

        let mut boolean_params = draft("p", "m", &["t"]);
        boolean_params.visible_tools = vec![tool("t", serde_json::Value::Bool(true))];
        assert!(boolean_params.validate_replacement(&base, 100).is_ok());

        let mut missing_ref = draft("p", "m", &["t"]);
        missing_ref.visible_tools = vec![tool("other", serde_json::json!({"type": "object"}))];
        missing_ref.executable_tool_names = vec!["other".to_string()];
        assert!(
            missing_ref
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("no executable reference")
        );
    }

    #[test]
    fn validates_executable_tool_names_and_set_equality() {
        let base = draft("p", "m", &["t"]);

        let mut duplicate = draft("p", "m", &["t"]);
        duplicate.executable_tool_names = vec!["t".to_string(), "t".to_string()];
        assert!(
            duplicate
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("unique members")
        );

        let mut missing = draft("p", "m", &["t"]);
        missing.executable_tool_names = vec!["other".to_string()];
        assert!(
            missing
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("unique members")
        );

        let mut mismatch = draft("p", "m", &["t"]);
        mismatch.executable_tool_names = Vec::new();
        assert!(
            mismatch
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("same catalog")
        );
    }

    #[test]
    fn validates_generation_options_limits() {
        let base = draft("p", "m", &[]);

        let mut non_finite = draft("p", "m", &[]);
        non_finite.generation_options.temperature = Some(f32::NAN);
        assert!(
            non_finite
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("temperature must be finite")
        );

        let mut zero_max = draft("p", "m", &[]);
        zero_max.generation_options.max_tokens = Some(0);
        assert!(
            zero_max
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("maxTokens")
        );

        let mut over_max = draft("p", "m", &[]);
        over_max.generation_options.max_tokens = Some(200);
        assert!(
            over_max
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("maxTokens")
        );

        let mut no_limit = draft("p", "m", &[]);
        no_limit.generation_options.max_tokens = Some(200);
        assert!(no_limit.validate_replacement(&base, 0).is_ok());

        let mut zero_budget = draft("p", "m", &[]);
        zero_budget.generation_options.thinking_budgets = Some(ThinkingBudgets {
            minimal: Some(0),
            low: Some(1),
            medium: Some(1),
            high: Some(1),
        });
        assert!(
            zero_budget
                .validate_replacement(&base, 100)
                .unwrap_err()
                .contains("thinking budgets")
        );
    }
}
