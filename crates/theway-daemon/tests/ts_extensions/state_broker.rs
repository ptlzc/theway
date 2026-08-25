use serde_json::json;
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionStateMutation,
};

use super::super::engine::EngineInstanceKey;
use super::super::state_broker::ExtensionStateBroker;

fn key() -> EngineInstanceKey {
    EngineInstanceKey::new("sess", "ext")
}

fn set_entry(key_name: &str, value: serde_json::Value) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        extension_id: "ext".into(),
        state_schema_version: 1,
        origin_sequence: 1,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: key_name.into(),
            mutation: ExtensionStateMutation::Set { value },
        },
    }
}

#[test]
fn install_schema_and_state_get() {
    let broker = ExtensionStateBroker::default();
    broker.install(&key(), Some(3), &[set_entry("a", json!(1))]);

    assert_eq!(
        broker.call(&key(), "state.schema", "").unwrap(),
        json!(3)
    );
    assert_eq!(
        broker.call(&key(), "state.get", r#"{"key":"a"}"#).unwrap(),
        json!(1)
    );
    assert_eq!(
        broker.call(&key(), "state.get", r#"{"key":"missing"}"#).unwrap(),
        json!(null)
    );
}

#[test]
fn schema_requires_declared_schema_version() {
    let broker = ExtensionStateBroker::default();
    let err = broker.call(&key(), "state.schema", "").unwrap_err();
    assert_eq!(err.code, "state_schema_required");

    let err = broker.call(&key(), "state.get", r#"{"key":"a"}"#).unwrap_err();
    assert_eq!(err.code, "state_schema_required");
}

#[test]
fn events_replay_filters_by_custom_type() {
    let broker = ExtensionStateBroker::default();
    broker.install(
        &key(),
        Some(1),
        &[
            ExtensionDurableEntry {
                extension_id: "ext".into(),
                state_schema_version: 1,
                origin_sequence: 1,
                entry: ExtensionDurableEntryPayload::CustomEvent {
                    event_id: "e1".into(),
                    custom_type: "click".into(),
                    payload: json!({"x": 1}),
                },
            },
            ExtensionDurableEntry {
                extension_id: "ext".into(),
                state_schema_version: 1,
                origin_sequence: 2,
                entry: ExtensionDurableEntryPayload::CustomEvent {
                    event_id: "e2".into(),
                    custom_type: "hover".into(),
                    payload: json!({"y": 2}),
                },
            },
        ],
    );

    let all = broker.call(&key(), "events.replay", "{}").unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);
    let clicks = broker
        .call(&key(), "events.replay", r#"{"customType":"click"}"#)
        .unwrap();
    assert_eq!(clicks.as_array().unwrap().len(), 1);
    assert_eq!(clicks[0]["type"], "click");
}

#[test]
fn memory_operations_are_ephemeral_and_clearable() {
    let broker = ExtensionStateBroker::default();
    assert_eq!(
        broker.call(&key(), "memory.get", r#"{"key":"a"}"#).unwrap(),
        json!(null)
    );
    broker.call(&key(), "memory.set", r#"{"key":"a","value":{"n":1}}"#).unwrap();
    assert_eq!(
        broker.call(&key(), "memory.get", r#"{"key":"a"}"#).unwrap(),
        json!({"n": 1})
    );
    broker.call(&key(), "memory.delete", r#"{"key":"a"}"#).unwrap();
    assert_eq!(
        broker.call(&key(), "memory.get", r#"{"key":"a"}"#).unwrap(),
        json!(null)
    );
    broker.call(&key(), "memory.set", r#"{"key":"a","value":1}"#).unwrap();
    broker.call(&key(), "memory.clear", "").unwrap();
    assert_eq!(
        broker.call(&key(), "memory.get", r#"{"key":"a"}"#).unwrap(),
        json!(null)
    );
}

#[test]
fn memory_delete_missing_key_is_noop() {
    let broker = ExtensionStateBroker::default();
    broker.call(&key(), "memory.delete", r#"{"key":"a"}"#).unwrap();
}

#[test]
fn call_rejects_unknown_operation_and_invalid_arguments_and_keys() {
    let broker = ExtensionStateBroker::default();
    let err = broker.call(&key(), "unknown", "").unwrap_err();
    assert_eq!(err.code, "invalid_arguments");

    let err = broker.call(&key(), "state.get", "not-json").unwrap_err();
    assert_eq!(err.code, "invalid_arguments");

    let err = broker.call(&key(), "memory.get", r#"{"key":""}"#).unwrap_err();
    assert_eq!(err.code, "invalid_arguments");

    let err = broker
        .call(&key(), "memory.get", &format!(r#"{{"key":"{}"}}"#, "a".repeat(257)))
        .unwrap_err();
    assert_eq!(err.code, "invalid_arguments");

    let err = broker
        .call(&key(), "memory.get", r#"{"key":"a\u0000"}"#)
        .unwrap_err();
    assert_eq!(err.code, "invalid_arguments");
}

#[test]
fn apply_only_updates_existing_durable_projection() {
    let broker = ExtensionStateBroker::default();
    broker.apply(&key(), &[set_entry("a", json!(2))]);

    broker.install(&key(), Some(1), &[set_entry("a", json!(1))]);
    broker.apply(&key(), &[set_entry("a", json!(2))]);
    assert_eq!(
        broker.call(&key(), "state.get", r#"{"key":"a"}"#).unwrap(),
        json!(2)
    );
}

#[test]
fn clear_memory_removes_ephemeral_state() {
    let broker = ExtensionStateBroker::default();
    broker.call(&key(), "memory.set", r#"{"key":"a","value":1}"#).unwrap();
    broker.clear_memory(&key());
    assert_eq!(
        broker.call(&key(), "memory.get", r#"{"key":"a"}"#).unwrap(),
        json!(null)
    );
}
