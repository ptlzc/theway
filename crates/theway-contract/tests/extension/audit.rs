use std::collections::BTreeSet;

use theway_contract::extension::{
    ExtensionAuditEvent, ExtensionAuditOperation, ExtensionAuditOutcome, ExtensionPermission,
};

#[test]
fn audit_contract_has_no_payload_or_secret_value_field() {
    let event = ExtensionAuditEvent {
        timestamp: "2026-08-20T00:00:00Z".into(),
        extension_id: "security-test".into(),
        session_id: Some("session".into()),
        operation: ExtensionAuditOperation::SecretRead,
        outcome: ExtensionAuditOutcome::Succeeded,
        capability: Some(ExtensionPermission::SecretsRead("github".into())),
        target: Some("github".into()),
        redacted_fields: BTreeSet::from(["value".into()]),
    };
    let value = serde_json::to_value(event).unwrap();
    assert_eq!(value["operation"], "secret_read");
    assert_eq!(value["redactedFields"], serde_json::json!(["value"]));
    assert!(value.get("payload").is_none());
    assert!(value.get("value").is_none());
    assert!(value.get("arguments").is_none());
}
