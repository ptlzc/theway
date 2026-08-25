
use theway_contract::extension::{ExtensionDiagnostic, ExtensionDiagnosticCode, ExtensionDurableEntry};

use super::super::broker_services::ExtensionBrokerServices;
use super::super::engine::EngineInstanceKey;

#[test]
fn secrets_set_get_has() {
    let services = ExtensionBrokerServices::new(std::path::Path::new("/tmp/way-test"), crate::executor::default_executor());
    assert!(!services.has_secret("key"));
    services.set_secret("key", "value");
    assert!(services.has_secret("key"));
    assert_eq!(services.secret("key").as_deref(), Some("value"));
    assert_eq!(services.secret("missing"), None);
}

#[test]
fn diagnostics_for_filters_by_session() {
    let services = ExtensionBrokerServices::new(std::path::Path::new("/tmp/way-test"), crate::executor::default_executor());
    let mut diagnostic = ExtensionDiagnostic::new(
        "ext".to_string(),
        ExtensionDiagnosticCode::HookFailed,
        theway_contract::extension::ExtensionDiagnosticSeverity::Error,
        "boom",
    );
    diagnostic.session_id = Some("s1".into());
    services.diagnostics.lock().push(diagnostic);
    let mut other = ExtensionDiagnostic::new(
        "ext".to_string(),
        ExtensionDiagnosticCode::HookFailed,
        theway_contract::extension::ExtensionDiagnosticSeverity::Error,
        "boom",
    );
    other.session_id = Some("s2".into());
    services.diagnostics.lock().push(other);

    let diagnostics = services.diagnostics_for("s1");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].session_id.as_deref(), Some("s1"));
}

#[test]
fn install_apply_clear_state_roundtrip() {
    let services = ExtensionBrokerServices::new(std::path::Path::new("/tmp/way-test"), crate::executor::default_executor());
    let key = EngineInstanceKey::new("sess", "ext");
    services.install_state(&key, Some(1), &[]);
    assert_eq!(
        services.state.call(&key, "state.schema", "").unwrap(),
        serde_json::json!(1)
    );
    let entry = ExtensionDurableEntry {
        extension_id: "ext".into(),
        state_schema_version: 1,
        origin_sequence: 1,
        entry: theway_contract::extension::ExtensionDurableEntryPayload::StateMutation {
            key: "k".into(),
            mutation: theway_contract::extension::ExtensionStateMutation::Set {
                value: serde_json::json!(1),
            },
        },
    };
    services.apply_state(&key, &[entry]);
    assert_eq!(
        services.state.call(&key, "state.get", r#"{"key":"k"}"#).unwrap(),
        serde_json::json!(1)
    );
    services.clear_memory(&key);
}

#[test]
fn block_on_without_runtime_builds_current_thread_runtime() {
    let services = ExtensionBrokerServices::new(std::path::Path::new("/tmp/way-test"), crate::executor::default_executor());
    let result = services
        .block_on(async { Ok::<_, String>(42) })
        .unwrap();
    assert_eq!(result, 42);

    let err = services
        .block_on(async { Err::<(), _>("nope".to_string()) })
        .unwrap_err();
    assert_eq!(err, "nope");
}
