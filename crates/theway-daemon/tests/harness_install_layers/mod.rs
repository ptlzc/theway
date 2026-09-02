//! Harness install-layer test domain.
//!
//! Mirrored into the daemon crate's unit tests via `tests_bridge!`. Covers the
//! managed / user (Global) / project install layers, closest-wins same-id
//! resolution, and the `synthetic_package` constructor.

use std::path::{Path, PathBuf};

use serde_json::json;
use theway_contract::extension::{
    ExtensionCatalogStatus, ExtensionDiagnosticCode, ExtensionPackageManifest, ExtensionScope,
    ExtensionSourceLayer, ExtensionTrustDecision,
};

use super::super::catalog::PackageCatalog;
use super::super::trust::ExtensionTrustStore;
use super::super::ExtensionPackage;

fn managed_root(base: &Path) -> PathBuf {
    base.join("extensions-managed")
}

fn user_root(base: &Path) -> PathBuf {
    base.join("extensions")
}

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package_at(package_parent: &Path, id: &str) {
    let package = package_parent.join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": [],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), "export const kind='compaction';").unwrap();
}

fn trust_project(project: &Path, base: &Path) {
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(project, Vec::new(), Vec::new(), ExtensionTrustDecision::Trusted)
        .unwrap();
    trust.save().unwrap();
}

#[test]
fn managed_layer_is_discovered_and_auto_trusted() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package_at(&managed_root(base.path()), "shipped");

    let catalog = PackageCatalog::discover(project.path(), base.path());

    assert_eq!(catalog.selected_packages().len(), 1);
    assert_eq!(catalog.selected_packages()[0].source(), ExtensionSourceLayer::Managed);
    assert!(catalog
        .entries()
        .iter()
        .any(|entry| entry.extension_id == "shipped"
            && entry.source == ExtensionSourceLayer::Managed
            && entry.status == ExtensionCatalogStatus::Effective));
    assert!(!catalog.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::TrustRequired
            || diagnostic.code == ExtensionDiagnosticCode::PermissionDenied
    }));
}

#[test]
fn same_id_resolves_closest_wins_project_user_managed() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package_at(&managed_root(base.path()), "same");
    write_package_at(&user_root(base.path()), "same");
    write_package_at(&project_root(project.path()), "same");
    trust_project(project.path(), base.path());

    let catalog = PackageCatalog::discover(project.path(), base.path());

    let entries: Vec<_> = catalog
        .entries()
        .iter()
        .filter(|entry| entry.extension_id == "same")
        .collect();
    assert_eq!(entries.len(), 3, "all three layers keep a catalog record");
    assert!(entries.iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Project
            && entry.status == ExtensionCatalogStatus::Effective
    }));
    assert!(entries.iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Global
            && entry.status == ExtensionCatalogStatus::Shadowed
    }));
    assert!(entries.iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Managed
            && entry.status == ExtensionCatalogStatus::Shadowed
    }));

    assert_eq!(catalog.selected_packages().len(), 1);
    assert_eq!(catalog.selected_packages()[0].source(), ExtensionSourceLayer::Project);
    assert_eq!(
        catalog
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == ExtensionDiagnosticCode::Shadowed)
            .count(),
        2
    );
}

#[test]
fn removing_project_layer_promotes_user() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package_at(&managed_root(base.path()), "same");
    write_package_at(&user_root(base.path()), "same");
    write_package_at(&project_root(project.path()), "same");
    trust_project(project.path(), base.path());

    // Project wins when present.
    let catalog = PackageCatalog::discover(project.path(), base.path());
    assert_eq!(catalog.selected_packages()[0].source(), ExtensionSourceLayer::Project);

    // Remove the project package; the user layer must become effective.
    std::fs::remove_dir_all(project_root(project.path()).join("same")).unwrap();
    let catalog = PackageCatalog::discover(project.path(), base.path());
    assert_eq!(catalog.selected_packages().len(), 1);
    assert_eq!(catalog.selected_packages()[0].source(), ExtensionSourceLayer::Global);
    assert!(catalog.entries().iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Global
            && entry.status == ExtensionCatalogStatus::Effective
    }));
    assert!(catalog.entries().iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Managed
            && entry.status == ExtensionCatalogStatus::Shadowed
    }));
}

#[test]
fn synthetic_package_constructor_yields_usable_package() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = ExtensionPackageManifest {
        id: "synth".into(),
        version: "1.0.0".into(),
        entry: "index.js".into(),
        priority: 0,
        scope: ExtensionScope::Session,
        state_schema: None,
        config_schema: None,
        permissions: Vec::new(),
        optional_permissions: Vec::new(),
    };
    let entry_source = "export const kind='tool';";
    let package_dir = dir.path().to_path_buf();
    let entry_path = package_dir.join("index.js");

    let package = ExtensionPackage::synthetic_package(
        manifest,
        ExtensionSourceLayer::Managed,
        package_dir.clone(),
        entry_path.clone(),
        entry_source,
    );

    assert_eq!(package.manifest().id, "synth");
    assert_eq!(package.source(), ExtensionSourceLayer::Managed);
    assert_eq!(package.package_dir(), package_dir.as_path());
    assert_eq!(package.entry_path(), entry_path.as_path());
    assert_eq!(package.workspace_root(), package_dir.as_path());
    assert_eq!(package.content_sha256().len(), 64);
    assert!(package
        .content_sha256()
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(package.prepared_source().unwrap(), entry_source);
    assert!(package.requested_permissions().is_empty());
}
