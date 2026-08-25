use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::json;
use theway_contract::extension::{
    ExtensionActionBatch, ExtensionActionKind, ExtensionHookClass, ExtensionLifecycleEvent,
    ExtensionPermission,
};
use theway_core::agent::runtime_extensions::RuntimeExtensionInvocation;

use super::super::dispatcher::{
    RuntimeExtensionHostConfig, envelope, matches_schema, runtime_envelope,
    runtime_envelope_with_payload, validate_action_capabilities, validate_registration_capabilities,
    validate_registrations,
};

fn hook_registration_value(event: ExtensionLifecycleEvent, extra: serde_json::Value) -> serde_json::Value {
    json!({
        "registrationId": 1,
        "event": event,
        "descriptor": extra,
        "sequence": 2,
    })
}

#[test]
fn config_validate_rejects_zero_deadlines_and_limits() {
    let mut config = RuntimeExtensionHostConfig::default();
    config.fast_deadline = Duration::ZERO;
    assert!(config.validate().is_err());

    let mut config = RuntimeExtensionHostConfig::default();
    config.max_actions = 0;
    assert!(config.validate().is_err());

    assert!(RuntimeExtensionHostConfig::default().validate().is_ok());
}

#[test]
fn validate_registrations_requires_registrations_field() {
    let err = validate_registrations(json!({})).unwrap_err();
    assert!(err.contains("did not return registrations"), "{err}");
}

#[test]
fn validate_registrations_rejects_invalid_json_and_duplicate_ids() {
    let err = validate_registrations(json!({"registrations": "nope"})).unwrap_err();
    assert!(err.contains("invalid"), "{err}");

    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({})),
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({})),
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("unique"), "{err}");
}

#[test]
fn validate_registrations_rejects_noncanonical_abi_fields() {
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({
            "allowedActions": ["set_state"]
        }))
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("allowedActions"), "{err}");

    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({
            "deadline": "long"
        }))
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("deadline"), "{err}");

    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({
            "delivery": "bounded_coalescing"
        }))
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("delivery"), "{err}");

    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({
            "failure": "deny"
        }))
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("failure"), "{err}");
}

#[test]
fn validate_registrations_rejects_bad_priority_and_schema() {
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"priority": 2_000_000}))
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("priority"), "{err}");

    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({
            "payloadSchema": {"type": "wat"}
        }))
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("type"), "{err}");
}

#[test]
fn validate_registrations_sorts_by_event_class_priority_sequence() {
    let value = json!({"registrations": [
        json!({
            "registrationId": 1,
            "event": "input",
            "descriptor": {"priority": 10},
            "sequence": 1
        }),
        json!({
            "registrationId": 2,
            "event": "input",
            "descriptor": {"priority": 5},
            "sequence": 2
        }),
    ]});
    let registrations = validate_registrations(value).unwrap();
    assert_eq!(registrations.len(), 2);
    assert_eq!(registrations[0].priority, 10);
    assert_eq!(registrations[1].priority, 5);
}

#[test]
fn validate_registration_capabilities_requires_provider_raw() {
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::BeforeProviderRequestHeaders, json!({}))
    ]});
    let registrations = validate_registrations(value).unwrap();
    let err = validate_registration_capabilities(&registrations, &BTreeSet::new()).unwrap_err();
    assert!(err.contains("provider.raw"), "{err}");
    assert!(validate_registration_capabilities(
        &registrations,
        &BTreeSet::from([ExtensionPermission::ProviderRaw])
    )
    .is_ok());
}

#[test]
fn validate_action_capabilities_checks_session_write() {
    let batch = ExtensionActionBatch {
        decision: None,
        actions: vec![theway_contract::extension::ExtensionAction {
            kind: ExtensionActionKind::SetState,
            payload: json!({}),
        }],
    };
    let err = validate_action_capabilities(&batch, &BTreeSet::new()).unwrap_err();
    assert!(err.contains("session.write"), "{err}");
    assert!(validate_action_capabilities(
        &batch,
        &BTreeSet::from([ExtensionPermission::SessionWrite])
    )
    .is_ok());
}

#[test]
fn matches_schema_handles_boolean_and_object_schemas() {
    assert!(matches_schema(&json!(true), &json!([])));
    assert!(!matches_schema(&json!(false), &json!([])));
    assert!(!matches_schema(&json!({"type": "object"}), &json!([])));

    let schema = json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "additionalProperties": false
    });
    assert!(matches_schema(&schema, &json!({"name": "x", "age": 3})));
    assert!(!matches_schema(&schema, &json!({"name": 1})));
    assert!(!matches_schema(&schema, &json!({"name": "x", "extra": true})));
    assert!(!matches_schema(&schema, &json!({"age": 3})));
}

#[test]
fn matches_schema_validates_items_and_unknown_type() {
    let schema = json!({"type": "array", "items": {"type": "string"}});
    assert!(matches_schema(&schema, &json!(["a", "b"])));
    assert!(!matches_schema(&schema, &json!(["a", 1])));

    assert!(!matches_schema(&json!({"type": "nope"}), &json!(1)));
}

#[test]
fn envelope_builds_expected_event_context() {
    let envelope = envelope("ext", "sess", "/cwd", 42, ExtensionLifecycleEvent::Input, json!({"a": 1}));
    assert_eq!(envelope.event, ExtensionLifecycleEvent::Input);
    assert_eq!(envelope.context.extension_id, "ext");
    assert_eq!(envelope.context.session_id, "sess");
    assert_eq!(envelope.context.cwd, "/cwd");
    assert_eq!(envelope.context.sequence, 42);
    assert_eq!(envelope.payload, json!({"a": 1}));
}

#[test]
fn runtime_envelope_preserves_invocation_context_and_payload() {
    let mut context = theway_core::agent::runtime_extensions::RuntimeExtensionContext::new(
        "sess",
        "/cwd",
        9,
    );
    context.scope.run_id = Some("run".into());
    context.model = Some(theway_contract::extension::ExtensionModelRef {
        provider: "p".into(),
        model: "m".into(),
    });
    context.has_interactive_client = true;
    let invocation = RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        context,
        json!({"hello": "world"}),
    )
    .unwrap();

    let envelope = runtime_envelope("ext", &invocation);
    assert_eq!(envelope.context.extension_id, "ext");
    assert_eq!(envelope.context.session_id, "sess");
    assert_eq!(envelope.context.cwd, "/cwd");
    assert_eq!(envelope.context.sequence, 9);
    assert_eq!(
        envelope.context.model.as_ref().map(|m| m.model.as_str()),
        Some("m")
    );
    assert!(envelope.context.has_interactive_client);
    assert_eq!(envelope.payload, json!({"hello": "world"}));

    let custom = json!({"changed": true});
    let envelope = runtime_envelope_with_payload("ext", &invocation, custom.clone());
    assert_eq!(envelope.payload, custom);
}
