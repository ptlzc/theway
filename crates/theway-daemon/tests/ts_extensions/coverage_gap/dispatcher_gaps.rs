//! `dispatcher` / `live_event` / `services` branch gaps: zero-limit host
//! configs, registration validation shapes, payload schema matching, service
//! name rules, and custom live-event name validation.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use theway_contract::extension::ExtensionLifecycleEvent;

use super::*;
use crate::ts_extensions::dispatcher::{
    RuntimeExtensionHostConfig, matches_schema, validate_registrations,
};
use crate::ts_extensions::live_event::validate_custom_event_name;
use crate::ts_extensions::services::ServiceRegistry;

// ─────────────────────────────────────────────────────────────────────────────
// event_bus error paths
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ts_extension_event_bus_returns_errors_when_any_listener_fails() {
    use crate::ts_extensions::event_bus::{
        LiveEventListener, bail_mode, emit_mode, parallel_mode, serial_mode,
    };

    let ok: LiveEventListener = Arc::new(|_| Box::pin(async move { Ok(Value::Null) }));
    let fail: LiveEventListener = Arc::new(|_| Box::pin(async move { Err("boom".into()) }));

    let emitted = emit_mode(&[ok.clone(), fail.clone()], Value::Null).await;
    assert_eq!(emitted, Err(vec!["boom".to_string()]));

    let parallel = parallel_mode(&[ok.clone(), fail.clone()], Value::Null).await;
    assert_eq!(parallel, Err(vec!["boom".to_string()]));

    let serial = serial_mode(&[ok, fail], Value::Null).await;
    assert_eq!(serial, Err(vec!["boom".to_string()]));

    let fail2: LiveEventListener = Arc::new(|_| Box::pin(async move { Err("boom".into()) }));
    let bail = bail_mode(&[fail2], Value::Null).await;
    assert_eq!(bail, Err(vec!["boom".to_string()]));
}

// ─────────────────────────────────────────────────────────────────────────────
// live_event validation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_live_event_validation_rejects_every_invalid_shape() {
    assert!(validate_custom_event_name("").is_err());
    assert!(validate_custom_event_name(&"e".repeat(129)).is_err());
    assert!(validate_custom_event_name("session/start").is_err());
    assert!(validate_custom_event_name("/starts-slash").is_err());
    assert!(validate_custom_event_name("ends-slash/").is_err());
    assert!(validate_custom_event_name("double//slash").is_err());
    assert!(validate_custom_event_name("UPPER").is_err());
    assert!(validate_custom_event_name("valid/name_1-2.3").is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// dispatcher validation branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_dispatcher_validate_rejects_every_zero_limit() {
    let defaults = RuntimeExtensionHostConfig::default();
    let cases: Vec<RuntimeExtensionHostConfig> = vec![
        RuntimeExtensionHostConfig { standard_deadline: Duration::ZERO, ..defaults.clone() },
        RuntimeExtensionHostConfig { long_deadline: Duration::ZERO, ..defaults.clone() },
        RuntimeExtensionHostConfig { broker_operation_quota: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { observation_queue_capacity: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { circuit_failure_threshold: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { max_durable_entries: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { max_durable_entry_bytes: 0, ..defaults.clone() },
        RuntimeExtensionHostConfig { max_extension_durable_bytes: 0, ..defaults.clone() },
    ];
    for config in cases {
        assert!(config.validate().is_err());
    }
}

#[test]
fn ts_extension_dispatcher_rejects_custom_event_with_non_observe_class() {
    let value = json!({"registrations": [{
        "registrationId": 1,
        "event": "custom/event",
        "descriptor": {"class": "gate"},
        "sequence": 2,
    }]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("custom live events only support observe"), "{err}");
}

#[test]
fn ts_extension_dispatcher_rejects_duplicate_allowed_actions() {
    let value = json!({"registrations": [
        hook_registration_value(
            ExtensionLifecycleEvent::Input,
            json!({"allowedActions": ["replace_input", "replace_input"]}),
        )
    ]});
    let err = validate_registrations(value).unwrap_err();
    assert!(err.contains("allowedActions"), "{err}");
}

#[test]
fn ts_extension_dispatcher_payload_schema_validation_branches() {
    // schema not object and not boolean
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": "oops"}))
    ]});
    assert!(validate_registrations(value).is_err());

    // object without type
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {}}))
    ]});
    assert!(validate_registrations(value).is_ok());

    // required not an array of strings
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "object", "required": [1]}}))
    ]});
    assert!(validate_registrations(value).is_err());

    // properties present and valid
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "object", "properties": {"x": {"type": "string"}}}}))
    ]});
    assert!(validate_registrations(value).is_ok());

    // properties not an object
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "object", "properties": 1}}))
    ]});
    assert!(validate_registrations(value).is_err());

    // items present and invalid
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "array", "items": 42}}))
    ]});
    assert!(validate_registrations(value).is_err());

    // items present and valid
    let value = json!({"registrations": [
        hook_registration_value(ExtensionLifecycleEvent::Input, json!({"payloadSchema": {"type": "array", "items": {"type": "string"}}}))
    ]});
    assert!(validate_registrations(value).is_ok());
}

#[test]
fn ts_extension_dispatcher_matches_schema_branches() {
    // non-object non-boolean schema
    assert!(!matches_schema(&json!(42), &json!(42)));
    // object schema without type
    assert!(matches_schema(&json!({"required": ["x"]}), &json!({"x": 1})));
    // integer accepts signed and unsigned integers, rejects floats
    assert!(matches_schema(&json!({"type": "integer"}), &json!(1)));
    assert!(matches_schema(&json!({"type": "integer"}), &json!(1_u64)));
    assert!(!matches_schema(&json!({"type": "integer"}), &json!(1.5)));
    // required with non-object value
    assert!(!matches_schema(&json!({"required": ["x"]}), &json!(1)));
}

// ─────────────────────────────────────────────────────────────────────────────
// services branches
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ts_extension_services_rejects_every_invalid_name_shape() {
    let registry = ServiceRegistry::new();
    assert!(registry.provide("s", "ext", &"a".repeat(65), &json!({})).is_err());
    assert!(registry.provide("s", "ext", "-bad", &json!({})).is_err());
    assert!(registry.provide("s", "ext", "bad-", &json!({})).is_err());
    assert!(registry.provide("s", "ext", "UPPER", &json!({})).is_err());
}

#[test]
fn ts_extension_services_same_owner_overwrites_and_other_session_keeps() {
    let registry = ServiceRegistry::new();
    registry.provide("s1", "ext", "a", &json!(1)).unwrap();
    registry.provide("s1", "ext", "a", &json!(2)).unwrap();
    assert_eq!(registry.get("s1", "a").unwrap(), json!(2));

    registry.provide("s2", "ext", "a", &json!(3)).unwrap();
    let removed = registry.dispose_owner("s1", "ext");
    assert_eq!(removed, vec!["a".to_string()]);
    assert!(registry.get("s1", "a").is_none());
    assert_eq!(registry.get("s2", "a").unwrap(), json!(3));
}
