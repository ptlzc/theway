use serde_json::json;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionManifestError, ExtensionPackageManifest, ExtensionPermission,
    ExtensionScope,
};

fn valid_manifest() -> ExtensionPackageManifest {
    serde_json::from_value(json!({
        "id": "deepseek-anchor",
        "version": "1.2.3",
        "abi": 2,
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
    assert_eq!(manifest.abi, ExtensionAbiMajor::V2);
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
        "abi": 2,
        "entry": "index.js",
        "scope": "session",
        "unexpected": true
    }));

    assert!(decoded.is_err());
}

#[test]
fn package_manifest_invalid_identity_version_abi_and_entry_are_rejected() {
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
    manifest.abi = ExtensionAbiMajor(3);
    assert_eq!(
        manifest.validate(),
        Err(ExtensionManifestError::UnsupportedAbi(3))
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
            "abi": 2,
            "entry": "index.js",
            "scope": "session",
            "permissions": [permission]
        }));
        assert!(decoded.is_err(), "permission {permission} must be rejected");
    }
}
