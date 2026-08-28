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

// ── session-resource methods mirrored on StorageService ─────────────

#[tokio::test]
async fn client_state_list_sessions_returns_summaries_and_current() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;

    let (sessions, current) = client.state_list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "sess-1");
    assert_eq!(current, "sess-1");
}

#[tokio::test]
async fn client_state_create_session_round_trips() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;

    let created = client.state_create_session(Some("new one".into())).await.unwrap();
    assert_eq!(created.name, "new one");
    assert!(created.session_id.starts_with("sess-new-"));
}

#[tokio::test]
async fn client_state_rename_session_returns_accepted() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;
    assert!(client.state_rename_session("sess-1", "renamed").await.unwrap());

    let (sessions, _current) = client.state_list_sessions().await.unwrap();
    assert_eq!(sessions[0].name, "renamed");
}

#[tokio::test]
async fn client_state_delete_session_removes_and_returns_running_ids_on_refusal() {
    let (mut state, _command_rx) = grpc_state();
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-keep");
    session_ops.add_session("sess-run");
    session_ops.set_running("sess-run", &["run-1"]);
    state.session_ops = session_ops;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = serve_grpc(listener, state);
    let mut client = GrpcClient::connect(&addr.to_string()).await.unwrap();

    // Running session: delete is refused through the RPC error path.
    let err = client.state_delete_session("sess-run").await.unwrap_err();
    assert!(err.to_string().contains("still has running graphs"), "{err}");

    // Non-running session: delete succeeds with an empty vec.
    let removed = client.state_delete_session("sess-keep").await.unwrap();
    assert!(removed.is_empty());
    let (sessions, _current) = client.state_list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "sess-run");
}