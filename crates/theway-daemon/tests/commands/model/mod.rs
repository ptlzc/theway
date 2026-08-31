//! Tests for `model` (root module) — split out of src (see docs/rust-test-files.md).

use theway_llm_provider::{Api, Model, Provider};
use theway_transport::auth::{AuthStore, ProviderCredential};

use crate::model::{
    auto_detect_model, explicit_model_not_found_message, first_model_for_provider, CANDIDATES,
};
use crate::test_env::{EnvGuard, ENV_LOCK};

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

fn remove_envs_for_candidates() -> Vec<EnvGuard> {
    let mut guards = Vec::new();
    for name in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "DS4_API_KEY",
        "OPENROUTER_API_KEY",
        "GROQ_API_KEY",
        "MISTRAL_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
    ] {
        guards.push(EnvGuard::remove(name));
    }
    guards
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
fn candidates_start_with_anthropic_and_cover_eight_providers() {
    assert_eq!(CANDIDATES.len(), 8);
    assert_eq!(CANDIDATES[0].0, "ANTHROPIC_API_KEY");
    assert_eq!(CANDIDATES[0].1, "anthropic");
    assert!(CANDIDATES.iter().any(|(_, p, _)| *p == "google"));
}

#[test]
fn credential_less_startup_fails_loudly_with_guidance() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _guards = remove_envs_for_candidates();

    let err = auto_detect_model(None, None).unwrap_err().to_string();
    assert!(err.contains("no API key found"), "{err}");
    assert!(err.contains("--provider"), "{err}");
    assert!(err.contains("config.toml"), "{err}");
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
    let _env_lock = ENV_LOCK.lock().unwrap();
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
fn first_model_for_provider_falls_back_to_any_catalog_model() {
    let model = first_model_for_provider("openai").expect("openai has catalog models");
    assert_eq!(model.provider, Provider::from("openai"));
    assert!(first_model_for_provider("not-a-provider").is_none());
}

#[test]
fn auto_detect_model_env_detection_returns_catalog_model() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _cleanup = remove_envs_for_candidates();
    let _groq = EnvGuard::set("GROQ_API_KEY", "gsk-test");

    let model = auto_detect_model(None, None).unwrap();
    assert_eq!(model.provider, Provider::from("groq"));
    assert_eq!(model.id, "llama-3.3-70b-versatile");
}

#[test]
fn auto_detect_model_ignores_empty_env_vars_and_errors() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _cleanup = remove_envs_for_candidates();
    let _openai = EnvGuard::set("OPENAI_API_KEY", "");
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let err = auto_detect_model(None, None).unwrap_err().to_string();
    assert!(err.contains("no API key found"), "{err}");
    assert!(err.contains("OPENAI_API_KEY"), "{err}");
}

#[test]
fn auto_detect_model_uses_auth_store_fallback() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _cleanup = remove_envs_for_candidates();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let mut store = AuthStore::default();
    store.set("groq", ProviderCredential::ApiKey { value: "gsk-stored".into() });
    store.save().unwrap();

    let model = auto_detect_model(None, None).unwrap();
    assert_eq!(model.provider, Provider::from("groq"));
    assert_eq!(model.id, "llama-3.3-70b-versatile");
}

#[test]
fn auto_detect_model_no_credentials_lists_env_vars() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _cleanup = remove_envs_for_candidates();
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let err = auto_detect_model(None, None).unwrap_err().to_string();
    assert!(err.contains("no API key found"), "{err}");
    for env in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "DS4_API_KEY", "OPENROUTER_API_KEY"] {
        assert!(err.contains(env), "{env} missing from {err}");
    }
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
fn auto_detect_model_catalog_miss_falls_back_to_first_provider_model() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let _cleanup = remove_envs_for_candidates();
    let _ds4 = EnvGuard::set("DS4_API_KEY", "dsv4-test");
    let base = tempfile::tempdir().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    // Arrange: no default ds4 model, but a custom ds4 model exists.
    unregister_all_ds4_custom_models();
    theway_llm_provider::register_custom_model(local_model("ds4", "custom-ds4-fallback"));

    let model = auto_detect_model(None, None).unwrap();
    assert_eq!(model.provider, Provider::from("ds4"));
    assert_eq!(model.id, "custom-ds4-fallback");

    theway_llm_provider::unregister_custom_model(
        &Provider::from("ds4"),
        "custom-ds4-fallback",
    );
}
