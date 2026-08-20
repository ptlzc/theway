use serde_json::json;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionDurableEntry, ExtensionDurableEntryKind,
    ExtensionDurableEntryPayload, ExtensionModelContextPlacement, ExtensionStateMutation,
    ExtensionStateValidationError,
};

fn durable(entry: ExtensionDurableEntryPayload) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        abi_major: ExtensionAbiMajor::V2,
        extension_id: "deepseek-anchor".into(),
        state_schema_version: 2,
        origin_sequence: 7,
        entry,
    }
}

#[test]
fn durable_state_set_and_tombstone_round_trip() {
    for mutation in [
        ExtensionStateMutation::Set {
            value: json!({"phase": "promoted"}),
        },
        ExtensionStateMutation::Delete,
    ] {
        let entry = durable(ExtensionDurableEntryPayload::StateMutation {
            key: "phase".into(),
            mutation,
        });

        entry.validate().unwrap();
        let encoded = serde_json::to_value(&entry).unwrap();
        let decoded: ExtensionDurableEntry = serde_json::from_value(encoded).unwrap();

        assert_eq!(decoded, entry);
        assert_eq!(
            decoded.entry.kind(),
            ExtensionDurableEntryKind::StateMutation
        );
    }
}

#[test]
fn durable_custom_event_is_namespaced_and_has_no_visibility_switch() {
    let entry = durable(ExtensionDurableEntryPayload::CustomEvent {
        event_id: "decision-7".into(),
        custom_type: "policy.decision".into(),
        payload: json!({"outcome": "allow"}),
    });

    entry.validate().unwrap();
    let encoded = serde_json::to_value(&entry).unwrap();

    assert_eq!(encoded["entry"]["kind"], "custom_event");
    assert!(encoded["entry"].get("display").is_none());
    assert!(encoded["entry"].get("modelVisible").is_none());
}

#[test]
fn durable_model_context_requires_content_matching_placement() {
    let system = durable(ExtensionDurableEntryPayload::ModelContext {
        context_id: "anchor-context".into(),
        placement: ExtensionModelContextPlacement::SystemPromptSection,
        content: json!("restored instructions"),
    });
    let invalid = durable(ExtensionDurableEntryPayload::ModelContext {
        context_id: "anchor-message".into(),
        placement: ExtensionModelContextPlacement::Message,
        content: json!("not a message object"),
    });

    system.validate().unwrap();
    assert_eq!(
        invalid.validate(),
        Err(ExtensionStateValidationError::InvalidModelContext)
    );
}

#[test]
fn durable_migration_requires_forward_version_matching_envelope() {
    let valid = durable(ExtensionDurableEntryPayload::StateMigration {
        from_schema_version: 1,
        to_schema_version: 2,
    });
    let invalid = durable(ExtensionDurableEntryPayload::StateMigration {
        from_schema_version: 2,
        to_schema_version: 3,
    });

    valid.validate().unwrap();
    assert_eq!(
        invalid.validate(),
        Err(ExtensionStateValidationError::InvalidMigration)
    );
}

#[test]
fn durable_entry_rejects_invalid_namespace_schema_sequence_and_key() {
    let mut entry = durable(ExtensionDurableEntryPayload::StateMutation {
        key: "phase".into(),
        mutation: ExtensionStateMutation::Delete,
    });
    entry.extension_id = "Invalid".into();
    assert_eq!(
        entry.validate(),
        Err(ExtensionStateValidationError::InvalidExtensionId)
    );

    let mut entry = durable(ExtensionDurableEntryPayload::StateMutation {
        key: "phase".into(),
        mutation: ExtensionStateMutation::Delete,
    });
    entry.state_schema_version = 0;
    assert_eq!(
        entry.validate(),
        Err(ExtensionStateValidationError::InvalidStateSchema)
    );

    let mut entry = durable(ExtensionDurableEntryPayload::StateMutation {
        key: "phase".into(),
        mutation: ExtensionStateMutation::Delete,
    });
    entry.origin_sequence = 0;
    assert_eq!(
        entry.validate(),
        Err(ExtensionStateValidationError::InvalidOriginSequence)
    );

    let entry = durable(ExtensionDurableEntryPayload::StateMutation {
        key: "\n".into(),
        mutation: ExtensionStateMutation::Delete,
    });
    assert_eq!(
        entry.validate(),
        Err(ExtensionStateValidationError::InvalidStateKey)
    );
}
