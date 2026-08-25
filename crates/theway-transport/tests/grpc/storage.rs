//! Focused coverage for the storage-only gRPC service and the `StorageService`
//! implementation on `GrpcState`.

use super::*;
use crate::proto::theway_grpc::storage_service_server::StorageService;
use crate::proto::theway_grpc::{
    CreateSessionRequest, DeleteSessionRequest, Empty, RenameSessionRequest,
};
use crate::testing::{FakeSessionOps, FakeStorageOps};

fn storage_state() -> (StorageServiceState, Arc<FakeSessionOps>, Arc<FakeStorageOps>) {
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    let storage_ops = Arc::new(FakeStorageOps::new());
    (
        StorageServiceState::new(session_ops.clone(), storage_ops.clone()),
        session_ops,
        storage_ops,
    )
}

#[tokio::test]
async fn storage_only_service_sessions_and_persistence_round_trip() {
    let (state, ops, _storage) = storage_state();
    ops.add_session("sess-2");

    let list = StorageService::list_sessions(&state, Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.sessions.len(), 2);
    assert!(list.current_session_id.is_empty());

    let created = state
        .create_session(Request::new(CreateSessionRequest {
            name: Some("brand new".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    let id = created.session.unwrap().session_id;
    assert!(id.starts_with("sess-new-"));

    let renamed = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: id.clone(),
            name: "renamed".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(renamed.accepted);

    let deleted = state
        .delete_session(Request::new(DeleteSessionRequest {
            session_id: id.clone(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(deleted.running_run_ids.is_empty());
}

#[tokio::test]
async fn storage_only_service_error_paths() {
    let (state, ops, _storage) = storage_state();
    ops.set_running("sess-1", &["run-1"]);

    let err = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: "nope".into(),
            name: "x".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    let err = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: "sess-1".into(),
            name: "  ".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let err = state
        .delete_session(Request::new(DeleteSessionRequest {
            session_id: "nope".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    let err = state
        .delete_session(Request::new(DeleteSessionRequest {
            session_id: "sess-1".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert!(err.message().contains("run-1"));
}

#[tokio::test]
async fn grpc_state_storage_session_methods_are_covered() {
    let (state, mut command_rx, ops, _tools) = grpc_state_with_ops();
    ops.add_session("other");

    let list = StorageService::list_sessions(&state, Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.current_session_id, "test-session");
    assert_eq!(list.sessions.len(), 2);

    let created = StorageService::create_session(&state, Request::new(CreateSessionRequest {
            name: Some("created".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    let id = created.session.unwrap().session_id;
    match command_rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id: switched } => assert_eq!(switched, id),
        other => panic!("unexpected command: {other:?}"),
    }

    let renamed = StorageService::rename_session(&state, Request::new(RenameSessionRequest {
            session_id: "test-session".into(),
            name: "renamed".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(renamed.accepted);

    let deleted = StorageService::delete_session(&state, Request::new(DeleteSessionRequest {
            session_id: "other".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(deleted.running_run_ids.is_empty());
}

#[tokio::test]
async fn grpc_state_storage_delete_current_falls_back() {
    let (state, mut command_rx, ops, _tools) = grpc_state_with_ops();
    ops.add_session("next");

    let deleted = StorageService::delete_session(&state, Request::new(DeleteSessionRequest {
            session_id: "test-session".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(deleted.running_run_ids.is_empty());
    assert_eq!(*state.session_id.read().unwrap(), "next");
    match command_rx.recv().await.unwrap() {
        WireCommand::SwitchSession { id } => assert_eq!(id, "next"),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn storage_only_service_round_trip_over_transport() {
    let (state, _ops, _storage) = storage_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_storage_service(listener, state);

    let mut client = theway_grpc::storage_service_client::StorageServiceClient::connect(format!(
        "http://{addr}"
    ))
    .await
    .unwrap();
    let response = client
        .list_sessions(Empty {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(response.sessions.len(), 1);

    let created = client
        .create_session(CreateSessionRequest {
            name: Some("wire".into()),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(created.session.is_some());

    server.abort();
}

#[tokio::test]
async fn storage_only_service_persistence_round_trip() {
    let (state, _ops, storage) = storage_state();
    storage.put_dag_run("sess-1", "dag-old", "old");
    storage.put_trigger_rules("sess-1", vec![]);
    storage.put_cron_jobs("sess-1", vec![]);

    let saved = state
        .save_dag_run(Request::new(theway_grpc::SaveDagRunRequest {
            session_id: "sess-1".into(),
            run_id: "dag-1".into(),
            snapshot: r#"{"id":"dag-1"}"#.into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(saved.saved);
    let loaded = state
        .load_dag_runs(Request::new(theway_grpc::LoadDagRunsRequest {
            session_id: "sess-1".into(),
            run_id: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(loaded.runs.iter().any(|r| r.run_id == "dag-1"));

    let saved = state
        .save_trigger_rules(Request::new(theway_grpc::SaveTriggerRulesRequest {
            session_id: "sess-1".into(),
            rules: vec![theway_grpc::StoredTriggerRule {
                id: "tr-1".into(),
                condition: "c".into(),
                action: "a".into(),
                enabled: true,
                fire_once: false,
                fired_at: None,
                promote_to_chat: false,
                created_at: "now".into(),
            }],
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.count, 1);
    let loaded = state
        .load_trigger_rules(Request::new(theway_grpc::LoadTriggerRulesRequest {
            session_id: "sess-1".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.rules.len(), 1);

    let saved = state
        .save_cron_jobs(Request::new(theway_grpc::SaveCronJobsRequest {
            session_id: "sess-1".into(),
            jobs: vec![theway_grpc::StoredCronJob {
                id: "cron-1".into(),
                schedule: "* * * * *".into(),
                action: "backup".into(),
                enabled: true,
                running_trace_id: None,
                last_due_at: None,
                last_fired_at: None,
                last_completed_at: None,
                last_error: None,
                skipped_overlap_count: 0,
                stateful: false,
                created_at: "now".into(),
            }],
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved.count, 1);
    let loaded = state
        .load_cron_jobs(Request::new(theway_grpc::LoadCronJobsRequest {
            session_id: "sess-1".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(loaded.jobs.len(), 1);
}

#[tokio::test]
async fn storage_only_service_maps_storage_failures_to_internal() {
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    let state = StorageServiceState::new(session_ops, Arc::new(crate::UnavailableStorageOps));

    let err = state
        .save_dag_run(Request::new(theway_grpc::SaveDagRunRequest {
            session_id: "sess-1".into(),
            run_id: "r".into(),
            snapshot: "s".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .load_dag_runs(Request::new(theway_grpc::LoadDagRunsRequest {
            session_id: "sess-1".into(),
            run_id: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .save_trigger_rules(Request::new(theway_grpc::SaveTriggerRulesRequest {
            session_id: "sess-1".into(),
            rules: vec![],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .load_trigger_rules(Request::new(theway_grpc::LoadTriggerRulesRequest {
            session_id: "sess-1".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .save_cron_jobs(Request::new(theway_grpc::SaveCronJobsRequest {
            session_id: "sess-1".into(),
            jobs: vec![],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .load_cron_jobs(Request::new(theway_grpc::LoadCronJobsRequest {
            session_id: "sess-1".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);
}

#[derive(Default)]
struct FailingSessionOps;

#[async_trait::async_trait]
impl crate::transport::SessionOps for FailingSessionOps {
    async fn list(&self) -> anyhow::Result<Vec<crate::wire::SessionSummary>> {
        anyhow::bail!("list failed")
    }
    async fn create(&self) -> anyhow::Result<String> {
        anyhow::bail!("create failed")
    }
    async fn rename(&self, _id: &str, _name: &str) -> anyhow::Result<()> {
        anyhow::bail!("rename failed")
    }
    async fn delete(&self, _id: &str) -> anyhow::Result<Vec<String>> {
        anyhow::bail!("delete failed")
    }
}

#[tokio::test]
async fn storage_only_service_maps_session_failures() {
    let state = StorageServiceState::new(Arc::new(FailingSessionOps), Arc::new(FakeStorageOps::new()));

    let err = state
        .list_sessions(Request::new(Empty {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .create_session(Request::new(CreateSessionRequest {
            name: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = state
        .rename_session(Request::new(RenameSessionRequest {
            session_id: "x".into(),
            name: "y".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let err = state
        .delete_session(Request::new(DeleteSessionRequest {
            session_id: "x".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);
}

#[tokio::test]
async fn grpc_state_storage_maps_session_failures() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    state.session_ops = Arc::new(FailingSessionOps);

    let err = StorageService::list_sessions(&state, Request::new(Empty {}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = StorageService::create_session(&state, Request::new(CreateSessionRequest {
        name: None,
    }))
    .await
    .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let err = StorageService::rename_session(&state, Request::new(RenameSessionRequest {
        session_id: "x".into(),
        name: "y".into(),
    }))
    .await
    .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let err = StorageService::delete_session(&state, Request::new(DeleteSessionRequest {
        session_id: "x".into(),
    }))
    .await
    .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);
}

struct FlakySessionOps {
    inner: Arc<FakeSessionOps>,
    fail_list: bool,
    fail_create: bool,
    fail_rename: bool,
    fail_delete: bool,
}

#[async_trait::async_trait]
impl crate::transport::SessionOps for FlakySessionOps {
    async fn list(&self) -> anyhow::Result<Vec<crate::wire::SessionSummary>> {
        if self.fail_list {
            anyhow::bail!("list failed")
        }
        self.inner.list().await
    }
    async fn create(&self) -> anyhow::Result<String> {
        if self.fail_create {
            anyhow::bail!("create failed")
        }
        self.inner.create().await
    }
    async fn rename(&self, id: &str, name: &str) -> anyhow::Result<()> {
        if self.fail_rename {
            anyhow::bail!("rename failed")
        }
        self.inner.rename(id, name).await
    }
    async fn delete(&self, id: &str) -> anyhow::Result<Vec<String>> {
        if self.fail_delete {
            anyhow::bail!("delete failed")
        }
        self.inner.delete(id).await
    }
}

#[tokio::test]
async fn storage_only_service_scripted_session_failures() {
    let inner = Arc::new(FakeSessionOps::new());
    inner.add_session("sess-1");

    let state = StorageServiceState::new(
        Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: false, fail_create: false, fail_rename: true, fail_delete: false }),
        Arc::new(FakeStorageOps::new()),
    );
    let err = state
        .create_session(Request::new(CreateSessionRequest { name: Some("x".into()) }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let state = StorageServiceState::new(
        Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: true, fail_create: false, fail_rename: false, fail_delete: false }),
        Arc::new(FakeStorageOps::new()),
    );
    let err = state
        .create_session(Request::new(CreateSessionRequest { name: None }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let state = StorageServiceState::new(
        Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: false, fail_create: false, fail_rename: false, fail_delete: true }),
        Arc::new(FakeStorageOps::new()),
    );
    let err = state
        .delete_session(Request::new(DeleteSessionRequest { session_id: "sess-1".into() }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);
}

#[tokio::test]
async fn grpc_state_storage_scripted_session_failures() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let inner = Arc::new(FakeSessionOps::new());
    inner.add_session("test-session");
    state.session_ops = Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: false, fail_create: false, fail_rename: true, fail_delete: false });
    let err = StorageService::create_session(&state, Request::new(CreateSessionRequest { name: Some("x".into()) }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let inner = Arc::new(FakeSessionOps::new());
    inner.add_session("test-session");
    state.session_ops = Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: true, fail_create: false, fail_rename: false, fail_delete: false });
    let err = StorageService::create_session(&state, Request::new(CreateSessionRequest { name: None }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);

    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let inner = Arc::new(FakeSessionOps::new());
    inner.add_session("test-session");
    state.session_ops = Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: false, fail_create: false, fail_rename: true, fail_delete: false });
    let err = StorageService::rename_session(&state, Request::new(RenameSessionRequest { session_id: "test-session".into(), name: "x".into() }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let inner = Arc::new(FakeSessionOps::new());
    inner.add_session("other");
    state.session_ops = Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: false, fail_create: false, fail_rename: false, fail_delete: false });
    let err = StorageService::delete_session(&state, Request::new(DeleteSessionRequest { session_id: "nope".into() }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);

    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let inner = Arc::new(FakeSessionOps::new());
    inner.add_session("test-session");
    state.session_ops = Arc::new(FlakySessionOps { inner: inner.clone(), fail_list: false, fail_create: false, fail_rename: false, fail_delete: true });
    let err = StorageService::delete_session(&state, Request::new(DeleteSessionRequest { session_id: "test-session".into() }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);
}
