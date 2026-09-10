//! Host shutdown, registration, effect-ledger, and command-predicate branch
//! gaps: post-shutdown invokes, subscription bookkeeping, provider/name/text
//! validation, disposed handles, and availability mismatch errors.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};
use theway_contract::extension::{ExtensionHookClass, ExtensionLifecycleEvent, ExtensionPermission};
use theway_core::agent::runtime_extensions::{
    NoopSessionExtensionStatePort, RuntimeExtensionContext, RuntimeExtensionInvocation,
};

use super::*;
use crate::ts_extensions::catalog::PackageCatalog;
use crate::ts_extensions::dispatcher::{RuntimeExtensionHostConfig, validate_registrations};
use crate::ts_extensions::engine::QuickJsEnginePool;
use crate::ts_extensions::host::SessionPluginHost;

// ─────────────────────────────────────────────────────────────────────────────
// host shutdown/early-exit branches
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_host_invoke_after_shutdown_returns_empty() {
    let cwd = tempfile::tempdir().unwrap();
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "coverage-gap-session",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await;
    host.shutdown().await;
    let outputs = host
        .invoke(ExtensionLifecycleEvent::Input, json!({"message": {}}))
        .await;
    assert!(outputs.is_empty());
}

#[tokio::test]
async fn ts_extension_host_publish_live_event_after_shutdown_returns_null() {
    let cwd = tempfile::tempdir().unwrap();
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "coverage-gap-session",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await;
    host.shutdown().await;
    let result = host.publish_live_event("custom/event", json!({}), "emit").await.unwrap();
    assert_eq!(result, Value::Null);
}

#[tokio::test]
async fn ts_extension_host_subscription_counts_and_shutdown_invoke_runtime() {
    let cwd = tempfile::tempdir().unwrap();
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "coverage-gap-session",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await;

    let hooks = validate_registrations(json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({}))
    ]}))
    .unwrap();
    host.add_subscriptions(&hooks);
    assert!(host.has_subscription(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform
    ));
    host.remove_subscriptions(&hooks);
    assert!(!host.has_subscription(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform
    ));

    host.shutdown().await;
    let invocation = RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        RuntimeExtensionContext::new("coverage-gap-session", "/cwd", 1),
        json!({}),
    )
    .unwrap();
    let result = host.invoke_runtime(invocation).await.unwrap();
    assert!(result.actions.is_empty());
}

// ─────────────────────────────────────────────────────────────────────────────
// registrations provider/name/text branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_registrations_provider_validation_branches() {
    use crate::ts_extensions::registrations::validate_effect_registrations;

    let granted = BTreeSet::from([ExtensionPermission::ProvidersRegister]);
    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "http://",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("http")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": []
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("1-256")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [
                {"id": "dup", "name": "A", "contextWindow": 100, "maxTokens": 50},
                {"id": "dup", "name": "B", "contextWindow": 100, "maxTokens": 50}
            ]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 0, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 0}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 101}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("invalid")), "{:?}", validated.errors);

    let metadata = json!({"effects": [{
        "registrationId": 1,
        "kind": "provider",
        "descriptor": {
            "providerId": "acme",
            "baseUrl": "https://example.com",
            "format": "openai_responses",
            "credentialRef": "bad ref",
            "models": [{"id": "m", "name": "M", "contextWindow": 100, "maxTokens": 50}]
        },
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&metadata, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("credentialRef")), "{:?}", validated.errors);
}

#[test]
fn ts_extension_registrations_name_and_text_limits() {
    use crate::ts_extensions::registrations::validate_effect_registrations;
    let granted = BTreeSet::from([ExtensionPermission::ToolsRegister]);

    let empty_name = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "", "label": "L", "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&empty_name, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("name")), "{:?}", validated.errors);

    let long_name = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "a".repeat(129), "label": "L", "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&long_name, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("name")), "{:?}", validated.errors);

    let long_text = json!({"effects": [{
        "registrationId": 1,
        "kind": "tool",
        "descriptor": {"name": "ok", "label": "x".repeat(17 * 1024), "description": "D", "inputSchema": {}},
        "sequence": 1
    }]});
    let validated =
        validate_effect_registrations(&long_text, "ext", theway_contract::extension::ExtensionScope::Session, &granted)
            .unwrap();
    assert!(validated.errors.iter().any(|e| e.contains("label")), "{:?}", validated.errors);
}

// ─────────────────────────────────────────────────────────────────────────────
// effects branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_effects_set_restoration_data_on_disposed_handle_fails() {
    use crate::ts_extensions::effects::{EffectLedger, EffectScopeBinding};
    use crate::ts_extensions::registrations::hook_effect_registration;

    let ledger = EffectLedger::default();
    let registration = hook_effect_registration(1, 1, "key".into(), theway_contract::extension::ExtensionScope::Session);
    let handle = ledger
        .accept(
            crate::ts_extensions::effects::EffectOwner {
                extension_id: "ext".into(),
                session_id: "sess".into(),
            },
            EffectScopeBinding::setup(theway_contract::extension::ExtensionScope::Session),
            registration,
            false,
        )
        .unwrap();
    ledger.dispose(handle).unwrap();
    let err = ledger.set_restoration_data(handle, json!({"x": 1})).unwrap_err();
    assert_eq!(err, crate::ts_extensions::effects::EffectLedgerError::DisposedHandle);
}

// ─────────────────────────────────────────────────────────────────────────────
// registration_runtime command predicate branches
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_registration_runtime_command_predicate_mismatch_errors() {
    use crate::ts_extensions::effects::EffectOwner;
    use crate::ts_extensions::registration_runtime::RegistrationRuntime;
    use crate::ts_extensions::registrations::{
        CommandRegistration, EffectRegistration, OwnedRegistration, RegistrationPredicate,
    };

    let runtime = RegistrationRuntime::default();
    let owner = EffectOwner {
        extension_id: "ext".into(),
        session_id: "sess".into(),
    };
    let registration = EffectRegistration {
        registration_id: 1,
        sequence: 1,
        value: OwnedRegistration::Command(CommandRegistration {
            command: theway_contract::extension::ExtensionCommandDescriptor {
                name: "cmd".into(),
                label: "Cmd".into(),
                description: "desc".into(),
                argument_schema: json!({}),
            },
            availability: RegistrationPredicate {
                providers: BTreeSet::from(["openai".into()]),
                models: BTreeSet::new(),
                requires_interactive_client: false,
            },
            scope: theway_contract::extension::ExtensionScope::Session,
        }),
    };
    let accepted = runtime.accept_all(
        owner.clone(),
        vec![registration],
        &QuickJsEnginePool::new(1),
    );
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);

    let result = runtime
        .invoke_command(
            &QuickJsEnginePool::new(1),
            "/cwd",
            "cmd",
            json!({}),
            &crate::ts_extensions::registration_runtime::ExtensionCommandContext {
                provider: "anthropic".into(),
                model: "claude".into(),
                has_interactive_client: false,
            },
        )
        .await;
    assert!(result.is_err());
}
