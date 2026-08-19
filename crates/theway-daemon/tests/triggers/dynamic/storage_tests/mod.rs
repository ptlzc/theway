//! Additional tests for `triggers::dynamic` storage-seam branches.
//!
//! Bridged from a `#[cfg(test)] mod storage_tests` wrapper in the source module
//! (the primary and extra bridge slots are already occupied).

use std::sync::Arc;

use crate::triggers::dynamic::{DynamicTriggerRegistry, DynamicTriggerStorageError};
use theway_contract::session::SessionReader;
use theway_daemon::runtime_storage::{LocalRuntimeStorage, RuntimeStorage};

#[test]
fn storage_path_returns_loaded_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    let registry = DynamicTriggerRegistry::new();

    registry.load_from_path(&path).unwrap();

    assert_eq!(registry.storage_path(), Some(path));
}

#[tokio::test]
async fn load_from_storage_returns_read_error_when_session_missing() {
    let dir = tempfile::tempdir().unwrap();
    let storage: Arc<dyn RuntimeStorage> = Arc::new(LocalRuntimeStorage);
    let registry = DynamicTriggerRegistry::new();

    let err = registry
        .load_from_storage(storage, dir.path().to_path_buf(), "missing-session".into())
        .await
        .unwrap_err();

    assert!(matches!(err, DynamicTriggerStorageError::Read(_)));
}

#[tokio::test]
async fn load_from_storage_runtime_persist_spawns_save() {
    let _env_guard = crate::test_env::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", tmp.path());

    let repo = theway_storage::session::open_repo(tmp.path()).await;
    let session = theway_storage::session::create(&repo, tmp.path())
        .await
        .unwrap();
    let session_id = session
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown-session")
        .to_string();
    drop(session);

    let storage: Arc<dyn RuntimeStorage> = Arc::new(LocalRuntimeStorage);
    let registry = DynamicTriggerRegistry::new();
    registry
        .load_from_storage(storage.clone(), tmp.path().to_path_buf(), session_id.clone())
        .await
        .unwrap();
    assert!(registry.storage_path().is_none());

    let rule = registry.add_rule("event says persist", "echo persisted").unwrap();

    // `persist_rules` writes through the runtime storage seam via a spawned task.
    let saved = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let rules = storage
                .load_dynamic_triggers(tmp.path(), &session_id)
                .await
                .unwrap_or_default();
            if rules.iter().any(|r| r.id == rule.id) {
                break rules;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("spawned runtime persist should save the added rule");

    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, rule.id);
    assert_eq!(saved[0].action, "echo persisted");
}
