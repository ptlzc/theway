//! Harness kind-routing test domain (issue #82).
//!
//! Mirrored into the daemon crate's unit tests via `tests_bridge!`. Covers
//! single-file `.ts` extensions declaring `kind` = tool / action / prompt /
//! hook / service being synthesized into package-catalog entries with the
//! kind-bound permission set, plus the legacy compaction path regression.

use theway_contract::extension::{
    ExtensionCatalogStatus, ExtensionPackageManifest, ExtensionPermission, ExtensionScope,
    ExtensionSourceLayer,
};

use super::super::catalog::PackageCatalog;
use super::super::ExtensionPackage;

fn synthetic_package_for_kind(kind: &str, id: &str) -> ExtensionPackage {
    let manifest = ExtensionPackageManifest {
        id: id.to_string(),
        version: "0.0.0-single-file".into(),
        entry: "index.js".into(),
        priority: 0,
        scope: ExtensionScope::Session,
        state_schema: None,
        config_schema: None,
        permissions: kind_permissions(kind),
        optional_permissions: vec![],
    };
    let dir = std::env::temp_dir().join(format!("synthetic-{id}"));
    ExtensionPackage::synthetic_package(
        manifest,
        ExtensionSourceLayer::Project,
        dir.clone(),
        dir.join("index.js"),
        "export const kind = 'tool';",
    )
}

fn kind_permissions(kind: &str) -> Vec<ExtensionPermission> {
    match kind {
        "tool" => vec![ExtensionPermission::ToolsRegister],
        "action" => vec![ExtensionPermission::ActionsRegister],
        "prompt" => vec![ExtensionPermission::PromptsRegister],
        "hook" => vec![ExtensionPermission::HooksSubscribe],
        "service" => vec![ExtensionPermission::ServicesProvide],
        _ => vec![],
    }
}

#[test]
fn synthetic_package_merges_into_effective_catalog() {
    let mut catalog = PackageCatalog::default();
    let package = synthetic_package_for_kind("tool", "tool-ext");
    catalog.merge_synthetic_packages(vec![package]);

    assert_eq!(catalog.effective_packages().len(), 1);
    let effective = &catalog.effective_packages()[0];
    assert_eq!(effective.source(), ExtensionSourceLayer::Project);
    assert!(effective
        .granted_permissions()
        .contains(&ExtensionPermission::ToolsRegister));
    assert!(!effective
        .granted_permissions()
        .contains(&ExtensionPermission::ActionsRegister));
    assert!(catalog.entries().iter().any(|entry| {
        entry.extension_id == "tool-ext"
            && entry.status == ExtensionCatalogStatus::Effective
    }));
}

#[test]
fn synthetic_kind_permission_binding() {
    let cases = [
        ("action", ExtensionPermission::ActionsRegister),
        ("prompt", ExtensionPermission::PromptsRegister),
        ("hook", ExtensionPermission::HooksSubscribe),
        ("service", ExtensionPermission::ServicesProvide),
    ];
    for (kind, permission) in cases {
        let mut catalog = PackageCatalog::default();
        catalog.merge_synthetic_packages(vec![synthetic_package_for_kind(
            kind,
            &format!("ext-{kind}"),
        )]);
        let effective = &catalog.effective_packages()[0];
        assert!(
            effective.granted_permissions().contains(&permission),
            "kind {kind} must grant {permission:?}"
        );
        assert_eq!(effective.granted_permissions().len(), 1);
    }
}

#[test]
fn synthetic_fingerprint_tracks_package() {
    let mut a = PackageCatalog::default();
    a.merge_synthetic_packages(vec![synthetic_package_for_kind("tool", "fp")]);
    let mut b = PackageCatalog::default();
    b.merge_synthetic_packages(vec![synthetic_package_for_kind("tool", "fp")]);
    assert_eq!(a.fingerprint(), b.fingerprint());

    let mut changed = PackageCatalog::default();
    changed.merge_synthetic_packages(vec![synthetic_package_for_kind("hook", "fp")]);
    assert_ne!(a.fingerprint(), changed.fingerprint());
}

#[test]
fn single_file_kind_routes_into_extension_registry() {
    // End-to-end: `ExtensionRegistry::discover` picks up a project-level
    // `.theway/extensions/hello.ts` with `kind = "tool"` as an effective
    // package carrying the tools.register permission, while a
    // `kind = "compaction"` file stays on the legacy path.
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let extensions = project.path().join(".theway").join("extensions");
    std::fs::create_dir_all(&extensions).unwrap();
    std::fs::write(
        extensions.join("hello.ts"),
        r#"export const kind = "tool";
export default { setup(api) {} };"#,
    )
    .unwrap();
    std::fs::write(
        extensions.join("legacy-compact.ts"),
        r#"export const kind = "compaction";"#,
    )
    .unwrap();

    let registry = super::super::ExtensionRegistry::discover(project.path(), base.path());

    let package_ids: Vec<_> = registry
        .package_catalog()
        .effective_packages()
        .iter()
        .map(|package| package.manifest().id.clone())
        .collect();
    assert!(package_ids.contains(&"hello".to_string()), "{package_ids:?}");
    let hello = registry
        .package_catalog()
        .effective_packages()
        .into_iter()
        .find(|package| package.manifest().id == "hello")
        .unwrap();
    assert!(hello
        .granted_permissions()
        .contains(&ExtensionPermission::ToolsRegister));

    // The compaction file is NOT synthesized: it remains a legacy extension
    // and does not surface as an effective package.
    assert!(!package_ids.contains(&"legacy-compact".to_string()));
    assert_eq!(registry.by_kind("compaction").len(), 1);
    assert!(registry.get("legacy-compact").is_some());
    assert!(!package_ids.contains(&"hello.ts".to_string()));
}

#[test]
fn action_and_service_registrations_decode_with_permission_checks() {
    // Action/service effect registrations decode from the bridge shape and
    // require the kind-bound permissions (regression for the #82 extension
    // points).
    let granted_actions: std::collections::BTreeSet<_> =
        [ExtensionPermission::ActionsRegister].into_iter().collect();
    let decoded = super::super::registrations::validate_effect_registrations(
        &serde_json::json!({
            "effects": [
                {
                    "registrationId": 1,
                    "kind": "action",
                    "descriptor": {"name": "greet", "description": "Greet", "inputSchema": {"type": "object"}},
                    "sequence": 1,
                },
                {
                    "registrationId": 2,
                    "kind": "service",
                    "descriptor": {"name": "metrics"},
                    "sequence": 2,
                },
            ]
        }),
        "ext-id",
        ExtensionScope::Session,
        &granted_actions,
    )
    .unwrap();
    assert_eq!(decoded.registrations.len(), 1, "service must be rejected without ServicesProvide");
    assert!(matches!(
        decoded.registrations[0].value,
        super::super::registrations::OwnedRegistration::Action(_)
    ));
}
