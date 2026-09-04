//! `resolve_run_model`: explicit provider/model catalog resolution, the legacy
//! model-only id rewrite, and the no-override parent fallback.

use crate::multiagent::runner::resolve_run_model;
use theway_llm_provider::{
    Api, InputModality, Model, ModelCost, Provider, register_custom_model,
    unregister_custom_model,
};

fn model(provider: &str, id: &str, base_url: &str) -> Model {
    Model {
        id: id.into(),
        name: format!("{provider} {id}"),
        api: Api::from("faux"),
        provider: Provider::from(provider),
        base_url: base_url.into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![InputModality::Text],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

#[test]
fn explicit_pair_resolves_from_catalog_without_parent() {
    let provider = "test-resolve-provider";
    let id = "test-resolve-model";
    register_custom_model(model(provider, id, "http://catalog.local/v1"));

    let resolved = resolve_run_model(None, Some(provider), Some(id))
        .expect("catalog pair must resolve")
        .expect("explicit pair resolves without a parent model");
    assert_eq!(resolved.provider.0, provider);
    assert_eq!(resolved.id, id);
    assert_eq!(resolved.base_url, "http://catalog.local/v1");

    unregister_custom_model(&Provider::from(provider), id);
}

#[test]
fn unknown_pair_errors_with_catalog_hint() {
    let err = resolve_run_model(None, Some("test-no-such-provider"), Some("x"))
        .expect_err("unknown provider must fail");
    assert!(
        err.contains("model provider not found in catalog"),
        "{err}"
    );
    assert!(err.contains("test-no-such-provider"), "{err}");
}

#[test]
fn provider_without_model_errors() {
    let err = resolve_run_model(Some(&model("p", "m", "u")), Some("p"), None)
        .expect_err("provider-only override must fail");
    assert!(err.contains("provider override requires a model override"), "{err}");
}

#[test]
fn model_only_uses_catalog_entry_on_the_parent_provider() {
    let parent = model("test-rewrite-provider", "parent-id", "http://parent.local/v1");
    register_custom_model(model(
        "test-rewrite-provider",
        "catalog-id",
        "http://catalog.local/v1",
    ));

    let resolved = resolve_run_model(Some(&parent), None, Some("catalog-id"))
        .expect("model-only override must resolve")
        .expect("parent is present");
    assert_eq!(resolved.provider.0, "test-rewrite-provider");
    assert_eq!(resolved.id, "catalog-id");
    assert_eq!(resolved.base_url, "http://catalog.local/v1");

    unregister_custom_model(
        &Provider::from("test-rewrite-provider"),
        "catalog-id",
    );
}

#[test]
fn model_only_unknown_id_rewrites_the_parent_descriptor() {
    let parent = model("test-rewrite-provider", "parent-id", "http://parent.local/v1");
    let resolved = resolve_run_model(Some(&parent), None, Some("unknown-id"))
        .expect("legacy rewrite must not fail")
        .expect("parent is present");
    assert_eq!(resolved.provider.0, "test-rewrite-provider");
    assert_eq!(resolved.id, "unknown-id");
    assert_eq!(resolved.base_url, "http://parent.local/v1");
}

#[test]
fn model_only_without_parent_returns_none() {
    assert!(resolve_run_model(None, None, Some("any-id"))
        .expect("no explicit pair must not error")
        .is_none());
}

#[test]
fn same_id_returns_the_parent_model() {
    let parent = model("test-rewrite-provider", "same-id", "http://parent.local/v1");
    let resolved = resolve_run_model(Some(&parent), None, Some("same-id"))
        .unwrap()
        .unwrap();
    assert_eq!(resolved.provider.0, parent.provider.0);
    assert_eq!(resolved.id, parent.id);
    assert_eq!(resolved.base_url, parent.base_url);
}

#[test]
fn no_overrides_return_the_parent_model() {
    let parent = model("test-rewrite-provider", "parent-id", "http://parent.local/v1");
    let resolved = resolve_run_model(Some(&parent), None, None)
        .unwrap()
        .expect("parent is present");
    assert_eq!(resolved.provider.0, parent.provider.0);
    assert_eq!(resolved.id, parent.id);
    assert_eq!(resolved.base_url, parent.base_url);
    assert!(resolve_run_model(None, None, None)
        .expect("no overrides must not error")
        .is_none());
}
