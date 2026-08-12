//! Tests for `mod` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn automation_counts_reads_enabled_and_total_from_sidecars() {
    let dir = tempdir().unwrap();
    let session_path = dir.path().join("s.jsonl");

    let counts = automation_counts(&session_path).await;
    assert!(counts.is_empty(), "missing sidecars must count as zero");
    assert_eq!(counts.badge(), None);

    std::fs::write(
        trigger_sidecar_path(&session_path),
        r#"{"version":1,"rules":[{"enabled":true},{"enabled":false}]}"#,
    )
    .unwrap();
    std::fs::write(
        cron_sidecar_path(&session_path),
        "[[jobs]]\nenabled = true\n\n[[jobs]]\nenabled = false\n\n[[jobs]]\nenabled = true\n",
    )
    .unwrap();
    let counts = automation_counts(&session_path).await;
    assert_eq!(counts.cron_total, 3);
    assert_eq!(counts.cron_enabled, 2);
    assert_eq!(counts.trigger_total, 2);
    assert_eq!(counts.trigger_enabled, 1);
    assert!(counts.any_enabled());
    assert_eq!(counts.badge().as_deref(), Some("2 cron, 1 trigger"));

    // Corrupt sidecars degrade to zeros: listings/hints must never hard-fail on them.
    std::fs::write(cron_sidecar_path(&session_path), "not toml [").unwrap();
    std::fs::write(trigger_sidecar_path(&session_path), "{oops").unwrap();
    let counts = automation_counts(&session_path).await;
    assert!(counts.is_empty());
}

#[test]
fn automation_badge_renders_each_shape() {
    let only_cron = AutomationCounts {
        cron_enabled: 2,
        cron_total: 2,
        ..Default::default()
    };
    assert_eq!(only_cron.badge().as_deref(), Some("2 cron"));
    let only_trigger = AutomationCounts {
        trigger_enabled: 1,
        trigger_total: 3,
        ..Default::default()
    };
    assert_eq!(only_trigger.badge().as_deref(), Some("1 trigger"));
    let all_disabled = AutomationCounts {
        cron_total: 2,
        trigger_total: 1,
        ..Default::default()
    };
    assert_eq!(all_disabled.badge().as_deref(), Some("automation off"));
}

#[tokio::test]
async fn automation_elsewhere_hint_names_newest_session_with_enabled_automation() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let older = repo.create("/cwd").await.unwrap();
    let older_meta = older.storage().get_metadata_json().await.unwrap();
    let older_path = PathBuf::from(older_meta["path"].as_str().unwrap());
    let older_id = older_meta["id"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let newer = repo.create("/cwd").await.unwrap();
    let newer_path = PathBuf::from(
        newer.storage().get_metadata_json().await.unwrap()["path"]
            .as_str()
            .unwrap(),
    );

    assert!(
        automation_elsewhere_hint(&repo, Some(&newer_path))
            .await
            .is_none(),
        "no automation anywhere must produce no hint"
    );

    std::fs::write(cron_sidecar_path(&older_path), "[[jobs]]\nenabled = true\n").unwrap();
    let hint = automation_elsewhere_hint(&repo, Some(&newer_path))
        .await
        .expect("enabled automation in the older session must be surfaced");
    let short: String = older_id.chars().take(16).collect();
    assert!(hint.contains(&short), "{hint}");
    assert!(hint.contains("--resume-id"), "{hint}");

    assert!(
        automation_elsewhere_hint(&repo, Some(&older_path))
            .await
            .is_none(),
        "the session holding the automation must not hint at itself"
    );

    // Disabled-only automation will not fire, so it is not worth a hint.
    std::fs::write(
        cron_sidecar_path(&older_path),
        "[[jobs]]\nenabled = false\n",
    )
    .unwrap();
    assert!(
        automation_elsewhere_hint(&repo, Some(&newer_path))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn resume_matches_legacy_metadata_id_when_file_stem_differs() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let path = dir.path().join("file-id.jsonl");
    std::fs::write(
        &path,
        serde_json::json!({
            "id": "metadata-id",
            "createdAt": "2026-01-01T00:00:00Z",
            "cwd": "/cwd",
            "path": path.to_string_lossy(),
        })
        .to_string()
            + "\n",
    )
    .unwrap();

    let session = resume(&repo, Some("metadata")).await.unwrap();
    let meta = session.storage().get_metadata_json().await.unwrap();
    assert_eq!(meta.get("id").and_then(|v| v.as_str()), Some("metadata-id"));
}

#[tokio::test]
async fn resume_with_no_id_picks_most_recent_session() {
    // UUIDv7 is time-ordered, so the lexically-greatest filename in the sessions dir is
    // the newest one. Verify resume() picks it when called with no explicit id (which is
    // what `theway -c / --continue` ends up doing).
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());

    // First, older session.
    let older = repo.create("/cwd").await.unwrap();
    let older_id = older
        .storage()
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    // tiny sleep to ensure the UUIDv7 timestamp slot bumps for the next create
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let newer = repo.create("/cwd").await.unwrap();
    let newer_id = newer
        .storage()
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_ne!(older_id, newer_id);

    let picked = resume(&repo, None).await.unwrap();
    let picked_id = picked
        .storage()
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(
        picked_id, newer_id,
        "resume() with no id should pick the most recent session"
    );
}

#[tokio::test]
async fn delete_matches_legacy_metadata_id_when_file_stem_differs() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let path = dir.path().join("file-id.jsonl");
    std::fs::write(
        &path,
        serde_json::json!({
            "id": "metadata-id",
            "createdAt": "2026-01-01T00:00:00Z",
            "cwd": "/cwd",
            "path": path.to_string_lossy(),
        })
        .to_string()
            + "\n",
    )
    .unwrap();

    let deleted = delete_by_id(&repo, "metadata").await.unwrap();
    assert_eq!(deleted, path);
    assert!(!deleted.exists());
}

#[test]
fn trigger_sidecar_path_lives_next_to_session_file() {
    let path = std::path::Path::new("/tmp/session-id.jsonl");
    assert_eq!(
        trigger_sidecar_path(path),
        std::path::PathBuf::from("/tmp/session-id.triggers.json")
    );
    assert_eq!(
        cron_sidecar_path(path),
        std::path::PathBuf::from("/tmp/session-id.cron.toml")
    );
}

#[tokio::test]
async fn sidecar_paths_survive_session_resume() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let created = repo.create("/cwd").await.unwrap();
    let metadata = created.storage().get_metadata_json().await.unwrap();
    let session_id = metadata.get("id").and_then(|v| v.as_str()).unwrap();
    let session_path = metadata.get("path").and_then(|v| v.as_str()).unwrap();
    let expected_trigger = trigger_sidecar_path(std::path::Path::new(session_path));
    let expected_cron = cron_sidecar_path(std::path::Path::new(session_path));

    std::fs::write(&expected_trigger, "{\"version\":1,\"rules\":[]}").unwrap();
    std::fs::write(&expected_cron, "[[jobs]]\n").unwrap();
    let resumed = resume(&repo, Some(session_id)).await.unwrap();

    assert_eq!(
        trigger_sidecar_path_for_session(&resumed, &repo)
            .await
            .unwrap(),
        expected_trigger
    );
    assert_eq!(
        cron_sidecar_path_for_session(&resumed, &repo)
            .await
            .unwrap(),
        expected_cron
    );
}

#[tokio::test]
async fn cron_sidecar_is_session_specific() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let first = repo.create("/cwd").await.unwrap();
    let second = repo.create("/cwd").await.unwrap();

    let first_path = cron_sidecar_path_for_session(&first, &repo).await.unwrap();
    let second_path = cron_sidecar_path_for_session(&second, &repo).await.unwrap();

    assert_ne!(first_path, second_path);
    std::fs::write(&first_path, "[[jobs]]\n").unwrap();
    assert!(first_path.exists());
    assert!(
        !second_path.exists(),
        "a new session must not inherit another session's cron sidecar"
    );
}

#[test]
fn endpoint_sidecar_path_lives_next_to_session_file() {
    let path = std::path::Path::new("/tmp/session-id.jsonl");
    assert_eq!(
        endpoint_sidecar_path(path),
        std::path::PathBuf::from("/tmp/session-id.endpoints.json")
    );
}

#[tokio::test]
async fn delete_removes_endpoint_sidecar() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let session = repo.create("/cwd").await.unwrap();
    let id = session
        .storage()
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let session_path = repo.list().await.unwrap().pop().unwrap();
    let endpoint_path = endpoint_sidecar_path(&session_path);
    std::fs::write(&endpoint_path, "{\"version\":1,\"endpoints\":[]}").unwrap();

    let deleted = delete_by_id(&repo, &id).await.unwrap();

    assert_eq!(deleted, session_path);
    assert!(!endpoint_path.exists());
}

#[tokio::test]
async fn delete_removes_session_sidecars() {
    let dir = tempdir().unwrap();
    let repo = HybridSessionRepo::new(dir.path());
    let session = repo.create("/cwd").await.unwrap();
    let id = session
        .storage()
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let session_path = repo.list().await.unwrap().pop().unwrap();
    let trigger_path = trigger_sidecar_path(&session_path);
    let cron_path = cron_sidecar_path(&session_path);
    std::fs::write(&trigger_path, "{}").unwrap();
    std::fs::write(&cron_path, "[[jobs]]\n").unwrap();

    let deleted = delete_by_id(&repo, &id).await.unwrap();

    assert_eq!(deleted, session_path);
    assert!(!deleted.exists());
    assert!(!trigger_path.exists());
    assert!(!cron_path.exists());
}
