//! Additional tests for `triggers::dynamic` — split out of src (see docs/rust-test-files.md).
//!
//! This file is bridged from a small `#[cfg(test)] mod extra_tests` wrapper in the source
//! module (the top-level bridge slot is already occupied by the primary mirror).

use crate::triggers::dynamic::*;
use std::sync::Arc;

use theway_contract::session::SessionReader;
use theway_daemon::runtime_storage::RuntimeStorage;

#[test]
fn add_rule_rejects_empty_condition_or_action() {
    let registry = DynamicTriggerRegistry::new();

    let err = registry.add_rule("", "echo hi").unwrap_err();
    assert!(matches!(err, AddTriggerRuleError::Parse(_)));
    let err = registry.add_rule("event", "   ").unwrap_err();
    assert!(matches!(err, AddTriggerRuleError::Parse(_)));
}

#[test]
fn add_rule_with_flags_records_fire_once_and_promote_to_chat() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule_with_flags("condition text", "action text", false, true)
        .unwrap();

    assert!(!rule.fire_once);
    assert!(rule.promote_to_chat);
    assert!(rule.enabled);
    assert_eq!(rule.condition, "condition text");
    assert_eq!(rule.action, "action text");
}

#[test]
fn add_from_spec_returns_parse_error_for_bad_spec() {
    let registry = DynamicTriggerRegistry::new();
    let err = registry.add_from_spec("this has no action separator").unwrap_err();
    assert!(matches!(err, AddTriggerRuleError::Parse(_)));
}

#[test]
fn remove_and_set_enabled_missing_id_return_none() {
    let registry = DynamicTriggerRegistry::new();
    assert!(registry.remove_rule("dyn-missing").unwrap().is_none());
    assert!(registry.set_rule_enabled("dyn-missing", true).unwrap().is_none());
}

#[test]
fn remove_and_set_enabled_trim_ids() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry.add_rule("event", "echo hi").unwrap();

    let removed = registry.remove_rule(&format!("  {}  ", rule.id)).unwrap();
    assert_eq!(removed, Some(rule));
}

#[test]
fn clear_rules_removes_all_and_reports_count() {
    let registry = DynamicTriggerRegistry::new();
    registry.add_rule("event", "one").unwrap();
    registry.add_rule("event", "two").unwrap();

    assert_eq!(registry.clear_rules().unwrap(), 2);
    assert!(registry.list().is_empty());
    assert_eq!(registry.clear_rules().unwrap(), 0);
}

#[test]
fn mark_rules_fired_empty_ids_or_disabled_rule_returns_empty() {
    let registry = DynamicTriggerRegistry::new();
    assert!(registry.mark_rules_fired(&[]).unwrap().is_empty());

    let rule = registry.add_rule("event", "echo hi").unwrap();
    registry.set_rule_enabled(&rule.id, false).unwrap();

    assert!(
        registry
            .mark_rules_fired(std::slice::from_ref(&rule.id))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn poll_interval_defaults_and_clamps_to_one_second() {
    set_dynamic_trigger_poll_interval_secs(0);
    assert_eq!(dynamic_trigger_poll_interval_secs(), 1);

    set_dynamic_trigger_poll_interval_secs(DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS);
    assert_eq!(
        dynamic_trigger_poll_interval_secs(),
        DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS
    );
}

#[test]
fn global_registry_is_a_stable_singleton() {
    assert!(std::ptr::eq(
        crate::triggers::global_registry(),
        crate::triggers::global_registry()
    ));
}

#[test]
fn read_rules_file_handles_missing_empty_and_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    assert!(read_rules_file(&missing).unwrap().is_empty());

    let empty = dir.path().join("empty.json");
    std::fs::write(&empty, "   \n").unwrap();
    assert!(read_rules_file(&empty).unwrap().is_empty());

    let invalid = dir.path().join("invalid.json");
    std::fs::write(&invalid, "{ not json").unwrap();
    assert!(matches!(
        read_rules_file(&invalid),
        Err(DynamicTriggerStorageError::Parse(_))
    ));
}

#[test]
fn write_rules_file_round_trips_through_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("rules.json");
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule("event says build finished", "echo fired")
        .unwrap();

    write_rules_file(&path, std::slice::from_ref(&rule)).unwrap();

    let read = read_rules_file(&path).unwrap();
    assert_eq!(read, vec![rule]);
}

#[test]
fn storage_path_is_none_without_loaded_storage() {
    let registry = DynamicTriggerRegistry::new();
    assert!(registry.storage_path().is_none());
}

#[test]
fn clear_for_tests_resets_registry_state() {
    let registry = DynamicTriggerRegistry::new();
    registry.add_rule("event", "echo hi").unwrap();
    registry.clear_for_tests();
    assert!(registry.list().is_empty());
}

#[tokio::test]
async fn load_from_storage_reads_rules_saved_through_storage_seam() {
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

    let storage: Arc<dyn RuntimeStorage> = Arc::new(theway_daemon::runtime_storage::LocalRuntimeStorage);
    let rule = theway_contract::triggers::DynamicTriggerRule {
        id: "dyn-test-load-storage".into(),
        condition: "event says hello".into(),
        action: "echo hello".into(),
        enabled: true,
        fire_once: true,
        fired_at: None,
        promote_to_chat: false,
        created_at: chrono::Utc::now(),
    };
    storage
        .save_dynamic_triggers(tmp.path(), &session_id, std::slice::from_ref(&rule))
        .await
        .unwrap();

    let registry = DynamicTriggerRegistry::new();
    registry
        .load_from_storage(storage, tmp.path().to_path_buf(), session_id)
        .await
        .unwrap();

    assert_eq!(registry.list(), vec![rule]);
}
