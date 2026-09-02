use serde_json::json;
use theway_contract::extension::{
    ExtensionManifestError, ExtensionPackageManifest, ExtensionPermission, ExtensionScope,
    ExtensionSourceLayer,
};

fn valid_manifest() -> ExtensionPackageManifest {
    serde_json::from_value(json!({
        "id": "deepseek-anchor",
        "version": "1.2.3",
        "entry": "dist/index.js",
        "priority": 100,
        "scope": "session",
        "stateSchema": 1,
        "permissions": ["session.write", "tools.register"],
        "optionalPermissions": ["secrets.read:deepseek_api_key"]
    }))
    .unwrap()
}

#[test]
fn package_manifest_valid_input_round_trips() {
    let manifest = valid_manifest();

    manifest.validate().unwrap();
    let encoded = serde_json::to_value(&manifest).unwrap();
    let decoded: ExtensionPackageManifest = serde_json::from_value(encoded).unwrap();

    assert_eq!(decoded, manifest);
    assert_eq!(manifest.scope, ExtensionScope::Session);
    assert_eq!(
        manifest.optional_permissions[0].secret_name(),
        Some("deepseek_api_key")
    );
}

#[test]
fn package_manifest_unknown_field_is_rejected_during_decode() {
    let decoded = serde_json::from_value::<ExtensionPackageManifest>(json!({
        "id": "example",
        "version": "1.0.0",
        "entry": "index.js",
        "scope": "session",
        "unexpected": true
    }));

    assert!(decoded.is_err());
}

#[test]
fn package_manifest_abi_selector_is_rejected_during_decode() {
    let decoded = serde_json::from_value::<ExtensionPackageManifest>(json!({
        "id": "example",
        "version": "1.0.0",
        "abi": 7,
        "entry": "index.js",
        "scope": "session"
    }));

    assert!(decoded.is_err());
}

#[test]
fn package_manifest_invalid_identity_version_and_entry_are_rejected() {
    let mut manifest = valid_manifest();
    manifest.id = "Invalid_ID".into();
    assert_eq!(manifest.validate(), Err(ExtensionManifestError::InvalidId));

    let mut manifest = valid_manifest();
    manifest.version = "latest".into();
    assert_eq!(
        manifest.validate(),
        Err(ExtensionManifestError::InvalidVersion)
    );

    let mut manifest = valid_manifest();
    manifest.entry = "../escape.js".into();
    assert_eq!(
        manifest.validate(),
        Err(ExtensionManifestError::InvalidEntry)
    );
}

#[test]
fn package_manifest_duplicate_or_overlapping_permissions_are_rejected() {
    let mut duplicate = valid_manifest();
    duplicate
        .permissions
        .push(ExtensionPermission::SessionWrite);
    assert_eq!(
        duplicate.validate(),
        Err(ExtensionManifestError::DuplicatePermission(
            "session.write".into()
        ))
    );

    let mut overlap = valid_manifest();
    overlap
        .optional_permissions
        .push(ExtensionPermission::ToolsRegister);
    assert_eq!(
        overlap.validate(),
        Err(ExtensionManifestError::RequiredOptionalOverlap(
            "tools.register".into()
        ))
    );
}

#[test]
fn package_manifest_unknown_or_wildcard_permission_is_rejected_during_decode() {
    for permission in ["filesystem.everything", "secrets.read:*"] {
        let decoded = serde_json::from_value::<ExtensionPackageManifest>(json!({
            "id": "example",
            "version": "1.0.0",
            "entry": "index.js",
            "scope": "session",
            "permissions": [permission]
        }));
        assert!(decoded.is_err(), "permission {permission} must be rejected");
    }
}

#[test]
fn new_permissions_parse_and_display_canonical_names() {
    for (name, expected) in [
        ("actions.register", ExtensionPermission::ActionsRegister),
        ("prompts.register", ExtensionPermission::PromptsRegister),
        ("hooks.subscribe", ExtensionPermission::HooksSubscribe),
        ("services.provide", ExtensionPermission::ServicesProvide),
    ] {
        let parsed: ExtensionPermission = name.parse().unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(parsed.to_string(), name);
        assert_eq!(parsed.canonical_name(), name);
    }
}

#[test]
fn new_permissions_round_trip_through_serde() {
    for permission in [
        ExtensionPermission::ActionsRegister,
        ExtensionPermission::PromptsRegister,
        ExtensionPermission::HooksSubscribe,
        ExtensionPermission::ServicesProvide,
    ] {
        let encoded = serde_json::to_string(&permission).unwrap();
        let decoded: ExtensionPermission = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, permission);
        assert_eq!(encoded, format!("\"{}\"", permission.canonical_name()));
    }
}

#[test]
fn managed_source_layer_round_trips_and_sorts_before_global_and_project() {
    let serialized = serde_json::to_string(&ExtensionSourceLayer::Managed).unwrap();
    assert_eq!(serialized, "\"managed\"");

    let decoded: ExtensionSourceLayer = serde_json::from_str(&serialized).unwrap();
    assert_eq!(decoded, ExtensionSourceLayer::Managed);

    assert!(ExtensionSourceLayer::Managed < ExtensionSourceLayer::Global);
    assert!(ExtensionSourceLayer::Global < ExtensionSourceLayer::Project);
}

#[test]
fn config_schema_round_trips_both_states() {
    let manifest = valid_manifest();
    assert!(manifest.config_schema.is_none());
    let encoded = serde_json::to_value(&manifest).unwrap();
    assert!(encoded.get("configSchema").is_none());

    let with_schema = ExtensionPackageManifest {
        config_schema: Some(json!({
            "type": "object",
            "properties": {"threshold": {"type": "number"}}
        })),
        ..valid_manifest()
    };
    let encoded = serde_json::to_value(&with_schema).unwrap();
    assert_eq!(
        encoded["configSchema"]["properties"]["threshold"]["type"],
        "number"
    );

    let decoded: ExtensionPackageManifest = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded.config_schema, with_schema.config_schema);
    assert_eq!(decoded, with_schema);
}
