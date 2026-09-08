//! Smoke test for `models_generated.rs`. Validates the JSON catalog parses, the loader
//! produces a non-empty list, and a handful of well-known models resolve through `get_model`.

use theway_llm_provider::{
    Provider, get_model, list_apis, list_models, register_custom_model, unregister_custom_model,
};

#[test]
fn catalog_is_populated() {
    let all = list_models();
    assert!(
        all.len() > 100,
        "catalog has {} entries — expected hundreds",
        all.len()
    );
}

#[test]
fn known_anthropic_model_resolves() {
    let p = Provider::from("anthropic");
    let names: Vec<String> = list_models()
        .into_iter()
        .filter(|m| m.provider == p)
        .map(|m| m.id)
        .collect();
    assert!(!names.is_empty(), "anthropic provider has no models");
    // Pick the first one and round-trip it through get_model.
    let first = names.first().expect("at least one anthropic model");
    let m = get_model(&p, first).expect("get_model round-trip");
    assert_eq!(m.provider, p);
    assert_eq!(&m.id, first);
}

#[test]
fn apis_include_anthropic_messages() {
    let apis: Vec<String> = list_apis().into_iter().map(|a| a.0).collect();
    assert!(
        apis.iter().any(|a| a == "anthropic-messages"),
        "expected anthropic-messages api in {apis:?}"
    );
}

#[test]
fn unknown_model_returns_none() {
    let p = Provider::from("anthropic");
    assert!(get_model(&p, "does-not-exist-xyz").is_none());
}

#[test]
fn custom_model_overrides_builtin_without_listing_duplicates() {
    let p = Provider::from("deepseek");
    let id = "deepseek-v4-pro";
    let builtin = get_model(&p, id).expect("builtin deepseek-v4-pro");
    let mut custom = builtin.clone();
    custom.name = "DeepSeek V4 Pro (custom override)".into();

    register_custom_model(custom);
    let listed: Vec<_> = list_models()
        .into_iter()
        .filter(|model| model.provider == p && model.id == id)
        .collect();
    assert_eq!(
        listed.len(),
        1,
        "custom override must replace, not duplicate, the builtin entry"
    );
    assert_eq!(listed[0].name, "DeepSeek V4 Pro (custom override)");

    unregister_custom_model(&p, id);
    let listed: Vec<_> = list_models()
        .into_iter()
        .filter(|model| model.provider == p && model.id == id)
        .collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, builtin.name);
}
