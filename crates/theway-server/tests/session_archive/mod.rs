//! Tests for `session_archive` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use chrono::Utc;
use theway_storage::hybrid_repo::HybridSessionRepo;

#[tokio::test]
async fn export_import_rewrites_metadata_and_disables_automation() {
    let temp = tempfile::tempdir().unwrap();
    let source_cwd = temp.path().join("source");
    let dest_cwd = temp.path().join("dest");
    tokio::fs::create_dir_all(&source_cwd).await.unwrap();
    tokio::fs::create_dir_all(&dest_cwd).await.unwrap();
    let source_repo = HybridSessionRepo::new(temp.path().join("source-sessions"));
    let source = source_repo
        .create(source_cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    source
        .append_custom(
            "test_event",
            Some(serde_json::json!({"transcript": "preserved"})),
        )
        .await
        .unwrap();
    let source_meta = source.storage().get_metadata_json().await.unwrap();
    let source_path = PathBuf::from(source_meta["path"].as_str().unwrap());

    let trigger_path = crate::session::trigger_sidecar_path(&source_path);
    let trigger_file = DynamicTriggerFile {
        version: 1,
        rules: vec![DynamicTriggerRule {
            id: "trigger-1".into(),
            condition: "when something happens".into(),
            action: "do work".into(),
            enabled: true,
            fire_once: true,
            fired_at: Some(Utc::now()),
            promote_to_chat: false,
            created_at: Utc::now(),
        }],
    };
    tokio::fs::write(
        &trigger_path,
        serde_json::to_string_pretty(&trigger_file).unwrap(),
    )
    .await
    .unwrap();

    let cron_path = crate::session::cron_sidecar_path(&source_path);
    let cron_file = CronJobsFile {
        jobs: vec![CronJob {
            id: "cron-1".into(),
            schedule: "0 * * * *".into(),
            action: "hourly work".into(),
            enabled: true,
            running_trace_id: Some("trace-secret".into()),
            last_due_at: Some(Utc::now()),
            last_fired_at: Some(Utc::now()),
            last_completed_at: None,
            last_error: Some("old error".into()),
            skipped_overlap_count: 2,
            stateful: false,
            created_at: Utc::now(),
        }],
    };
    tokio::fs::write(&cron_path, toml::to_string_pretty(&cron_file).unwrap())
        .await
        .unwrap();

    let archive = temp.path().join("backup.theway-session");
    let export = export_session(&source, &archive, false).await.unwrap();
    assert_eq!(export.entry_count, 1);
    assert!(export.has_triggers);
    assert!(export.has_cron);

    let dest_repo = HybridSessionRepo::new(temp.path().join("dest-sessions"));
    let imported = import_session(&dest_repo, &archive, &dest_cwd, ActivateTriggers::Off)
        .await
        .unwrap();
    assert_eq!(imported.entry_count, 1);
    assert_eq!(imported.triggers_imported, 1);
    assert_eq!(imported.cron_imported, 1);
    assert_ne!(imported.session_id, export.session_id);

    let imported_session = dest_repo.open(&imported.session_path).await.unwrap();
    let meta = imported_session
        .storage()
        .get_metadata_json()
        .await
        .unwrap();
    assert_eq!(meta["id"].as_str().unwrap(), imported.session_id);
    assert_eq!(meta["cwd"].as_str().unwrap(), dest_cwd.to_string_lossy());
    assert_eq!(
        meta["path"].as_str().unwrap(),
        imported.session_path.to_string_lossy()
    );

    let imported_triggers =
        tokio::fs::read_to_string(crate::session::trigger_sidecar_path(&imported.session_path))
            .await
            .unwrap();
    let imported_trigger_file: DynamicTriggerFile =
        serde_json::from_str(&imported_triggers).unwrap();
    assert!(!imported_trigger_file.rules[0].enabled);
    // fired_at is history: a fire-once rule that already fired must not re-fire after a
    // later manual enable, so import preserves it in every activation mode.
    assert!(imported_trigger_file.rules[0].fired_at.is_some());

    let imported_cron =
        tokio::fs::read_to_string(crate::session::cron_sidecar_path(&imported.session_path))
            .await
            .unwrap();
    let imported_cron_file: CronJobsFile = toml::from_str(&imported_cron).unwrap();
    let job = &imported_cron_file.jobs[0];
    assert!(!job.enabled);
    assert!(job.running_trace_id.is_none());
    assert!(job.last_due_at.is_none());
    assert!(job.last_error.is_none());
    assert_eq!(job.skipped_overlap_count, 0);

    let excluded_archive = temp.path().join("backup-no-automation.theway-session");
    let export_without_automation = export_session(&source, &excluded_archive, true)
        .await
        .unwrap();
    assert!(!export_without_automation.has_triggers);
    assert!(!export_without_automation.has_cron);
    let archive_files = read_archive(&excluded_archive).unwrap();
    assert!(!archive_files.contains_key(TRIGGERS_PATH));
    assert!(!archive_files.contains_key(CRON_PATH));
    let imported_without_automation = import_session(
        &dest_repo,
        &excluded_archive,
        &dest_cwd,
        ActivateTriggers::Off,
    )
    .await
    .unwrap();
    assert_eq!(imported_without_automation.triggers_imported, 0);
    assert_eq!(imported_without_automation.cron_imported, 0);
}

#[test]
fn rejects_unsafe_archive_paths() {
    assert!(validate_archive_path(Path::new("manifest.json")).is_ok());
    assert!(validate_archive_path(Path::new("sidecars/triggers.json")).is_ok());
    assert!(validate_archive_path(Path::new("../session.jsonl")).is_err());
    assert!(validate_archive_path(Path::new("/tmp/session.jsonl")).is_err());
}

#[tokio::test]
async fn ask_activation_is_explicitly_rejected_until_interactive_confirm_exists() {
    let temp = tempfile::tempdir().unwrap();
    let repo = HybridSessionRepo::new(temp.path().join("sessions"));
    let err = import_session(
        &repo,
        &temp.path().join("missing.theway-session"),
        temp.path(),
        ActivateTriggers::Ask,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("activate-triggers=ask"), "{err}");
    assert!(err.contains("not implemented"), "{err}");
}

#[tokio::test]
async fn export_manifest_uses_last_entry_as_leaf_without_explicit_leaf_row() {
    let temp = tempfile::tempdir().unwrap();
    let source_cwd = temp.path().join("source");
    tokio::fs::create_dir_all(&source_cwd).await.unwrap();
    let source_repo = HybridSessionRepo::new(temp.path().join("source-sessions"));
    let source = source_repo
        .create(source_cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    source
        .append_custom("first", Some(serde_json::json!({"n": 1})))
        .await
        .unwrap();
    let last_id = source
        .append_custom("second", Some(serde_json::json!({"n": 2})))
        .await
        .unwrap();
    let archive = temp.path().join("backup.theway-session");
    export_session(&source, &archive, false).await.unwrap();

    let manifest = manifest_from_archive(&archive);
    assert_eq!(
        manifest.content.active_leaf_id.as_deref(),
        Some(last_id.as_str())
    );
}

#[tokio::test]
async fn export_manifest_uses_explicit_leaf_target_not_leaf_row_id() {
    let temp = tempfile::tempdir().unwrap();
    let source_cwd = temp.path().join("source");
    tokio::fs::create_dir_all(&source_cwd).await.unwrap();
    let source_repo = HybridSessionRepo::new(temp.path().join("source-sessions"));
    let source = source_repo
        .create(source_cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    let first_id = source
        .append_custom("first", Some(serde_json::json!({"n": 1})))
        .await
        .unwrap();
    source
        .append_custom("second", Some(serde_json::json!({"n": 2})))
        .await
        .unwrap();
    source.move_to(Some(&first_id), None).await.unwrap();

    let archive = temp.path().join("backup.theway-session");
    export_session(&source, &archive, false).await.unwrap();

    let manifest = manifest_from_archive(&archive);
    assert_eq!(
        manifest.content.active_leaf_id.as_deref(),
        Some(first_id.as_str())
    );
    let session_text = source.entries().await.unwrap();
    let last_entry = session_text.last().unwrap();
    assert_ne!(
        manifest.content.active_leaf_id.as_deref(),
        Some(last_entry.id())
    );
}

#[tokio::test]
async fn import_rejects_manifest_active_leaf_that_does_not_match_session_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let source_cwd = temp.path().join("source");
    let dest_cwd = temp.path().join("dest");
    tokio::fs::create_dir_all(&source_cwd).await.unwrap();
    tokio::fs::create_dir_all(&dest_cwd).await.unwrap();
    let source_repo = HybridSessionRepo::new(temp.path().join("source-sessions"));
    let source = source_repo
        .create(source_cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    source
        .append_custom("entry", Some(serde_json::json!({"n": 1})))
        .await
        .unwrap();
    let archive = temp.path().join("backup.theway-session");
    export_session(&source, &archive, false).await.unwrap();
    let mut files = read_archive(&archive).unwrap();
    let mut manifest: Manifest = serde_json::from_slice(files.get(MANIFEST_PATH).unwrap()).unwrap();
    manifest.content.active_leaf_id = Some("stale-leaf-id".into());
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    files.insert(MANIFEST_PATH.into(), manifest_bytes);
    let tampered_archive = temp.path().join("tampered.theway-session");
    write_test_archive(&tampered_archive, &files);

    let dest_repo = HybridSessionRepo::new(temp.path().join("dest-sessions"));
    let err = import_session(
        &dest_repo,
        &tampered_archive,
        &dest_cwd,
        ActivateTriggers::Off,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("active leaf"), "{err}");
}

async fn make_exported_session(
    temp: &Path,
    rules: Vec<DynamicTriggerRule>,
    jobs: Vec<CronJob>,
) -> (String, PathBuf) {
    let source_cwd = temp.join("source");
    tokio::fs::create_dir_all(&source_cwd).await.unwrap();
    let source_repo = HybridSessionRepo::new(temp.join("source-sessions"));
    let source = source_repo
        .create(source_cwd.to_string_lossy().to_string())
        .await
        .unwrap();
    source
        .append_custom("entry", Some(serde_json::json!({"n": 1})))
        .await
        .unwrap();
    let source_meta = source.storage().get_metadata_json().await.unwrap();
    let source_path = PathBuf::from(source_meta["path"].as_str().unwrap());
    if !rules.is_empty() {
        let trigger_file = DynamicTriggerFile { version: 1, rules };
        tokio::fs::write(
            crate::session::trigger_sidecar_path(&source_path),
            serde_json::to_string_pretty(&trigger_file).unwrap(),
        )
        .await
        .unwrap();
    }
    if !jobs.is_empty() {
        let cron_file = CronJobsFile { jobs };
        tokio::fs::write(
            crate::session::cron_sidecar_path(&source_path),
            toml::to_string_pretty(&cron_file).unwrap(),
        )
        .await
        .unwrap();
    }
    let archive = temp.join("backup.theway-session");
    let export = export_session(&source, &archive, false).await.unwrap();
    (export.session_id, archive)
}

fn test_trigger_rule(id: &str, enabled: bool, fired: bool) -> DynamicTriggerRule {
    DynamicTriggerRule {
        id: id.into(),
        condition: "when something happens".into(),
        action: "do work".into(),
        enabled,
        fire_once: true,
        fired_at: fired.then(Utc::now),
        promote_to_chat: false,
        created_at: Utc::now(),
    }
}

fn test_cron_job(id: &str, enabled: bool) -> CronJob {
    CronJob {
        id: id.into(),
        schedule: "0 * * * *".into(),
        action: "hourly work".into(),
        enabled,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: Utc::now(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn export_archive_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    let (_, archive) = make_exported_session(temp.path(), vec![], vec![]).await;
    let mode = std::fs::metadata(&archive).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "archive mode {mode:o}");
}

#[tokio::test]
async fn export_refuses_to_overwrite_existing_output() {
    let temp = tempfile::tempdir().unwrap();
    let (_, archive) = make_exported_session(temp.path(), vec![], vec![]).await;
    let source_repo = HybridSessionRepo::new(temp.path().join("source-sessions"));
    let source = source_repo
        .open(&source_repo.list().await.unwrap().pop().unwrap())
        .await
        .unwrap();
    let err = export_session(&source, &archive, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("exists"), "{err}");
    let original = tokio::fs::read(&archive).await.unwrap();
    assert!(
        !original.is_empty(),
        "existing archive must not be truncated"
    );
}

#[tokio::test]
async fn import_records_source_provenance_in_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let (source_id, archive) = make_exported_session(temp.path(), vec![], vec![]).await;
    let dest_cwd = temp.path().join("dest");
    tokio::fs::create_dir_all(&dest_cwd).await.unwrap();
    let dest_repo = HybridSessionRepo::new(temp.path().join("dest-sessions"));
    let imported = import_session(&dest_repo, &archive, &dest_cwd, ActivateTriggers::Off)
        .await
        .unwrap();

    let session = dest_repo.open(&imported.session_path).await.unwrap();
    let meta = session.storage().get_metadata_json().await.unwrap();
    let origin = &meta["importedFrom"];
    assert_eq!(origin["sessionId"].as_str(), Some(source_id.as_str()));
    assert_eq!(
        origin["cwd"].as_str().map(PathBuf::from),
        Some(temp.path().join("source"))
    );
    assert!(origin["exportedAt"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(
        origin["thewayVersion"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
}

#[tokio::test]
async fn activation_on_preserves_source_disabled_automation() {
    let temp = tempfile::tempdir().unwrap();
    let rules = vec![
        test_trigger_rule("was-enabled", true, true),
        test_trigger_rule("was-disabled", false, false),
    ];
    let jobs = vec![
        test_cron_job("job-on", true),
        test_cron_job("job-off", false),
    ];
    let (_, archive) = make_exported_session(temp.path(), rules, jobs).await;
    let dest_cwd = temp.path().join("dest");
    tokio::fs::create_dir_all(&dest_cwd).await.unwrap();
    let dest_repo = HybridSessionRepo::new(temp.path().join("dest-sessions"));
    let imported = import_session(&dest_repo, &archive, &dest_cwd, ActivateTriggers::On)
        .await
        .unwrap();

    let triggers =
        tokio::fs::read_to_string(crate::session::trigger_sidecar_path(&imported.session_path))
            .await
            .unwrap();
    let trigger_file: DynamicTriggerFile = serde_json::from_str(&triggers).unwrap();
    let enabled_rule = trigger_file
        .rules
        .iter()
        .find(|r| r.id == "was-enabled")
        .unwrap();
    let disabled_rule = trigger_file
        .rules
        .iter()
        .find(|r| r.id == "was-disabled")
        .unwrap();
    assert!(enabled_rule.enabled);
    assert!(
        enabled_rule.fired_at.is_some(),
        "fire-once history must survive activation"
    );
    assert!(
        !disabled_rule.enabled,
        "a rule the user disabled at the source must stay disabled"
    );

    let cron = tokio::fs::read_to_string(crate::session::cron_sidecar_path(&imported.session_path))
        .await
        .unwrap();
    let cron_file: CronJobsFile = toml::from_str(&cron).unwrap();
    let job_on = cron_file.jobs.iter().find(|j| j.id == "job-on").unwrap();
    let job_off = cron_file.jobs.iter().find(|j| j.id == "job-off").unwrap();
    assert!(job_on.enabled);
    assert!(
        !job_off.enabled,
        "a job the user disabled at the source must stay disabled"
    );
}

#[tokio::test]
async fn failed_sidecar_write_cleans_up_partial_import() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    tokio::fs::create_dir_all(&root).await.unwrap();

    let staging = HybridSessionRepo::new(temp.path().join("staging"));
    let session = staging.create("/tmp").await.unwrap();
    let entries = session.entries().await.unwrap();

    let session_path = root.join("imported.db");
    let temp_path = root.join("imported.db.tmp");
    let good_sidecar = root.join("imported.triggers.json");
    // A directory at the cron sidecar path makes its write fail mid-commit.
    let bad_sidecar = root.join("imported.cron.toml");
    tokio::fs::create_dir_all(&bad_sidecar).await.unwrap();

    let sidecars = vec![
        (
            good_sidecar.clone(),
            "{\"version\":1,\"rules\":[]}".to_string(),
        ),
        (bad_sidecar.clone(), "jobs = []".to_string()),
    ];
    let err = commit_import(
        &session_path,
        &temp_path,
        &entries,
        Path::new("/tmp"),
        None,
        &sidecars,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(err.contains("imported.cron.toml"), "{err}");

    assert!(
        !tokio::fs::try_exists(&session_path).await.unwrap(),
        "no orphan session may remain after a failed import"
    );
    assert!(!tokio::fs::try_exists(&temp_path).await.unwrap());
    assert!(
        !tokio::fs::try_exists(&good_sidecar).await.unwrap(),
        "sidecars written before the failure must be removed"
    );
}

#[tokio::test]
async fn import_summary_records_originally_enabled_automation_and_activates_it() {
    let temp = tempfile::tempdir().unwrap();
    let rules = vec![
        test_trigger_rule("was-enabled", true, false),
        test_trigger_rule("was-disabled", false, false),
    ];
    let jobs = vec![
        test_cron_job("job-on", true),
        test_cron_job("job-off", false),
    ];
    let (_, archive) = make_exported_session(temp.path(), rules, jobs).await;
    let dest_cwd = temp.path().join("dest");
    tokio::fs::create_dir_all(&dest_cwd).await.unwrap();
    let dest_repo = HybridSessionRepo::new(temp.path().join("dest-sessions"));
    let imported = import_session(&dest_repo, &archive, &dest_cwd, ActivateTriggers::Off)
        .await
        .unwrap();

    assert_eq!(imported.originally_enabled_triggers, vec!["was-enabled"]);
    assert_eq!(imported.originally_enabled_cron, vec!["job-on"]);

    let (t, c) = activate_imported(
        &imported.session_path,
        &imported.originally_enabled_triggers,
        &imported.originally_enabled_cron,
    )
    .expect("activation rewrites sidecars");
    assert_eq!((t, c), (1, 1));

    let triggers =
        tokio::fs::read_to_string(crate::session::trigger_sidecar_path(&imported.session_path))
            .await
            .unwrap();
    let trigger_file: DynamicTriggerFile = serde_json::from_str(&triggers).unwrap();
    let on = trigger_file
        .rules
        .iter()
        .find(|r| r.id == "was-enabled")
        .unwrap();
    let off = trigger_file
        .rules
        .iter()
        .find(|r| r.id == "was-disabled")
        .unwrap();
    assert!(on.enabled, "originally-enabled rule must be re-enabled");
    assert!(!off.enabled, "originally-disabled rule must stay disabled");

    let cron = tokio::fs::read_to_string(crate::session::cron_sidecar_path(&imported.session_path))
        .await
        .unwrap();
    let cron_file: CronJobsFile = toml::from_str(&cron).unwrap();
    assert!(
        cron_file
            .jobs
            .iter()
            .find(|j| j.id == "job-on")
            .unwrap()
            .enabled
    );
    assert!(
        !cron_file
            .jobs
            .iter()
            .find(|j| j.id == "job-off")
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn successful_import_leaves_no_temp_files() {
    let temp = tempfile::tempdir().unwrap();
    let (_, archive) = make_exported_session(temp.path(), vec![], vec![]).await;
    let dest_cwd = temp.path().join("dest");
    tokio::fs::create_dir_all(&dest_cwd).await.unwrap();
    let dest_repo = HybridSessionRepo::new(temp.path().join("dest-sessions"));
    import_session(&dest_repo, &archive, &dest_cwd, ActivateTriggers::Off)
        .await
        .unwrap();
    let mut dir = tokio::fs::read_dir(dest_repo.root()).await.unwrap();
    while let Some(entry) = dir.next_entry().await.unwrap() {
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(!name.ends_with(".tmp"), "leftover temp file {name}");
    }
}

fn manifest_from_archive(path: &Path) -> Manifest {
    let files = read_archive(path).unwrap();
    serde_json::from_slice(files.get(MANIFEST_PATH).unwrap()).unwrap()
}

fn write_test_archive(path: &Path, files: &BTreeMap<String, Vec<u8>>) {
    let file = std::fs::File::create(path).unwrap();
    let mut tar = tar::Builder::new(file);
    for (archive_path, bytes) in files {
        append_bytes(&mut tar, archive_path, bytes).unwrap();
    }
    tar.finish().unwrap();
}
