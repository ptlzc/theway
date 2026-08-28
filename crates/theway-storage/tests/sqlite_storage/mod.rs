use super::*;
use std::sync::Arc;
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionStateMutation,
};
use theway_contract::session::{SessionBinding, SessionRuntimeContext, SessionStore};

fn message(id: &str, parent_id: Option<&str>, role: &str, text: &str) -> StoredSessionEntry {
    StoredSessionEntry::from_payload(serde_json::json!({
        "type": "message",
        "id": id,
        "parentId": parent_id,
        "timestamp": "2024-01-01T00:00:00Z",
        "message": { "role": role, "content": text, "timestamp": 1 }
    }))
    .unwrap()
}

fn extension_entry(
    id: &str,
    parent_id: Option<&str>,
    extension_id: &str,
    sequence: u64,
    value: &str,
) -> StoredSessionEntry {
    StoredSessionEntry::extension(
        id.into(),
        parent_id.map(str::to_string),
        "2026-08-20T00:00:00Z".into(),
        ExtensionDurableEntry {
            extension_id: extension_id.into(),
            state_schema_version: 1,
            origin_sequence: sequence,
            entry: ExtensionDurableEntryPayload::StateMutation {
                key: "phase".into(),
                mutation: ExtensionStateMutation::Set {
                    value: serde_json::json!(value),
                },
            },
        },
    )
    .unwrap()
}

#[tokio::test]
async fn create_existing_path_reports_already_exists() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(format!("{}.db", uuidv7()));

    let storage = SqliteSessionStorage::create(&path, "/some/cwd")
        .await
        .unwrap();
    let duplicate = SqliteSessionStorage::create(&path, "/other").await;

    assert!(path.exists());
    assert_eq!(storage.metadata().base.id.len(), 36);
    assert!(matches!(
        duplicate.err().map(|error| error.code),
        Some(SessionErrorCode::AlreadyExists)
    ));
}

#[tokio::test]
async fn open_existing_database_recovers_metadata_and_entries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entry(message("m1", None, "user", "hi"))
        .await
        .unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    let entries = reopened.get_entries().await.unwrap();

    assert_eq!(reopened.metadata().cwd, "/cwd");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "m1");
    assert_eq!(entries[0].payload["message"]["content"], "hi");
}

#[tokio::test]
async fn append_entries_commits_the_complete_ordered_batch() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();

    storage
        .append_entries(vec![
            message("a", None, "user", "1"),
            message("b", Some("a"), "assistant", "2"),
        ])
        .await
        .unwrap();
    let entries = storage.get_entries().await.unwrap();

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[tokio::test]
async fn get_extension_entries_filters_active_branch_in_replay_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entries(vec![
            message("m1", None, "user", "hello"),
            extension_entry("e1", Some("m1"), "deepseek-anchor", 1, "bootstrap"),
            extension_entry("other", Some("e1"), "other-extension", 2, "other"),
            extension_entry("e2", Some("other"), "deepseek-anchor", 3, "promoted"),
        ])
        .await
        .unwrap();

    let entries = storage
        .get_extension_entries("deepseek-anchor", None)
        .await
        .unwrap();

    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["e1", "e2"]
    );
    assert_eq!(
        entries[1]
            .extension_payload()
            .unwrap()
            .unwrap()
            .origin_sequence,
        3
    );
}

#[tokio::test]
async fn branch_switch_and_reopen_reconstruct_extension_entries_from_the_log() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entries(vec![
            message("root", None, "user", "choose a branch"),
            extension_entry("left-1", Some("root"), "deepseek-anchor", 1, "left"),
            extension_entry(
                "left-2",
                Some("left-1"),
                "deepseek-anchor",
                2,
                "left-latest",
            ),
        ])
        .await
        .unwrap();
    storage.set_leaf_id(Some("root".into())).await.unwrap();
    storage
        .append_entry(extension_entry(
            "right-1",
            Some("root"),
            "deepseek-anchor",
            3,
            "right",
        ))
        .await
        .unwrap();

    let right = storage
        .get_extension_entries("deepseek-anchor", None)
        .await
        .unwrap();
    assert_eq!(
        right
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["right-1"]
    );

    storage.set_leaf_id(Some("left-2".into())).await.unwrap();
    drop(storage);

    let resumed = SqliteSessionStorage::open(&path).await.unwrap();
    let left = resumed
        .get_extension_entries("deepseek-anchor", None)
        .await
        .unwrap();
    assert_eq!(
        left.iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["left-1", "left-2"]
    );

    resumed.set_leaf_id(Some("right-1".into())).await.unwrap();
    let switched = resumed
        .get_extension_entries("deepseek-anchor", None)
        .await
        .unwrap();
    assert_eq!(
        switched
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["right-1"]
    );
}

#[tokio::test]
async fn append_entries_duplicate_failure_rolls_back_the_complete_batch() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entry(message("existing", None, "user", "before"))
        .await
        .unwrap();

    let result = storage
        .append_entries(vec![
            message("fresh", Some("existing"), "assistant", "must roll back"),
            message("existing", None, "user", "duplicate"),
        ])
        .await;
    let entries = storage.get_entries().await.unwrap();

    assert_eq!(result.unwrap_err().code, SessionErrorCode::StorageFailure);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "existing");
}

#[tokio::test]
async fn extension_state_entries_preserve_tombstones_and_last_write_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    let mutations = [
        ExtensionStateMutation::Set {
            value: serde_json::json!("bootstrap"),
        },
        ExtensionStateMutation::Delete,
        ExtensionStateMutation::Set {
            value: serde_json::json!("promoted"),
        },
    ];
    let mut parent = None;
    let mut entries = Vec::new();
    for (index, mutation) in mutations.into_iter().enumerate() {
        let id = format!("state-{index}");
        entries.push(
            StoredSessionEntry::extension(
                id.clone(),
                parent.clone(),
                "2026-08-20T00:00:00Z".into(),
                ExtensionDurableEntry {
                    extension_id: "deepseek-anchor".into(),
                    state_schema_version: 1,
                    origin_sequence: index as u64 + 1,
                    entry: ExtensionDurableEntryPayload::StateMutation {
                        key: "phase".into(),
                        mutation,
                    },
                },
            )
            .unwrap(),
        );
        parent = Some(id);
    }
    storage.append_entries(entries).await.unwrap();

    let replay = storage
        .get_extension_entries("deepseek-anchor", None)
        .await
        .unwrap();
    let operations = replay
        .iter()
        .map(|stored| {
            let durable = stored.extension_payload().unwrap().unwrap();
            let ExtensionDurableEntryPayload::StateMutation { mutation, .. } = durable.entry else {
                panic!("expected state mutation")
            };
            mutation
        })
        .collect::<Vec<_>>();

    assert!(matches!(operations[0], ExtensionStateMutation::Set { .. }));
    assert_eq!(operations[1], ExtensionStateMutation::Delete);
    assert_eq!(
        operations[2],
        ExtensionStateMutation::Set {
            value: serde_json::json!("promoted")
        }
    );
}

#[tokio::test]
async fn unknown_extension_custom_entry_round_trips_as_opaque_data() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    let stored = StoredSessionEntry::extension(
        "future-1".into(),
        None,
        "2026-08-20T00:00:00Z".into(),
        ExtensionDurableEntry {
            extension_id: "not-installed-here".into(),
            state_schema_version: 9,
            origin_sequence: 1,
            entry: ExtensionDurableEntryPayload::CustomEvent {
                event_id: "future-event-1".into(),
                custom_type: "future.private.payload".into(),
                payload: serde_json::json!({
                    "unknownNestedShape": [1, {"future": true}],
                    "opaque": "preserve exactly"
                }),
            },
        },
    )
    .unwrap();
    let expected = stored.payload.clone();

    storage.append_entry(stored).await.unwrap();
    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    let actual = reopened.get_entry("future-1").await.unwrap().unwrap();

    assert_eq!(actual.payload, expected);
}

#[tokio::test]
async fn path_to_root_follows_persisted_parent_indexes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .append_entry(message("a", None, "user", "1"))
        .await
        .unwrap();
    storage
        .append_entry(message("b", Some("a"), "assistant", "2"))
        .await
        .unwrap();

    let leaf = storage.get_leaf_id().await.unwrap();
    let path = storage.get_path_to_root(Some("b")).await.unwrap();

    assert_eq!(leaf.as_deref(), Some("b"));
    assert_eq!(
        path.iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

fn binding() -> SessionBinding {
    SessionBinding {
        client_key: "client-1".into(),
        runtime: SessionRuntimeContext {
            work_dir: "/work".into(),
            provider: Some("provider".into()),
            model: Some("model".into()),
            base_url: Some("https://example.com".into()),
            thinking: Some(true),
        },
    }
}

#[tokio::test]
async fn set_binding_persists_across_drop_and_open() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    let binding = binding();

    storage.set_binding(Some(binding.clone())).await.unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(reopened.metadata().binding.as_ref(), Some(&binding));
}

#[tokio::test]
async fn set_binding_none_clears_persisted_row() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage.set_binding(Some(binding())).await.unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    reopened.set_binding(None).await.unwrap();
    drop(reopened);

    let cleared = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(cleared.metadata().binding, None);
}

#[tokio::test]
async fn session_store_trait_set_binding_persists_and_clears() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage: Arc<dyn SessionStore> =
        Arc::new(SqliteSessionStorage::create(&path, "/cwd").await.unwrap());

    let expected = binding();
    storage.set_binding(Some(expected.clone())).await.unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(reopened.metadata().binding.as_ref(), Some(&expected));
    let reopened: Arc<dyn SessionStore> = Arc::new(reopened);
    reopened.set_binding(None).await.unwrap();
    drop(reopened);

    let cleared = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(cleared.metadata().binding, None);
}

#[tokio::test]
async fn created_session_starts_unbound() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();

    assert_eq!(storage.metadata().binding, None);
}

#[tokio::test]
async fn binding_metadata_never_persists_sentinel_secret() {
    let sentinel = "SENTINEL_SECRET_2b3f";
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    let mut bound = binding();
    bound.runtime.provider = Some("safe-provider".into());

    storage.set_binding(Some(bound)).await.unwrap();
    let metadata_text = serde_json::to_string(&*storage.metadata()).unwrap();
    assert!(!metadata_text.contains(sentinel));

    storage.checkpoint().await.unwrap();
    drop(storage);
    let db_bytes = tokio::fs::read(&path).await.unwrap();
    assert!(
        !db_bytes
            .windows(sentinel.len())
            .any(|window| window == sentinel.as_bytes()),
        "raw session db must not contain sentinel secret"
    );
}

#[tokio::test]
async fn get_label_applies_updates_in_append_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    for (id, label) in [("l1", "first"), ("l2", "second")] {
        let entry = StoredSessionEntry::from_payload(serde_json::json!({
            "type": "label",
            "id": id,
            "parentId": null,
            "timestamp": "2024-01-01T00:00:00Z",
            "targetId": "m1",
            "label": label,
        }))
        .unwrap();
        storage.append_entry(entry).await.unwrap();
    }

    let label = storage.get_label("m1").await.unwrap();

    assert_eq!(label.as_deref(), Some("second"));
}

// ── lazy creation (issue #46) ──────────────────────────────────────────────────────────

#[tokio::test]
async fn create_lazy_writes_nothing_until_first_real_write() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(format!("{}.db", uuidv7()));

    let storage = SqliteSessionStorage::create_lazy(&path, "/cwd")
        .await
        .unwrap();
    assert!(!path.exists(), "lazy create must not write the db file");
    let id = storage.metadata().base.id.clone();
    assert_eq!(id.len(), 36);
    assert_eq!(storage.metadata().cwd, "/cwd");

    // Reads on an unmaterialized session return empty state and still write
    // nothing to disk.
    assert!(storage.get_leaf_id().await.unwrap().is_none());
    assert!(storage.get_entries().await.unwrap().is_empty());
    assert!(storage.find_entries("custom").await.unwrap().is_empty());
    assert!(storage.get_entry("nope").await.unwrap().is_none());
    assert!(storage.get_path_to_root(None).await.unwrap().is_empty());
    assert!(storage.get_label("m1").await.unwrap().is_none());
    assert!(!path.exists(), "reads must never materialize the file");

    // First real write materializes the file with the pre-minted id.
    storage
        .append_entry(message("m1", None, "user", "hi"))
        .await
        .unwrap();
    assert!(path.exists(), "first write must materialize the file");

    // A fresh open sees the same id and the appended entry (the header was
    // persisted at materialization time).
    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(reopened.metadata().base.id, id);
    let entries = reopened.get_entries().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "m1");
}

#[tokio::test]
async fn create_lazy_metadata_mutations_materialize_and_persist() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(format!("{}.db", uuidv7()));
    let storage = SqliteSessionStorage::create_lazy(&path, "/cwd")
        .await
        .unwrap();
    let id = storage.metadata().base.id.clone();

    // A metadata mutation is a real write: it materializes AND persists the
    // header (binding included) into the fresh file.
    let mut bound = binding();
    bound.runtime.provider = Some("lazy-provider".into());
    storage.set_binding(Some(bound.clone())).await.unwrap();
    assert!(path.exists());

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(reopened.metadata().base.id, id);
    assert_eq!(
        reopened
            .metadata()
            .binding
            .as_ref()
            .unwrap()
            .runtime
            .provider,
        Some("lazy-provider".into())
    );
}

#[tokio::test]
async fn collapse_metadata_persists_across_drop_and_open() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();

    storage
        .set_collapse_node_id(Some("node-1".into()))
        .await
        .unwrap();
    storage.set_collapsed(true).await.unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(
        reopened.metadata().collapse_node_id.as_deref(),
        Some("node-1")
    );
    assert_eq!(reopened.metadata().collapsed, Some(true));
}

#[tokio::test]
async fn collapse_metadata_can_be_cleared() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage = SqliteSessionStorage::create(&path, "/cwd").await.unwrap();
    storage
        .set_collapse_node_id(Some("node-1".into()))
        .await
        .unwrap();
    storage.set_collapsed(true).await.unwrap();

    storage.set_collapse_node_id(None).await.unwrap();
    storage.set_collapsed(false).await.unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(reopened.metadata().collapse_node_id, None);
    assert_eq!(reopened.metadata().collapsed, Some(false));
}

#[tokio::test]
async fn collapse_metadata_via_session_store_trait() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("s.db");
    let storage: std::sync::Arc<dyn SessionStore> =
        std::sync::Arc::new(SqliteSessionStorage::create(&path, "/cwd").await.unwrap());

    storage
        .set_collapse_node_id(Some("node-trait".into()))
        .await
        .unwrap();
    storage.set_collapsed(true).await.unwrap();
    drop(storage);

    let reopened = SqliteSessionStorage::open(&path).await.unwrap();
    assert_eq!(
        reopened.metadata().collapse_node_id.as_deref(),
        Some("node-trait")
    );
    assert_eq!(reopened.metadata().collapsed, Some(true));
}

#[tokio::test]
async fn create_lazy_respects_existing_path_and_repo_listing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("occupied.db");
    SqliteSessionStorage::create(&path, "/cwd").await.unwrap();

    // Creating lazily over an existing file fails like eager create.
    let err = match SqliteSessionStorage::create_lazy(&path, "/other").await {
        Ok(_) => panic!("lazy create over an existing file must fail"),
        Err(e) => e,
    };
    assert!(matches!(err.code, SessionErrorCode::AlreadyExists));

    // A lazy session does not appear in the repo listing until materialized.
    let repo = crate::sqlite_repo::SqliteSessionRepo::new(dir.path());
    let lazy = repo.create_lazy("/cwd").await.unwrap();
    let lazy_id = lazy.metadata().base.id.clone();
    assert_eq!(
        repo.list().await.unwrap().len(),
        1,
        "only the eager file lists"
    );
    lazy.append_entry(message("m1", None, "user", "hi"))
        .await
        .unwrap();
    let listed = repo.list().await.unwrap();
    assert_eq!(listed.len(), 2, "materialized session now lists");
    assert!(
        listed
            .iter()
            .any(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())
                == Some(lazy_id.clone())),
        "materialized file keeps the pre-minted id as its stem"
    );
}
