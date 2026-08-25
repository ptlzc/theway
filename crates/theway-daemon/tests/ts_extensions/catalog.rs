use std::path::Path;

use serde_json::json;
use theway_contract::extension::{ExtensionCatalogStatus, ExtensionDiagnosticCode};

use super::super::catalog::PackageCatalog;

fn write_package(root: &Path, id: &str, entry_name: &str, entry: &str) {
    let package = root.join("extensions").join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": entry_name,
            "priority": 0,
            "scope": "session",
            "permissions": [],
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join(entry_name), entry).unwrap();
}

#[test]
fn entry_path_and_prepared_source_for_plain_js() {
    let project = tempfile::tempdir().unwrap();
    write_package(&project.path().join(".theway"), "pkg", "index.js", "export const x = 1;");
    let catalog = PackageCatalog::discover(project.path(), tempfile::tempdir().unwrap().path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    assert!(package.entry_path().ends_with("index.js"));
    assert_eq!(package.prepared_source().unwrap(), "export const x = 1;");
}

#[test]
fn discover_ignores_missing_roots() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let catalog = PackageCatalog::discover(project.path(), base.path());
    assert!(catalog.selected_packages().is_empty());
    assert!(catalog.entries().is_empty());
}

#[test]
fn discover_records_trust_load_error_diagnostic() {
    let base = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(base.path().join("extensions")).unwrap();
    std::fs::write(base.path().join("extensions/trust.json"), "{bad").unwrap();
    let catalog = PackageCatalog::discover(tempfile::tempdir().unwrap().path(), base.path());
    assert!(catalog.diagnostics().iter().any(|d| d.extension_id == "trust-policy"));
}

#[test]
fn discover_rejects_invalid_package_directory_and_entry() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let root = project.path().join(".theway/extensions");
    std::fs::create_dir_all(&root).unwrap();

    // Manifest id mismatch
    let bad = root.join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(
        bad.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "other",
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": []
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(bad.join("index.js"), "export {}").unwrap();

    // Missing entry
    let missing = root.join("missing");
    std::fs::create_dir_all(&missing).unwrap();
    std::fs::write(
        missing.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "missing",
            "version": "1.0.0",
            "entry": "nope.js",
            "priority": 0,
            "scope": "session",
            "permissions": []
        }))
        .unwrap(),
    )
    .unwrap();

    let catalog = PackageCatalog::discover(project.path(), base.path());
    assert!(catalog.selected_packages().is_empty());
    let diagnostics = catalog.diagnostics();
    assert!(diagnostics.iter().any(|d| d.message.contains("id must match")));
    assert!(diagnostics.iter().any(|d| d.message.contains("not readable")));
    assert!(catalog.entries().iter().any(|e| e.status == ExtensionCatalogStatus::Rejected));
}

#[test]
fn set_effective_status_updates_effective_entry_and_returns_false_for_unknown() {
    let base = tempfile::tempdir().unwrap();
    write_package(base.path(), "pkg", "index.js", "export {}");
    let mut catalog = PackageCatalog::discover(Path::new("/nonexistent"), base.path());
    assert!(catalog.set_effective_status("pkg", ExtensionCatalogStatus::Disabled, Some(ExtensionDiagnosticCode::CircuitOpened)));
    assert!(!catalog.set_effective_status("missing", ExtensionCatalogStatus::Disabled, None));
    assert!(catalog
        .entries()
        .iter()
        .any(|e| e.extension_id == "pkg" && e.status == ExtensionCatalogStatus::Disabled));
}

#[test]
fn fingerprint_includes_packages_and_entries() {
    let project = tempfile::tempdir().unwrap();
    write_package(&project.path().join(".theway"), "pkg", "index.js", "export {}");
    let catalog = PackageCatalog::discover(project.path(), tempfile::tempdir().unwrap().path());
    let fingerprint = catalog.fingerprint();
    assert!(fingerprint.iter().any(|f| f.starts_with("package:")));
    assert!(fingerprint.iter().any(|f| f.starts_with("entry:")));
}

#[test]
fn effective_packages_filters_by_status() {
    let base = tempfile::tempdir().unwrap();
    write_package(base.path(), "pkg", "index.js", "export {}");
    let mut catalog = PackageCatalog::discover(Path::new("/nonexistent"), base.path());
    assert_eq!(catalog.effective_packages().len(), 1);
    catalog.set_effective_status("pkg", ExtensionCatalogStatus::Disabled, None);
    assert!(catalog.effective_packages().is_empty());
}

#[test]
fn trust_error_blocks_project_without_trust() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package(&project.path().join(".theway"), "pkg", "index.js", "export {}");
    let catalog = PackageCatalog::discover(project.path(), base.path());
    // Project package without explicit trust is blocked, but still selected.
    assert_eq!(catalog.selected_packages().len(), 1);
    assert!(catalog
        .entries()
        .iter()
        .any(|e| e.status == ExtensionCatalogStatus::Blocked));
}
