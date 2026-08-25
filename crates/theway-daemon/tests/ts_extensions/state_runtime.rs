use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use theway_contract::extension::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind, ExtensionDurableEntry,
    ExtensionDurableEntryPayload, ExtensionPermission, ExtensionStateMutation,
    ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    SessionExtensionStateError, SessionExtensionStatePort,
};

use super::super::engine::QuickJsEnginePool;
use super::super::state_runtime::{
    ExtensionStateLimits, ExtensionStateRuntime,
};
use super::super::catalog::PackageCatalog;
use super::super::trust::ExtensionTrustStore;

#[derive(Default)]
struct MemoryStatePort {
    entries: Mutex<BTreeMap<String, Vec<ExtensionDurableEntry>>>,
}

#[async_trait]
impl SessionExtensionStatePort for MemoryStatePort {
    async fn append_durable_entries(
        &self,
        extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<Vec<String>, SessionExtensionStateError> {
        let target = self
            .entries
            .lock()
            .entry(extension_id.to_string())
            .or_default()
            .len();
        let ids = (0..entries.len())
            .map(|offset| format!("id-{}", target + offset + 1))
            .collect();
        self.entries
            .lock()
            .entry(extension_id.to_string())
            .or_default()
            .extend(entries);
        Ok(ids)
    }

    async fn replay_durable_entries(
        &self,
        extension_id: &str,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<ExtensionDurableEntry>, SessionExtensionStateError> {
        Ok(self.entries.lock().get(extension_id).cloned().unwrap_or_default())
    }
}

fn project_root(project: &Path) -> std::path::PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package(project: &Path, id: &str, state_schema: u32, permissions: &[&str], source: &str) {
    let package = project_root(project).join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "stateSchema": state_schema,
            "permissions": permissions,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
}


fn write_package_without_schema(project: &Path, id: &str, permissions: &[&str], source: &str) {
    let package = project_root(project).join(id);
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
    std::fs::write(package.join("index.js"), source).unwrap();
}

fn trust_project(project: &Path, base: &Path, permissions: &[&str]) {
    let requested = permissions
        .iter()
        .map(|value| value.parse::<ExtensionPermission>().unwrap())
        .collect::<Vec<_>>();
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(project, requested.clone(), requested, ExtensionTrustDecision::Trusted)
        .unwrap();
    trust.save().unwrap();
}

fn state_entry(extension_id: &str, key: &str, value: serde_json::Value) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        extension_id: extension_id.into(),
        state_schema_version: 1,
        origin_sequence: 1,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: key.into(),
            mutation: ExtensionStateMutation::Set { value },
        },
    }
}

fn runtime(
    project: &Path,
    base: &Path,
    id: &str,
    permissions: &[&str],
) -> (ExtensionStateRuntime, Arc<MemoryStatePort>) {
    write_package(project, id, 1, permissions, "export const kind='compaction';");
    trust_project(project, base, permissions);
    let catalog = PackageCatalog::discover(project, base);
    let _package = catalog
        .selected_packages()
        .into_iter()
        .find(|package| package.manifest().id == id)
        .unwrap();
    let port = Arc::new(MemoryStatePort::default());
    let runtime = ExtensionStateRuntime::new(
        "sess",
        port.clone(),
        QuickJsEnginePool::new(1),
        ExtensionStateLimits::default(),
    );
    (runtime, port)
}

#[test]
fn state_limits_defaults_are_sane() {
    let limits = ExtensionStateLimits::default();
    assert_eq!(limits.max_entries_per_batch, 32);
    assert_eq!(limits.max_entry_bytes, 64 * 1024);
    assert_eq!(limits.max_extension_bytes, 4 * 1024 * 1024);
}

#[tokio::test]
async fn reconstruct_and_commit_batch_persist_durable_state() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let (runtime, port) = runtime(project.path(), base.path(), "ext", &["session.write"]);

    let catalog = PackageCatalog::discover(project.path(), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    runtime.reconstruct(&package).await.unwrap();
    assert!(runtime.entries_for("ext").is_empty());

    let mut batch = ExtensionActionBatch {
        decision: None,
        actions: vec![ExtensionAction {
            kind: ExtensionActionKind::SetState,
            payload: serde_json::to_value(state_entry("ext", "k", json!(1))).unwrap(),
        }],
    };
    runtime.commit_batch("ext", 1, &mut batch).await.unwrap();
    assert!(batch.actions.is_empty());
    assert_eq!(port.entries.lock().get("ext").unwrap().len(), 1);
    assert_eq!(runtime.entries_for("ext").len(), 1);
}

#[tokio::test]
async fn reconstruct_requires_state_schema_when_persisted_entries_exist() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package_without_schema(project.path(), "ext", &["session.write"], "export {}");
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    port.entries.lock().insert(
        "ext".into(),
        vec![state_entry("ext", "k", json!(1))],
    );
    let runtime = ExtensionStateRuntime::new(
        "sess",
        port.clone(),
        QuickJsEnginePool::new(1),
        ExtensionStateLimits::default(),
    );
    let catalog = PackageCatalog::discover(project.path(), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    let result = runtime.reconstruct(&package).await;
    let err = match result {
        Err(error) => error,
        Ok(_) => panic!("expected reconstruct to require stateSchema"),
    };
    assert!(err.contains("stateSchema"), "{err}");
}

#[tokio::test]
async fn commit_batch_rejects_durable_actions_without_projection_or_write() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime(project.path(), base.path(), "ext", &[]);

    let mut batch = ExtensionActionBatch {
        decision: None,
        actions: vec![ExtensionAction {
            kind: ExtensionActionKind::SetState,
            payload: serde_json::to_value(state_entry("ext", "k", json!(1))).unwrap(),
        }],
    };
    let err = runtime.commit_batch("ext", 1, &mut batch).await.unwrap_err();
    assert!(err.contains("session.write"), "{err}");
}

#[tokio::test]
async fn commit_batch_rejects_count_and_size_limits() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let (runtime, _) = runtime(project.path(), base.path(), "ext", &["session.write"]);
    let catalog = PackageCatalog::discover(project.path(), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    runtime.reconstruct(&package).await.unwrap();

    let mut batch = ExtensionActionBatch {
        decision: None,
        actions: (0..33)
            .map(|i| ExtensionAction {
                kind: ExtensionActionKind::SetState,
                payload: serde_json::to_value(state_entry("ext", &format!("k{i}"), json!(i)))
                    .unwrap(),
            })
            .collect(),
    };
    let err = runtime.commit_batch("ext", 1, &mut batch).await.unwrap_err();
    assert!(err.contains("count"), "{err}");

    let mut batch = ExtensionActionBatch {
        decision: None,
        actions: vec![ExtensionAction {
            kind: ExtensionActionKind::SetState,
            payload: serde_json::to_value(state_entry("ext", "k", json!("x".repeat(70_000))))
                .unwrap(),
        }],
    };
    let err = runtime.commit_batch("ext", 1, &mut batch).await.unwrap_err();
    assert!(err.contains("size"), "{err}");
}

#[tokio::test]
async fn migrate_if_needed_returns_ok_when_no_schema_or_same_schema() {
    let project = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    write_package_without_schema(project.path(), "ext", &["session.write"], "export {}");
    trust_project(project.path(), base.path(), &["session.write"]);
    let port = Arc::new(MemoryStatePort::default());
    let runtime = ExtensionStateRuntime::new(
        "sess",
        port.clone(),
        QuickJsEnginePool::new(1),
        ExtensionStateLimits::default(),
    );
    let catalog = PackageCatalog::discover(project.path(), base.path());
    let package = catalog.selected_packages().into_iter().next().unwrap();
    runtime
        .migrate_if_needed(&package, &super::super::engine::EngineInstanceKey::new("sess", "ext"), &json!({}), 1, std::time::Duration::from_secs(1), 1)
        .await
        .unwrap();
}
