use serde_json::json;
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionStateMutation,
};
use theway_contract::session::{
    JsonlSessionMetadata, SessionBinding, SessionErrorCode, SessionMetadata, SessionRuntimeContext,
    StoredSessionEntry, validate_session_entries,
};

fn entry(payload: serde_json::Value) -> StoredSessionEntry {
    StoredSessionEntry::from_payload(payload).unwrap()
}

#[test]
fn stored_entry_extracts_indexes_without_changing_payload() {
    let payload = json!({
        "type": "message",
        "id": "m1",
        "parentId": null,
        "timestamp": "2024-01-01T00:00:00Z",
        "message": { "role": "user", "content": "hello", "timestamp": 1 }
    });

    let stored = StoredSessionEntry::from_payload(payload.clone()).unwrap();

    assert_eq!(stored.id, "m1");
    assert_eq!(stored.entry_type, "message");
    assert_eq!(stored.parent_id, None);
    assert_eq!(stored.payload, payload);
}

#[test]
fn stored_entry_rejects_unknown_or_malformed_shapes() {
    let unknown = StoredSessionEntry::from_payload(json!({
        "type": "future",
        "id": "x",
        "parentId": null,
        "timestamp": "2024-01-01T00:00:00Z"
    }))
    .unwrap_err();
    let malformed = StoredSessionEntry::from_payload(json!({
        "type": "model_change",
        "id": "x",
        "parentId": null,
        "timestamp": "2024-01-01T00:00:00Z",
        "provider": "faux"
    }))
    .unwrap_err();

    assert_eq!(unknown.code, SessionErrorCode::Corrupted);
    assert_eq!(malformed.code, SessionErrorCode::Corrupted);
}

#[test]
fn validate_entries_replays_leaf_and_rejects_dangling_references() {
    let message = entry(json!({
        "type": "message",
        "id": "m1",
        "parentId": null,
        "timestamp": "2024-01-01T00:00:00Z",
        "message": { "role": "user", "content": "hello", "timestamp": 1 }
    }));
    let leaf = StoredSessionEntry::leaf(
        "l1".into(),
        Some("m1".into()),
        "2024-01-01T00:00:01Z".into(),
        None,
    )
    .unwrap();
    let dangling = entry(json!({
        "type": "session_info",
        "id": "s1",
        "parentId": "missing",
        "timestamp": "2024-01-01T00:00:02Z",
        "name": "bad"
    }));

    assert_eq!(validate_session_entries(&[message, leaf]).unwrap(), None);
    assert_eq!(
        validate_session_entries(&[dangling]).unwrap_err().code,
        SessionErrorCode::Corrupted
    );
}

fn jsonl_metadata_without_binding() -> JsonlSessionMetadata {
    JsonlSessionMetadata {
        base: SessionMetadata {
            id: "session-1".into(),
            created_at: "2026-08-24T00:00:00Z".into(),
        },
        cwd: "/work".into(),
        path: "/sessions/session-1.db".into(),
        parent_session_path: None,
        imported_from: None,
        binding: None,
    }
}

fn bound_metadata() -> JsonlSessionMetadata {
    let mut metadata = jsonl_metadata_without_binding();
    metadata.binding = Some(SessionBinding {
        client_key: "client-1".into(),
        runtime: SessionRuntimeContext {
            work_dir: "/work".into(),
            provider: Some("provider".into()),
            model: Some("model".into()),
            base_url: Some("https://example.com".into()),
            thinking: Some(true),
        },
    });
    metadata
}

#[test]
fn legacy_metadata_without_binding_decodes_to_none() {
    let json = json!({
        "id": "session-1",
        "createdAt": "2026-08-24T00:00:00Z",
        "cwd": "/work",
        "path": "/sessions/session-1.db"
    });

    let metadata: JsonlSessionMetadata = serde_json::from_value(json).unwrap();

    assert_eq!(metadata.base.id, "session-1");
    assert_eq!(metadata.binding, None);
}

#[test]
fn bound_metadata_round_trips_with_camel_case_fields() {
    let metadata = bound_metadata();

    let json = serde_json::to_value(&metadata).unwrap();
    let decoded: JsonlSessionMetadata = serde_json::from_value(json.clone()).unwrap();

    assert_eq!(serde_json::to_value(&decoded).unwrap(), json);
    assert_eq!(json["binding"]["clientKey"], "client-1");
    assert_eq!(json["binding"]["runtime"]["workDir"], "/work");
    assert_eq!(json["binding"]["runtime"]["baseUrl"], "https://example.com");
}

#[test]
fn serialized_binding_contains_no_secret_keys() {
    let metadata = bound_metadata();
    let json = serde_json::to_value(&metadata).unwrap();
    let text = json.to_string();

    for key in ["secret", "apiKey", "credential", "token", "password"] {
        assert!(
            !text.to_ascii_lowercase().contains(key),
            "serialized metadata must not contain {key:?}"
        );
    }
}

#[test]
fn binding_debug_output_contains_no_secret_material() {
    let metadata = bound_metadata();
    let debug = format!("{:?}", metadata.binding.as_ref().unwrap());
    let lower = debug.to_ascii_lowercase();

    for key in ["secret", "apiKey", "credential", "token", "password"] {
        assert!(
            !lower.contains(key),
            "binding debug output must not contain {key:?}"
        );
    }
}

#[test]
fn stored_extension_entry_validates_and_decodes_the_public_envelope() {
    let durable = ExtensionDurableEntry {
        extension_id: "deepseek-anchor".into(),
        state_schema_version: 1,
        origin_sequence: 3,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: "phase".into(),
            mutation: ExtensionStateMutation::Set {
                value: json!("promoted"),
            },
        },
    };

    let stored = StoredSessionEntry::extension(
        "e1".into(),
        Some("m1".into()),
        "2026-08-20T00:00:00Z".into(),
        durable.clone(),
    )
    .unwrap();
    let decoded = stored.extension_payload().unwrap().unwrap();

    assert_eq!(stored.entry_type, "extension");
    assert_eq!(decoded, durable);
}
