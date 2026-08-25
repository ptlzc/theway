use std::path::Path;

use theway_core::ThinkingLevel;
use theway_transport::wire::WireSessionRuntimeContext;

use super::*;

#[test]
fn resolve_provider_model_uses_request_pair() {
    // Arrange
    let persisted = runtime_context(Path::new("/tmp"));
    let request = WireSessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: Some("provider-a".into()),
        model: Some("model-a".into()),
        base_url: None,
        thinking: None,
    };

    // Act
    let resolved = resolve_provider_model(&persisted, &request).unwrap();

    // Assert
    assert_eq!(resolved, (Some("provider-a".into()), Some("model-a".into())));
}

#[test]
fn resolve_provider_model_uses_persisted_pair_when_request_omits_both() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: Some("provider-b".into()),
        model: Some("model-b".into()),
        base_url: None,
        thinking: None,
    };
    let request = WireSessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: None,
        thinking: None,
    };

    // Act
    let resolved = resolve_provider_model(&persisted, &request).unwrap();

    // Assert
    assert_eq!(resolved, (Some("provider-b".into()), Some("model-b".into())));
}

#[test]
fn resolve_provider_model_returns_none_when_nothing_is_selected() {
    // Arrange
    let persisted = runtime_context(Path::new("/tmp"));
    let request = WireSessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: None,
        thinking: None,
    };

    // Act
    let resolved = resolve_provider_model(&persisted, &request).unwrap();

    // Assert
    assert_eq!(resolved, (None, None));
}

#[test]
fn resolve_provider_model_rejects_incomplete_persisted_selection() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: Some("provider-c".into()),
        model: None,
        base_url: None,
        thinking: None,
    };
    let request = WireSessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: None,
        thinking: None,
    };

    // Act
    let err = resolve_provider_model(&persisted, &request).unwrap_err();

    // Assert
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("incomplete"));
}

#[test]
fn resolve_effective_model_uses_startup_when_no_selection() {
    // Arrange
    let startup = sample_model("startup", "provider-startup");
    let persisted = runtime_context(Path::new("/tmp"));

    // Act
    let model = resolve_effective_model(&startup, &persisted, None, None, None).unwrap();

    // Assert
    assert_eq!(model.id, "startup");
    assert_eq!(model.provider.0, "provider-startup");
}

#[test]
fn resolve_effective_model_applies_base_url_to_startup_fallback() {
    // Arrange
    let startup = sample_model("startup", "provider-startup");
    let persisted = runtime_context(Path::new("/tmp"));

    // Act
    let model =
        resolve_effective_model(&startup, &persisted, None, None, Some(" http://localhost:8080 "))
            .unwrap();

    // Assert
    assert_eq!(model.base_url, "http://localhost:8080");
}

#[test]
fn resolve_effective_model_returns_catalog_model_for_requested_pair() {
    // Arrange
    let provider = "unit-test-provider";
    let model_id = "unit-test-model";
    theway_llm_provider::register_custom_model(sample_model(model_id, provider));
    let startup = sample_model("startup", "provider-startup");
    let persisted = runtime_context(Path::new("/tmp"));

    // Act
    let model = resolve_effective_model(&startup, &persisted, Some(provider), Some(model_id), None)
        .unwrap();

    // Assert
    assert_eq!(model.id, model_id);
    assert_eq!(model.provider.0, provider);

    // Cleanup
    theway_llm_provider::unregister_custom_model(&Provider::from(provider), model_id);
}

#[test]
fn resolve_effective_model_rejects_unknown_model() {
    // Arrange
    let startup = sample_model("startup", "provider-startup");
    let persisted = runtime_context(Path::new("/tmp"));

    // Act
    let err = resolve_effective_model(
        &startup,
        &persisted,
        Some("no-such-provider"),
        Some("no-such-model"),
        None,
    )
    .unwrap_err();

    // Assert
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("model not found"));
}

#[test]
fn resolved_base_url_prefers_request_and_trims() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: Some("http://persisted".into()),
        thinking: None,
    };

    // Act
    let url = resolved_base_url(&persisted, Some("  http://requested  "));

    // Assert
    assert_eq!(url.as_deref(), Some("http://requested"));
}

#[test]
fn resolved_base_url_treats_blank_request_as_none() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: Some("http://persisted".into()),
        thinking: None,
    };

    // Act
    let url = resolved_base_url(&persisted, Some("   "));

    // Assert
    assert_eq!(url, None);
}

#[test]
fn resolved_base_url_falls_back_to_persisted_when_request_absent() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: Some("  http://persisted  ".into()),
        thinking: None,
    };

    // Act
    let url = resolved_base_url(&persisted, None);

    // Assert
    assert_eq!(url.as_deref(), Some("http://persisted"));
}

#[test]
fn resolve_thinking_prefers_request_over_persisted_and_startup() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: None,
        thinking: Some(false),
    };

    // Act
    let level = resolve_thinking(&ThinkingLevel::Off, &persisted, Some(true));

    // Assert
    assert_eq!(level, ThinkingLevel::High);
}

#[test]
fn resolve_thinking_uses_persisted_when_request_absent() {
    // Arrange
    let persisted = SessionRuntimeContext {
        work_dir: "/tmp".into(),
        provider: None,
        model: None,
        base_url: None,
        thinking: Some(false),
    };

    // Act
    let level = resolve_thinking(&ThinkingLevel::High, &persisted, None);

    // Assert
    assert_eq!(level, ThinkingLevel::Off);
}

#[test]
fn resolve_thinking_uses_startup_when_nothing_is_set() {
    // Arrange
    let persisted = runtime_context(Path::new("/tmp"));

    // Act
    let level = resolve_thinking(&ThinkingLevel::Low, &persisted, None);

    // Assert
    assert_eq!(level, ThinkingLevel::Low);
}
