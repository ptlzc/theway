use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionCatalogStatus, ExtensionDiagnosticCode, ExtensionLifecycleEvent, ExtensionSourceLayer,
    ExtensionTrustDecision,
};
use theway_daemon::ts_extensions::{
    ExtensionRegistry, ExtensionTrustStore, PackageCatalog, QuickJsEnginePool, SessionPluginHost,
};

const COUNTER_EXTENSION: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
let count = 0;
export default defineExtension((api) => {
  api.on("input", () => ({
    actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "counter", details: { count: ++count },
    } }],
  }));
});
"#;

const PLUGIN_SDK_COUNTER_EXTENSION: &str = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
let count = 0;
export default defineExtension((api) => {
  api.on("input", () => ({
    actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "counter", details: { count: ++count },
    } }],
  }));
});
"#;

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn global_root(base: &Path) -> PathBuf {
    base.join("extensions")
}

fn write_package(
    root: &Path,
    directory_id: &str,
    manifest_id: &str,
    priority: i32,
    include_abi_selector: bool,
    entry: &str,
    source: &str,
) -> PathBuf {
    let package = root.join(directory_id);
    std::fs::create_dir_all(&package).unwrap();
    let mut manifest = json!({
        "id": manifest_id,
        "version": "1.0.0",
        "entry": entry,
        "priority": priority,
        "scope": "session",
        "permissions": [],
    });
    if include_abi_selector {
        manifest["abi"] = json!(7);
    }
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let entry_path = package.join(entry);
    if let Some(parent) = entry_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&entry_path, source).unwrap();
    package
}

fn counter(value: &Value) -> u64 {
    value["actions"][0]["payload"]["details"]["count"]
        .as_u64()
        .expect("counter action")
}

fn trust_project(project: &Path, base: &Path) {
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(
            project,
            Vec::new(),
            Vec::new(),
            ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();
}

fn discover(project: &Path, base: &Path) -> PackageCatalog {
    trust_project(project, base);
    PackageCatalog::discover(project, base)
}

#[test]
fn discovers_packages_and_orders_priority_source_then_id() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "high-project",
        "high-project",
        20,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    write_package(
        &global_root(base.path()),
        "a-global",
        "a-global",
        10,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    write_package(
        &project_root(project.path()),
        "b-project",
        "b-project",
        10,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );

    let catalog = discover(project.path(), base.path());
    let ids: Vec<_> = catalog
        .effective_packages()
        .into_iter()
        .map(|package| package.manifest().id.clone())
        .collect();
    assert_eq!(ids, ["high-project", "a-global", "b-project"]);
    assert!(catalog.diagnostics().is_empty());
}

#[test]
fn project_package_shadows_global_package_by_id() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &global_root(base.path()),
        "same-id",
        "same-id",
        100,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    write_package(
        &project_root(project.path()),
        "same-id",
        "same-id",
        0,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );

    let catalog = discover(project.path(), base.path());
    let entries: Vec<_> = catalog
        .entries()
        .iter()
        .filter(|entry| entry.extension_id == "same-id")
        .collect();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Project
            && entry.status == ExtensionCatalogStatus::Effective
    }));
    assert!(entries.iter().any(|entry| {
        entry.source == ExtensionSourceLayer::Global
            && entry.status == ExtensionCatalogStatus::Shadowed
    }));
    assert!(
        catalog
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == ExtensionDiagnosticCode::Shadowed)
    );
}

#[test]
fn rejects_invalid_ids_version_selectors_and_entry_escapes_before_evaluation() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let root = project_root(project.path());
    write_package(
        &root,
        "Bad-Id",
        "Bad-Id",
        0,
        false,
        "index.js",
        "throw new Error('must not run')",
    );
    write_package(
        &root,
        "mismatch",
        "different-id",
        0,
        false,
        "index.js",
        "throw new Error('must not run')",
    );
    write_package(
        &root,
        "version-selector",
        "version-selector",
        0,
        true,
        "index.js",
        "throw new Error('must not run')",
    );
    let escaped = write_package(
        &root,
        "escaped-entry",
        "escaped-entry",
        0,
        false,
        "inside.js",
        "throw new Error('must not run')",
    );
    std::fs::write(
        escaped.join("theway-extension.json"),
        serde_json::to_vec(&json!({
            "id": "escaped-entry",
            "version": "1.0.0",
            "entry": "../outside.js",
            "scope": "session"
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("outside.js"), "throw new Error('must not run')").unwrap();

    let catalog = discover(project.path(), base.path());
    assert!(catalog.effective_packages().is_empty());
    assert_eq!(
        catalog
            .entries()
            .iter()
            .filter(|entry| entry.status == ExtensionCatalogStatus::Rejected)
            .count(),
        4
    );
    assert!(catalog.diagnostics().iter().any(|diagnostic| {
        diagnostic.extension_id == "version-selector"
            && diagnostic.code == ExtensionDiagnosticCode::ManifestInvalid
    }));
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_entry_that_resolves_outside_package() {
    use std::os::unix::fs::symlink;

    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let root = project_root(project.path());
    let package = write_package(
        &root,
        "linked-entry",
        "linked-entry",
        0,
        false,
        "entry.js",
        COUNTER_EXTENSION,
    );
    let outside = project.path().join("outside.js");
    std::fs::write(&outside, COUNTER_EXTENSION).unwrap();
    std::fs::remove_file(package.join("entry.js")).unwrap();
    symlink(&outside, package.join("entry.js")).unwrap();

    let catalog = discover(project.path(), base.path());
    assert!(catalog.effective_packages().is_empty());
    assert_eq!(
        catalog.entries()[0].status,
        ExtensionCatalogStatus::Rejected
    );
}

#[test]
fn catalog_exposes_disabled_and_faulted_statuses() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "status-test",
        "status-test",
        0,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    let mut catalog = discover(project.path(), base.path());
    assert!(catalog.set_effective_status(
        "status-test",
        ExtensionCatalogStatus::Disabled,
        Some(ExtensionDiagnosticCode::PermissionDenied),
    ));
    assert!(catalog.effective_packages().is_empty());
    assert_eq!(
        catalog.entries()[0].status,
        ExtensionCatalogStatus::Disabled
    );
    assert!(catalog.set_effective_status(
        "status-test",
        ExtensionCatalogStatus::Faulted,
        Some(ExtensionDiagnosticCode::LoadFailed),
    ));
    assert_eq!(catalog.entries()[0].status, ExtensionCatalogStatus::Faulted);
}

#[tokio::test]
async fn persistent_instances_retain_memory_and_isolate_concurrent_sessions() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "counter",
        "counter",
        0,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    let catalog = discover(project.path(), base.path());
    let engine = QuickJsEnginePool::new(2);
    let host_a = Arc::new(
        SessionPluginHost::start(catalog.clone(), engine.clone(), "session-a", project.path())
            .await,
    );
    let host_b = Arc::new(
        SessionPluginHost::start(catalog, engine.clone(), "session-b", project.path()).await,
    );
    assert_eq!(engine.instance_count().await, 2);

    let (a_first, b_first) = tokio::join!(
        host_a.invoke(ExtensionLifecycleEvent::Input, json!({"text": "a"})),
        host_b.invoke(ExtensionLifecycleEvent::Input, json!({"text": "b"}))
    );
    assert_eq!(counter(&a_first[0].value), 1);
    assert_eq!(counter(&b_first[0].value), 1);
    let a_second = host_a
        .invoke(ExtensionLifecycleEvent::Input, json!({"text": "a2"}))
        .await;
    assert_eq!(counter(&a_second[0].value), 2);

    let (left, right) = tokio::join!(
        host_a.invoke(ExtensionLifecycleEvent::Input, json!({"text": "left"})),
        host_a.invoke(ExtensionLifecycleEvent::Input, json!({"text": "right"}))
    );
    let mut serialized_counts = vec![counter(&left[0].value), counter(&right[0].value)];
    serialized_counts.sort_unstable();
    assert_eq!(serialized_counts, [3, 4]);

    host_a.shutdown().await;
    host_b.shutdown().await;
    assert_eq!(engine.instance_count().await, 0);
}

#[tokio::test]
async fn lifecycle_runs_in_order_without_ambient_daemon_globals() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let source = r#"
import { defineExtension } from "@theway-ai/plugin-sdk";
const phases = [];
export default defineExtension((api) => {
  for (const name of [
    "process", "Deno", "Bun", "require", "fetch", "XMLHttpRequest", "WebSocket",
    "thewayFilesystem", "thewayNetwork", "thewayEnvironment", "thewaySecrets",
    "thewayProvider", "thewayPersistence"
  ]) {
    if (globalThis[name] !== undefined) throw new Error(`forbidden global ${name}`);
  }
  api.on("extension_load", () => { phases.push("load"); });
  api.on("session_start", () => { phases.push("start"); });
  api.on("input", () => ({
    actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status", severity: "info", message: "phases",
      details: { phases: [...phases] },
    } }],
  }));
  api.on("session_shutdown", () => { phases.push("shutdown"); });
  api.on("extension_unload", () => { phases.push("unload"); });
});
"#;
    write_package(
        &project_root(project.path()),
        "lifecycle",
        "lifecycle",
        0,
        false,
        "index.ts",
        source,
    );
    let catalog = discover(project.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "session", project.path()).await;
    assert_eq!(host.active_extension_ids().await, ["lifecycle"]);
    let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    assert_eq!(
        output[0].value["actions"][0]["payload"]["details"]["phases"],
        json!(["load", "start"])
    );
    host.shutdown().await;
    host.shutdown().await;
    assert_eq!(engine.instance_count().await, 0);
}

#[tokio::test]
async fn plugin_sdk_package_name_resolves_to_the_virtual_host_module() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "plugin-sdk-alias",
        "plugin-sdk-alias",
        0,
        false,
        "index.js",
        PLUGIN_SDK_COUNTER_EXTENSION,
    );
    let catalog = discover(project.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine, "session", project.path()).await;

    let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    assert_eq!(counter(&output[0].value), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn legacy_theway_module_specifier_is_rejected() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        &project_root(project.path()),
        "legacy-module",
        "legacy-module",
        0,
        false,
        "index.js",
        r#"import { defineExtension } from "theway";
export default defineExtension(() => {});"#,
    );
    let catalog = discover(project.path(), base.path());
    let host = SessionPluginHost::start(
        catalog,
        QuickJsEnginePool::new(1),
        "session",
        project.path(),
    )
    .await;

    assert!(host.active_extension_ids().await.is_empty());
    assert!(host.catalog_entries().iter().any(|entry| {
        entry.extension_id == "legacy-module" && entry.status == ExtensionCatalogStatus::Faulted
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn mixed_catalog_faults_bad_packages_and_keeps_valid_instances() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let root = project_root(project.path());
    write_package(
        &root,
        "good",
        "good",
        0,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    write_package(
        &root,
        "syntax-error",
        "syntax-error",
        0,
        false,
        "index.js",
        "export default ???;",
    );
    write_package(
        &root,
        "setup-error",
        "setup-error",
        0,
        false,
        "index.js",
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension(() => { throw new Error("setup failed"); });"#,
    );
    let invalid = root.join("invalid-manifest");
    std::fs::create_dir_all(&invalid).unwrap();
    std::fs::write(invalid.join("theway-extension.json"), b"{").unwrap();

    trust_project(project.path(), base.path());
    let registry = ExtensionRegistry::discover(project.path(), base.path());
    assert!(registry.names().is_empty());
    assert_eq!(
        registry
            .package_catalog()
            .entries()
            .iter()
            .filter(|entry| entry.status == ExtensionCatalogStatus::Rejected)
            .count(),
        1
    );

    let engine = QuickJsEnginePool::new(2);
    let host = SessionPluginHost::start(
        registry.package_catalog().clone(),
        engine.clone(),
        "mixed",
        project.path(),
    )
    .await;
    assert_eq!(host.active_extension_ids().await, ["good"]);
    assert_eq!(
        host.catalog_entries()
            .iter()
            .filter(|entry| entry.status == ExtensionCatalogStatus::Faulted)
            .count(),
        2
    );
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.extension_id == "syntax-error"
            && diagnostic.code == ExtensionDiagnosticCode::LoadFailed
    }));
    let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    assert_eq!(counter(&output[0].value), 1);
    host.shutdown().await;
    assert_eq!(engine.instance_count().await, 0);
}

#[tokio::test]
async fn repeated_runtime_errors_open_only_their_owners_circuit() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let root = project_root(project.path());
    write_package(
        &root,
        "good",
        "good",
        0,
        false,
        "index.js",
        COUNTER_EXTENSION,
    );
    write_package(
        &root,
        "runtime-error",
        "runtime-error",
        0,
        false,
        "index.js",
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", () => { throw new Error("hook failed"); });
});"#,
    );
    let catalog = discover(project.path(), base.path());
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start(catalog, engine.clone(), "runtime", project.path()).await;
    assert_eq!(engine.instance_count().await, 2);
    for _ in 0..3 {
        let output = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].extension_id, "good");
    }
    assert_eq!(engine.instance_count().await, 1);
    assert!(host.catalog_entries().iter().any(|entry| {
        entry.extension_id == "runtime-error"
            && entry.status == ExtensionCatalogStatus::Disabled
            && entry.reason_code == Some(ExtensionDiagnosticCode::CircuitOpened)
    }));
    host.shutdown().await;
}
