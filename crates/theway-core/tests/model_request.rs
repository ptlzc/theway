//! Integration coverage for `NormalizedModelRequestDraft::validate_replacement`.

use theway_core::NormalizedGenerationOptions;
use theway_core::NormalizedModelRequestDraft;
use theway_llm_provider::{ThinkingBudgets, Tool};

fn tool(name: &str) -> Tool {
    Tool {
        name: name.to_string(),
        description: String::new(),
        parameters: serde_json::json!({"type": "object"}),
    }
}

fn draft(provider: &str, model: &str, tools: &[&str]) -> NormalizedModelRequestDraft {
    NormalizedModelRequestDraft {
        provider: provider.to_string(),
        model: model.to_string(),
        system_instructions: None,
        messages: Vec::new(),
        visible_tools: tools.iter().map(|name| tool(name)).collect(),
        executable_tool_names: tools.iter().map(|name| name.to_string()).collect(),
        generation_options: NormalizedGenerationOptions::default(),
    }
}

#[test]
fn validate_replacement_accepts_identical_draft() {
    let base = draft("p", "m", &["t"]);
    assert!(
        draft("p", "m", &["t"])
            .validate_replacement(&base, 100)
            .is_ok()
    );
}

#[test]
fn validate_replacement_rejects_model_identity_change() {
    let base = draft("p", "m", &["t"]);
    let err = draft("p", "other-model", &["t"])
        .validate_replacement(&base, 100)
        .unwrap_err();
    assert!(err.contains("identity is immutable"), "{err}");
}

#[test]
fn validate_replacement_rejects_empty_provider() {
    let base = draft("", "m", &["t"]);
    let err = draft("", "m", &["t"])
        .validate_replacement(&base, 100)
        .unwrap_err();
    assert!(err.contains("cannot be empty"), "{err}");
}

#[test]
fn validate_replacement_rejects_empty_model() {
    let base = draft("p", "", &["t"]);
    let err = draft("p", "", &["t"])
        .validate_replacement(&base, 100)
        .unwrap_err();
    assert!(err.contains("cannot be empty"), "{err}");
}

#[test]
fn validate_replacement_rejects_empty_visible_tool_name() {
    let base = draft("p", "m", &["t"]);
    let mut patch = draft("p", "m", &["t"]);
    patch.visible_tools = vec![tool("")];
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("non-empty and unique"), "{err}");
}

#[test]
fn validate_replacement_rejects_duplicate_visible_tool_names() {
    let base = draft("p", "m", &["t", "t"]);
    let mut patch = draft("p", "m", &["t", "t"]);
    patch.visible_tools = vec![tool("t"), tool("t")];
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("non-empty and unique"), "{err}");
}

#[test]
fn validate_replacement_accepts_boolean_tool_parameters() {
    let base = draft("p", "m", &["t"]);
    let mut patch = draft("p", "m", &["t"]);
    patch.visible_tools = vec![Tool {
        name: "t".to_string(),
        description: String::new(),
        parameters: serde_json::Value::Bool(true),
    }];
    assert!(patch.validate_replacement(&base, 100).is_ok());
}

#[test]
fn validate_replacement_rejects_visible_tool_missing_from_base() {
    let base = draft("p", "m", &["t"]);
    let mut patch = draft("p", "m", &["t"]);
    patch.visible_tools = vec![tool("t"), tool("other")];
    patch.executable_tool_names = vec!["t".to_string(), "other".to_string()];
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("no executable reference"), "{err}");
}

#[test]
fn validate_replacement_rejects_duplicate_executable_tool_names() {
    let base = draft("p", "m", &["t"]);
    let mut patch = draft("p", "m", &["t"]);
    patch.visible_tools = vec![tool("t")];
    patch.executable_tool_names = vec!["t".to_string(), "t".to_string()];
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("unique members"), "{err}");
}

#[test]
fn validate_replacement_rejects_executable_tool_missing_from_base() {
    let base = draft("p", "m", &["t"]);
    let mut patch = draft("p", "m", &["t"]);
    patch.visible_tools = vec![tool("t")];
    patch.executable_tool_names = vec!["other".to_string()];
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("unique members"), "{err}");
}

#[test]
fn validate_replacement_rejects_visible_executable_mismatch() {
    let base = draft("p", "m", &["t"]);
    let mut patch = draft("p", "m", &["t"]);
    patch.visible_tools = vec![tool("t")];
    patch.executable_tool_names = vec!["t".to_string()];
    // Same names but different lengths: visible has two, executable has one.
    patch.visible_tools = vec![tool("t"), tool("u")];
    patch.executable_tool_names = vec!["t".to_string()];
    // `u` is in base? No, so first it would hit "no executable reference".
    // Instead make visible == base but executable missing one of them.
    patch.visible_tools = vec![tool("t")];
    patch.executable_tool_names = Vec::new();
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("same catalog"), "{err}");
}

#[test]
fn validate_replacement_rejects_non_finite_temperature() {
    let base = draft("p", "m", &[]);
    let mut patch = draft("p", "m", &[]);
    patch.generation_options.temperature = Some(f32::NAN);
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("temperature must be finite"), "{err}");
}

#[test]
fn validate_replacement_rejects_zero_max_tokens() {
    let base = draft("p", "m", &[]);
    let mut patch = draft("p", "m", &[]);
    patch.generation_options.max_tokens = Some(0);
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("maxTokens"), "{err}");
}

#[test]
fn validate_replacement_rejects_max_tokens_above_model_limit() {
    let base = draft("p", "m", &[]);
    let mut patch = draft("p", "m", &[]);
    patch.generation_options.max_tokens = Some(200);
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("maxTokens"), "{err}");
}

#[test]
fn validate_replacement_ignores_max_tokens_limit_when_model_max_is_zero() {
    let base = draft("p", "m", &[]);
    let mut patch = draft("p", "m", &[]);
    patch.generation_options.max_tokens = Some(200);
    assert!(patch.validate_replacement(&base, 0).is_ok());
}

#[test]
fn validate_replacement_rejects_zero_thinking_budget() {
    let base = draft("p", "m", &[]);
    let mut patch = draft("p", "m", &[]);
    patch.generation_options.thinking_budgets = Some(ThinkingBudgets {
        minimal: Some(0),
        low: Some(1),
        medium: Some(1),
        high: Some(1),
    });
    let err = patch.validate_replacement(&base, 100).unwrap_err();
    assert!(err.contains("thinking budgets"), "{err}");
}

#[test]
fn validate_replacement_accepts_positive_thinking_budgets() {
    let base = draft("p", "m", &[]);
    let mut patch = draft("p", "m", &[]);
    patch.generation_options.thinking_budgets = Some(ThinkingBudgets {
        minimal: Some(1),
        low: Some(2),
        medium: Some(3),
        high: Some(4),
    });
    assert!(patch.validate_replacement(&base, 100).is_ok());
}
