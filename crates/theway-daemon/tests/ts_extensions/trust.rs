use std::path::Path;

use serde_json::json;
use theway_contract::extension::{
    ExtensionPermission, ExtensionSourceLayer, ExtensionTrustDecision,
};

use super::super::catalog::PackageCatalog;
use super::super::trust::{ExtensionTrustStore, GlobalExtensionPolicy};

fn write_package(root: &Path, id: &str, permissions: &[&str]) {
    let package = root.join("extensions").join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": permissions,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), "export const kind='compaction';").unwrap();
}

fn permissions(values: &[&str]) -> Vec<ExtensionPermission> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

#[test]
fn load_handles_missing_valid_unsupported_and_invalid_files() {
    let base = tempfile::tempdir().unwrap();
    let store = ExtensionTrustStore::load(base.path());
    assert!(store.load_error().is_none());

    std::fs::create_dir_all(base.path().join("extensions")).unwrap();
    std::fs::write(
        base.path().join("extensions/trust.json"),
        r#"{"version":1,"globalPolicy":"allow_declared","decisions":[]}"#,
    )
    .unwrap();
    let store = ExtensionTrustStore::load(base.path());
    assert!(store.load_error().is_none());

    std::fs::write(
        base.path().join("extensions/trust.json"),
        r#"{"version":2,"globalPolicy":"allow_declared","decisions":[]}"#,
    )
    .unwrap();
    let store = ExtensionTrustStore::load(base.path());
    assert_eq!(store.load_error(), Some("unsupported trust file version"));

    std::fs::write(base.path().join("extensions/trust.json"), "{bad").unwrap();
    let store = ExtensionTrustStore::load(base.path());
    assert!(store.load_error().unwrap().contains("invalid extension trust file"));

}

#[test]
fn set_global_policy_and_save_roundtrip() {
    let base = tempfile::tempdir().unwrap();
    let mut store = ExtensionTrustStore::load(base.path());
    store.set_global_policy(GlobalExtensionPolicy::Deny);
    store.save().unwrap();

    let loaded = ExtensionTrustStore::load(base.path());
    assert!(loaded.load_error().is_none());
    // Evaluate a global package to observe the policy through catalog discovery.
    write_package(base.path(), "global-ext", &["workspace.read"]);
    let catalog = PackageCatalog::discover(Path::new("/nonexistent"), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    let evaluation = loaded.evaluate(&package);
    assert_eq!(evaluation.blocked, Some(theway_contract::extension::ExtensionDiagnosticCode::PermissionDenied));
}

#[test]
fn decide_project_rejects_granted_superset_and_revokes_existing_decision() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let mut store = ExtensionTrustStore::load(base.path());
    let requested = permissions(&["workspace.read"]);
    let granted = permissions(&["workspace.read", "workspace.write"]);
    assert!(store
        .decide_project(project.path(), requested.clone(), granted, ExtensionTrustDecision::Trusted)
        .is_err());
    assert!(store
        .decide_project(project.path(), requested.clone(), requested.clone(), ExtensionTrustDecision::Trusted)
        .is_ok());
    store.save().unwrap();

    let mut store = ExtensionTrustStore::load(base.path());
    assert!(store.revoke_project(project.path()).unwrap());
    assert!(!store.revoke_project(project.path()).unwrap());
}

#[test]
fn decide_package_requires_subset_and_records_decision() {
    let base = tempfile::tempdir().unwrap();
    write_package(base.path(), "pkg", &["workspace.read"]);
    let catalog = PackageCatalog::discover(Path::new("/nonexistent"), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    let mut store = ExtensionTrustStore::load(base.path());
    assert!(store
        .decide_package(&package, permissions(&["workspace.read"]), permissions(&["workspace.read", "workspace.write"]), ExtensionTrustDecision::Trusted)
        .is_err());
    store
        .decide_package(&package, permissions(&["workspace.read"]), permissions(&["workspace.read"]), ExtensionTrustDecision::Trusted)
        .unwrap();
    store.save().unwrap();

    let loaded = ExtensionTrustStore::load(base.path());
    let evaluation = loaded.evaluate(&package);
    assert!(evaluation.blocked.is_none());
    assert!(evaluation.granted_permissions.contains(&ExtensionPermission::WorkspaceRead));
}

#[test]
fn evaluate_blocks_project_without_decision_and_denied_decision() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package(&project.path().join(".theway"), "proj", &["workspace.read"]);
    let catalog = PackageCatalog::discover(project.path(), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    assert_eq!(package.source(), ExtensionSourceLayer::Project);
    let store = ExtensionTrustStore::load(base.path());
    let evaluation = store.evaluate(&package);
    assert_eq!(evaluation.blocked, Some(theway_contract::extension::ExtensionDiagnosticCode::TrustRequired));

    let mut store = ExtensionTrustStore::load(base.path());
    store
        .decide_project(project.path(), permissions(&["workspace.read"]), permissions(&["workspace.read"]), ExtensionTrustDecision::Denied)
        .unwrap();
    let evaluation = store.evaluate(&package);
    assert_eq!(evaluation.blocked, Some(theway_contract::extension::ExtensionDiagnosticCode::PermissionDenied));
}

#[test]
fn evaluate_global_require_record_and_deny_policies() {
    let base = tempfile::tempdir().unwrap();
    write_package(base.path(), "g", &["workspace.read"]);
    let catalog = PackageCatalog::discover(Path::new("/nonexistent"), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();

    let mut store = ExtensionTrustStore::load(base.path());
    store.set_global_policy(GlobalExtensionPolicy::RequireRecord);
    let evaluation = store.evaluate(&package);
    assert_eq!(evaluation.blocked, Some(theway_contract::extension::ExtensionDiagnosticCode::TrustRequired));

    store.set_global_policy(GlobalExtensionPolicy::Deny);
    let evaluation = store.evaluate(&package);
    assert_eq!(evaluation.blocked, Some(theway_contract::extension::ExtensionDiagnosticCode::PermissionDenied));

    store.set_global_policy(GlobalExtensionPolicy::AllowDeclared);
    let evaluation = store.evaluate(&package);
    assert!(evaluation.blocked.is_none());
    assert_eq!(evaluation.granted_permissions.len(), 1);
}
