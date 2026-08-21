use serde_json::json;
use theway_contract::extension::{
    ExtensionCatalogEntry, ExtensionCatalogStatus, ExtensionDiagnostic, ExtensionDiagnosticCode,
    ExtensionDiagnosticSensitivity, ExtensionDiagnosticSeverity, ExtensionPermission,
    ExtensionScope, ExtensionSourceLayer,
};

#[test]
fn diagnostic_sensitive_detail_is_discarded_before_serialization() {
    let mut diagnostic = ExtensionDiagnostic::new(
        "deepseek-anchor",
        ExtensionDiagnosticCode::PermissionDenied,
        ExtensionDiagnosticSeverity::Error,
        "provider request was denied",
    );
    diagnostic.add_detail(
        "provider",
        json!("deepseek"),
        ExtensionDiagnosticSensitivity::Public,
    );
    diagnostic.add_detail(
        "authorization",
        json!("Bearer top-secret-token"),
        ExtensionDiagnosticSensitivity::Sensitive,
    );

    let encoded = serde_json::to_string(&diagnostic).unwrap();

    assert!(encoded.contains("deepseek"));
    assert!(encoded.contains("authorization"));
    assert!(!encoded.contains("top-secret-token"));
    assert!(!diagnostic.details.contains_key("authorization"));
    assert!(diagnostic.redacted_fields.contains("authorization"));
}

#[test]
fn catalog_entry_round_trips_without_engine_or_secret_values() {
    let entry = ExtensionCatalogEntry {
        extension_id: "deepseek-anchor".into(),
        version: "1.0.0".into(),
        source: ExtensionSourceLayer::Project,
        scope: ExtensionScope::Session,
        priority: 100,
        status: ExtensionCatalogStatus::Blocked,
        permissions: vec![ExtensionPermission::SessionWrite],
        reason_code: Some(ExtensionDiagnosticCode::TrustRequired),
    };

    let encoded = serde_json::to_value(&entry).unwrap();
    let decoded: ExtensionCatalogEntry = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, entry);
    assert_eq!(encoded["status"], "blocked");
    assert!(encoded.get("engine").is_none());
    assert!(encoded.get("credential").is_none());
}
