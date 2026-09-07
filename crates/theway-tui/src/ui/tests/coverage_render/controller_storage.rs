use super::*;

#[tokio::test]
async fn controller_storage_sessions_and_sidecars_cover_all_ops() {
    use std::collections::HashMap;

    let tmp = tempfile::tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(tmp.path()));
    let created = repo.create("/cwd".to_string()).await.unwrap();
    let sid = crate::startup::session_id_of(&created).await;
    drop(created);

    let ops = ControllerSessionOps::new(repo.clone(), tmp.path().to_path_buf());
    let summaries = ops.list().await.unwrap();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert!(!summaries[0].session_id.is_empty());

    let explicit = ops
        .create(Some("custom-id"), &HashMap::new())
        .await
        .unwrap();
    assert_eq!(explicit, "custom-id");
    ops.update_metadata(&explicit, &HashMap::new())
        .await
        .unwrap();
    ops.rename(&explicit, "named").await.unwrap();
    assert!(ops.rename(&explicit, "  ").await.is_err());
    ops.delete(&explicit).await.unwrap();
    assert!(
        ops.update_metadata("missing", &HashMap::new())
            .await
            .is_err()
    );
    assert!(ops.rename("missing", "x").await.is_err());

    let storage = ControllerStorageOps::new(repo.clone());
    let save_req = theway_transport::wire::WireSaveDagRunRequest {
        session_id: sid.clone(),
        run_id: "r1".into(),
        snapshot: "{}".into(),
    };
    storage.save_dag_run(&save_req).await.unwrap();
    let upd = theway_transport::wire::WireSaveDagRunRequest {
        session_id: sid.clone(),
        run_id: "r1".into(),
        snapshot: "{\"x\":1}".into(),
    };
    storage.save_dag_run(&upd).await.unwrap();
    let one = storage
        .load_dag_runs(&theway_transport::wire::WireLoadDagRunsRequest {
            session_id: sid.clone(),
            run_id: Some("r1".into()),
        })
        .await
        .unwrap();
    assert_eq!(one.runs.len(), 1);
    let all = storage
        .load_dag_runs(&theway_transport::wire::WireLoadDagRunsRequest {
            session_id: sid.clone(),
            run_id: None,
        })
        .await
        .unwrap();
    assert_eq!(all.runs.len(), 1);

    let rule = DynamicTriggerRule {
        id: "tr".into(),
        condition: "c".into(),
        action: "a".into(),
        enabled: true,
        fire_once: false,
        fired_at: Some(chrono::Utc::now()),
        promote_to_chat: true,
        created_at: chrono::Utc::now(),
    };
    let saved_rules = storage
        .save_trigger_rules(&theway_transport::wire::WireSaveTriggerRulesRequest {
            session_id: sid.clone(),
            rules: vec![trigger_to_wire(&rule)],
        })
        .await
        .unwrap();
    assert_eq!(saved_rules.count, 1);
    let loaded_rules = storage
        .load_trigger_rules(&theway_transport::wire::WireLoadTriggerRulesRequest {
            session_id: sid.clone(),
        })
        .await
        .unwrap();
    assert_eq!(loaded_rules.rules.len(), 1);
    assert_eq!(trigger_from_wire(&loaded_rules.rules[0]).unwrap().id, "tr");

    let job = CronJob {
        id: "cj".into(),
        schedule: "* * * * *".into(),
        action: "a".into(),
        enabled: true,
        running_trace_id: Some("trace".into()),
        last_due_at: Some(chrono::Utc::now()),
        last_fired_at: Some(chrono::Utc::now()),
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 2,
        stateful: true,
        created_at: chrono::Utc::now(),
    };
    storage
        .save_cron_jobs(&theway_transport::wire::WireSaveCronJobsRequest {
            session_id: sid.clone(),
            jobs: vec![cron_to_wire(&job)],
        })
        .await
        .unwrap();
    let loaded_jobs = storage
        .load_cron_jobs(&theway_transport::wire::WireLoadCronJobsRequest {
            session_id: sid.clone(),
        })
        .await
        .unwrap();
    assert_eq!(loaded_jobs.jobs.len(), 1);
    assert_eq!(cron_from_wire(&loaded_jobs.jobs[0]).unwrap().id, "cj");
}

#[tokio::test]
async fn controller_storage_sidecar_direct_read_write_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let trigger = tmp.path().join("triggers.json");
    let cron = tmp.path().join("cron.toml");
    assert!(read_trigger_rules(&trigger).await.unwrap().is_empty());
    assert!(read_cron_jobs(&cron).await.unwrap().is_empty());

    let rule = DynamicTriggerRule {
        id: "r".into(),
        condition: "c".into(),
        action: "a".into(),
        enabled: true,
        fire_once: true,
        fired_at: None,
        promote_to_chat: false,
        created_at: chrono::Utc::now(),
    };
    write_trigger_rules(&trigger, std::slice::from_ref(&rule))
        .await
        .unwrap();
    let loaded = read_trigger_rules(&trigger).await.unwrap();
    assert_eq!(loaded.len(), 1);

    std::fs::write(&trigger, "").unwrap();
    assert!(read_trigger_rules(&trigger).await.unwrap().is_empty());
    std::fs::write(&trigger, "{not json").unwrap();
    assert!(read_trigger_rules(&trigger).await.is_err());

    let job = CronJob {
        id: "c".into(),
        schedule: "* * * * *".into(),
        action: "a".into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: chrono::Utc::now(),
    };
    write_cron_jobs(&cron, &[job]).await.unwrap();
    assert_eq!(read_cron_jobs(&cron).await.unwrap().len(), 1);
    std::fs::write(&cron, "not toml [[[").unwrap();
    assert!(read_cron_jobs(&cron).await.is_err());

    assert!(parse_rfc3339("2026-01-01T00:00:00Z").is_ok());
    assert!(parse_rfc3339("nope").is_err());
    assert_eq!(sanitize("abc-123_9"), "abc-123_9");
    assert_eq!(sanitize("a/b"), "a_b");
    assert_eq!(sanitize(""), "default");
    assert_eq!(sanitize(&"x".repeat(100)).len(), 80);
}
