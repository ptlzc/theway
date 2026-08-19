// ── runtime state storage (issue #84) ──────────────────────────────

#[tokio::test]
async fn client_state_storage_round_trips_dag_trigger_cron() {
    use crate::wire::{
        WireLoadCronJobsRequest, WireLoadDagRunsRequest, WireLoadTriggerRulesRequest,
        WireSaveCronJobsRequest, WireSaveDagRunRequest, WireSaveTriggerRulesRequest,
        WireStoredCronJob, WireStoredTriggerRule,
    };

    let (mut state, _command_rx) = grpc_state();
    let storage = Arc::new(FakeStorageOps::new());
    state.storage_ops = storage.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = serve_grpc(listener, state);
    let mut client = GrpcClient::connect(&addr.to_string()).await.unwrap();

    // DAG run save/load.
    let saved = client
        .state_save_dag_run(&WireSaveDagRunRequest {
            session_id: "sess-1".into(),
            run_id: "dag-1".into(),
            snapshot: r#"{"id":"dag-1"}"#.into(),
        })
        .await
        .unwrap();
    assert!(saved.saved);
    let loaded = client
        .state_load_dag_runs(&WireLoadDagRunsRequest {
            session_id: "sess-1".into(),
            run_id: None,
        })
        .await
        .unwrap();
    assert_eq!(loaded.runs.len(), 1);
    assert_eq!(loaded.runs[0].run_id, "dag-1");

    // Trigger rules save/load.
    let saved = client
        .state_save_trigger_rules(&WireSaveTriggerRulesRequest {
            session_id: "sess-1".into(),
            rules: vec![WireStoredTriggerRule {
                id: "tr-1".into(),
                condition: "file changes".into(),
                action: "run tests".into(),
                enabled: true,
                fire_once: false,
                fired_at: None,
                promote_to_chat: true,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(saved.count, 1);
    let loaded = client
        .state_load_trigger_rules(&WireLoadTriggerRulesRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap();
    assert_eq!(loaded.rules.len(), 1);
    assert_eq!(loaded.rules[0].id, "tr-1");

    // Cron jobs save/load.
    let saved = client
        .state_save_cron_jobs(&WireSaveCronJobsRequest {
            session_id: "sess-1".into(),
            jobs: vec![WireStoredCronJob {
                id: "cron-1".into(),
                schedule: "*/5 * * * *".into(),
                action: "backup".into(),
                enabled: true,
                running_trace_id: None,
                last_due_at: None,
                last_fired_at: None,
                last_completed_at: None,
                last_error: None,
                skipped_overlap_count: 0,
                stateful: false,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(saved.count, 1);
    let loaded = client
        .state_load_cron_jobs(&WireLoadCronJobsRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap();
    assert_eq!(loaded.jobs.len(), 1);
    assert_eq!(loaded.jobs[0].id, "cron-1");
}
