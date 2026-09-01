//! Tests for `model` (root module) — split out of src (see docs/rust-test-files.md).

use theway_llm_provider::{Api, Model, Provider};

use crate::model::{auto_detect_model, explicit_model_not_found_message};

fn local_model(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        name: format!("Local {id}"),
        api: Api::from("openai-responses"),
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

fn unregister_all_ds4_custom_models() {
    let ds4 = Provider::from("ds4");
    for model in theway_llm_provider::list_models() {
        if model.provider == ds4 {
            theway_llm_provider::unregister_custom_model(&ds4, &model.id);
        }
    }
}

#[test]
fn auto_detect_model_requires_both_override_parts() {
    // Model is session-level (injected by the client): the daemon no longer
    // auto-detects from env, so a missing override is a clear client-side error.
    let err = auto_detect_model(None, None).unwrap_err().to_string();
    assert!(err.contains("no model selected"), "{err}");
    assert!(err.contains("TUI"), "{err}");

    // A lone override (provider without model, or model without provider) also
    // fails rather than guessing.
    let err = auto_detect_model(Some("openai"), None).unwrap_err().to_string();
    assert!(err.contains("no model selected"), "{err}");
}

#[test]
fn explicit_model_not_found_message_lists_sorted_candidates_and_more_hint() {
    let message = explicit_model_not_found_message("openai", "definitely-not-a-model", false);
    assert!(message.contains("model not found in catalog"), "{message}");
    assert!(message.contains("provider=openai id=definitely-not-a-model"), "{message}");
    assert!(message.contains("Candidates: gpt-4, "), "{message}");
    assert!(message.contains("run `/model list openai` inside theway for all 42 models"), "{message}");
}

#[test]
fn explicit_model_not_found_message_handles_unknown_provider_with_ds4_hint() {
    // Arrange: ensure no custom ds4 models make ds4 look known.
    unregister_all_ds4_custom_models();

    let without_hint = explicit_model_not_found_message("ds4", "deepseek-v4-flash", false);
    assert!(without_hint.contains("model provider not found in catalog: provider=ds4"), "{without_hint}");
    assert!(!without_hint.contains("--base-url"), "{without_hint}");

    let with_hint = explicit_model_not_found_message("ds4", "deepseek-v4-flash", true);
    assert!(with_hint.contains("For local DS4, pass --base-url"), "{with_hint}");
    assert!(with_hint.contains("DS4_BASE_URL"), "{with_hint}");

    let unknown = explicit_model_not_found_message("not-a-provider", "x", true);
    assert!(unknown.contains("Known providers: "), "{unknown}");
    assert!(unknown.contains("anthropic("), "{unknown}");
    assert!(!unknown.contains("--base-url"), "{unknown}");
}

#[test]
fn auto_detect_model_explicit_override_unknown_model_lists_candidates() {
    let err = auto_detect_model(Some("openai"), Some("definitely-not-a-model"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("model not found in catalog"), "{err}");
    assert!(err.contains("Candidates:"), "{err}");
    assert!(err.contains("run `/model list openai`"), "{err}");
}

#[test]
fn auto_detect_model_explicit_override_unknown_provider_lists_providers() {
    let err = auto_detect_model(Some("not-a-provider"), Some("x"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("model provider not found in catalog"), "{err}");
    assert!(err.contains("Known providers:"), "{err}");
    assert!(err.contains("anthropic("), "{err}");
}

#[test]
fn auto_detect_model_explicit_override_resolves_catalog_model() {
    let provider = "openai";
    let err = auto_detect_model(Some(provider), Some("definitely-not-a-model"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("model not found in catalog"), "{err}");
}
