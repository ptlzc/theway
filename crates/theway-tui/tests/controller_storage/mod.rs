use std::sync::Arc;

use super::{ControllerStorageOps, SidecarKind};
use theway_storage::sqlite_repo::SqliteSessionRepo;

#[tokio::test]
async fn sidecar_lookup_uses_resolved_session_path_without_opening_database() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(temp.path()));
    let session_id = "01a01b12-3d67-7b21-a1a9-bbd2d74fa570";
    let database = temp.path().join(format!("{session_id}.db"));
    // An invalid database is sufficient for path resolution and proves this
    // lookup never opens the daemon-owned transcript file.
    tokio::fs::write(&database, b"not a sqlite database")
        .await
        .unwrap();
    let storage = ControllerStorageOps::new(repo);

    assert_eq!(
        storage
            .sidecar_path(session_id, SidecarKind::Trigger)
            .await
            .unwrap(),
        database.with_extension("triggers.json")
    );
    assert_eq!(
        storage
            .sidecar_path(session_id, SidecarKind::Cron)
            .await
            .unwrap(),
        database.with_extension("cron.toml")
    );
}
