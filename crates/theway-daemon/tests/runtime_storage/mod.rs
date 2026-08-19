//! Tests for `runtime_storage` — split out of src (see docs/rust-test-files.md).

use super::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use theway_core::multiagent::graph::engine::NodeLauncher;
use theway_core::multiagent::graph::persist::{PersistedNode, PersistedRun};
use theway_core::multiagent::graph::types::{
    DagNodeDef, DagRunDef, Direction, NodeStatus, RunKind,
};
use theway_core::multiagent::jobs::JobTranscript;
use theway_transport::grpc::{serve_storage_service, StorageServiceState};
use theway_transport::transport::{SessionOps, StorageOps};
use theway_transport::wire::{
    SessionSummary, WireLoadCronJobsResult, WireLoadDagRunsResult, WireLoadTriggerRulesResult,
    WireSaveCronJobsResult, WireSaveDagRunResult, WireSaveTriggerRulesResult, WireStoredDagRun,
};

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn dynamic_rule(id: &str) -> DynamicTriggerRule {
    DynamicTriggerRule {
        id: id.to_string(),
        condition: "file_count > 1".to_string(),
        action: "notify".to_string(),
        enabled: true,
        fire_once: true,
        fired_at: Some(dt("2026-01-02T03:04:05Z")),
        promote_to_chat: true,
        created_at: dt("2026-01-01T00:00:00Z"),
    }
}

fn cron_job(id: &str) -> CronJob {
    CronJob {
        id: id.to_string(),
        schedule: "*/5 * * * *".to_string(),
        action: "run".to_string(),
        enabled: true,
        running_trace_id: Some("trace-1".to_string()),
        last_due_at: Some(dt("2026-01-02T03:04:05Z")),
        last_fired_at: Some(dt("2026-01-02T03:04:06Z")),
        last_completed_at: Some(dt("2026-01-02T03:04:07Z")),
        last_error: Some("boom".to_string()),
        skipped_overlap_count: 2,
        stateful: true,
        created_at: dt("2026-01-01T00:00:00Z"),
    }
}

fn persisted_run(session_id: &str, run_id: &str) -> PersistedRun {
    PersistedRun {
        id: run_id.to_string(),
        name: "persisted-name".to_string(),
        max_concurrency: 10,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 0,
        session_id: Some(session_id.to_string()),
        kind: RunKind::Dag,
        nodes: vec![PersistedNode {
            id: "n1".to_string(),
            agent: "main-agent".to_string(),
            task: "do the thing".to_string(),
            depends_on: vec![],
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
            status: NodeStatus::Ready,
            attempt: 0,
            started_at: None,
            completed_at: None,
            error: None,
            input_tokens: None,
            output_tokens: None,
            result: None,
            output: None,
            live_preview: None,
        }],
    }
}

fn node_def(id: &str) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "main-agent".to_string(),
        task: "do the thing".to_string(),
        depends_on: None,
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(name: &str) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes: vec![node_def("n1")],
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}

struct NoopLauncher;

impl NodeLauncher for NoopLauncher {
    fn launch(&self, _run_id: &str, _node_id: &str, _cancel: tokio_util::sync::CancellationToken) {}
}

// ── fakes for the controller-side gRPC seams ─────────────────────────────────

struct FakeSessionOps;

#[async_trait]
impl SessionOps for FakeSessionOps {
    async fn list(&self) -> anyhow::Result<Vec<SessionSummary>> {
        Ok(vec![])
    }

    async fn create(&self) -> anyhow::Result<String> {
        Ok("sess-new".to_string())
    }

    async fn rename(&self, _id: &str, _name: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn delete(&self, _id: &str) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
}

#[derive(Default)]
struct FakeStorageOps {
    dag_runs: std::sync::Mutex<Vec<WireStoredDagRun>>,
    trigger_rules: std::sync::Mutex<Vec<WireStoredTriggerRule>>,
    cron_jobs: std::sync::Mutex<Vec<WireStoredCronJob>>,
    saved_dag: std::sync::Mutex<Vec<WireSaveDagRunRequest>>,
    saved_triggers: std::sync::Mutex<Vec<WireSaveTriggerRulesRequest>>,
    saved_cron: std::sync::Mutex<Vec<WireSaveCronJobsRequest>>,
}

impl FakeStorageOps {
    fn with_dag_runs(mut self, dag_runs: Vec<WireStoredDagRun>) -> Self {
        self.dag_runs = std::sync::Mutex::new(dag_runs);
        self
    }

    fn with_trigger_rules(mut self, trigger_rules: Vec<WireStoredTriggerRule>) -> Self {
        self.trigger_rules = std::sync::Mutex::new(trigger_rules);
        self
    }

    fn with_cron_jobs(mut self, cron_jobs: Vec<WireStoredCronJob>) -> Self {
        self.cron_jobs = std::sync::Mutex::new(cron_jobs);
        self
    }
}

#[async_trait]
impl StorageOps for FakeStorageOps {
    async fn save_dag_run(
        &self,
        request: &WireSaveDagRunRequest,
    ) -> anyhow::Result<WireSaveDagRunResult> {
        self.saved_dag.lock().unwrap().push(request.clone());
        Ok(WireSaveDagRunResult { saved: true })
    }

    async fn load_dag_runs(
        &self,
        _request: &WireLoadDagRunsRequest,
    ) -> anyhow::Result<WireLoadDagRunsResult> {
        Ok(WireLoadDagRunsResult {
            runs: self.dag_runs.lock().unwrap().clone(),
        })
    }

    async fn save_trigger_rules(
        &self,
        request: &WireSaveTriggerRulesRequest,
    ) -> anyhow::Result<WireSaveTriggerRulesResult> {
        self.saved_triggers.lock().unwrap().push(request.clone());
        Ok(WireSaveTriggerRulesResult {
            count: request.rules.len() as u32,
        })
    }

    async fn load_trigger_rules(
        &self,
        _request: &WireLoadTriggerRulesRequest,
    ) -> anyhow::Result<WireLoadTriggerRulesResult> {
        Ok(WireLoadTriggerRulesResult {
            rules: self.trigger_rules.lock().unwrap().clone(),
        })
    }

    async fn save_cron_jobs(
        &self,
        request: &WireSaveCronJobsRequest,
    ) -> anyhow::Result<WireSaveCronJobsResult> {
        self.saved_cron.lock().unwrap().push(request.clone());
        Ok(WireSaveCronJobsResult {
            count: request.jobs.len() as u32,
        })
    }

    async fn load_cron_jobs(
        &self,
        _request: &WireLoadCronJobsRequest,
    ) -> anyhow::Result<WireLoadCronJobsResult> {
        Ok(WireLoadCronJobsResult {
            jobs: self.cron_jobs.lock().unwrap().clone(),
        })
    }
}

async fn start_storage_server(
    ops: Arc<FakeStorageOps>,
) -> (String, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = serve_storage_service(
        listener,
        StorageServiceState::new(Arc::new(FakeSessionOps), ops),
    );
    (addr, server)
}

// ── local runtime storage ─────────────────────────────────────────────────────

#[tokio::test]
async fn local_runtime_storage_opens_repo_and_disk_transcript_store() {
    // Arrange
    let _env_lock = crate::test_env::ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", dir.path());
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let storage = local_runtime_storage();

    // Act
    let repo = storage.session_repository(&cwd).await.unwrap();
    let transcript_store = storage.job_transcript_store(&cwd);
    let messages = vec![serde_json::json!({ "role": "user", "content": "hi" })];
    transcript_store.save(&JobTranscript {
        job_id: "job-1",
        run_id: Some("run-1"),
        node_id: Some("node-1"),
        messages: &messages,
    });

    // Assert
    assert!(repo.list().await.unwrap().is_empty());
    assert_eq!(
        transcript_store.load_node("run-1", "node-1").as_deref(),
        Some(messages.as_slice())
    );
}

#[tokio::test]
async fn local_runtime_storage_round_trips_sidecar_automation() {
    // Arrange
    let _env_lock = crate::test_env::ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _theway_dir = crate::test_env::EnvGuard::set("THEWAY_DIR", dir.path());
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let repo = theway_storage::session::open_repo(&cwd).await;
    let store = theway_storage::session::create(&repo, &cwd).await.unwrap();
    let metadata = theway_contract::session::SessionReader::get_metadata_json(&store)
        .await
        .unwrap();
    let session_id = metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let storage = local_runtime_storage();
    let rules = vec![dynamic_rule("rule-1")];
    let jobs = vec![cron_job("job-1")];

    // Act
    storage
        .save_dynamic_triggers(&cwd, &session_id, &rules)
        .await
        .unwrap();
    storage
        .save_cron_jobs(&cwd, &session_id, &jobs)
        .await
        .unwrap();
    let loaded_rules = storage
        .load_dynamic_triggers(&cwd, &session_id)
        .await
        .unwrap();
    let loaded_jobs = storage.load_cron_jobs(&cwd, &session_id).await.unwrap();

    // Assert
    assert_eq!(loaded_rules, rules);
    assert_eq!(loaded_jobs, jobs);
    assert!(storage
        .load_dag_runs(&cwd, &session_id)
        .await
        .unwrap()
        .is_empty());

    // Act: local DAG persistence goes through the same storage seam.
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    engine
        .plan(
            run_def("local-persisted-run"),
            None,
            Some(session_id.clone()),
        )
        .unwrap();
    let sink = storage.spawn_dag_persist(engine, cwd.clone());
    sink.flush().await;

    // Assert
    let dag_runs = storage.load_dag_runs(&cwd, &session_id).await.unwrap();
    assert_eq!(dag_runs.len(), 1);
    assert_eq!(dag_runs[0].name, "local-persisted-run");
}

// ── remote runtime storage ────────────────────────────────────────────────────

#[tokio::test]
async fn remote_runtime_storage_round_trips_triggers_and_cron() {
    // Arrange
    let ops = Arc::new(
        FakeStorageOps::default()
            .with_trigger_rules(vec![WireStoredTriggerRule {
                id: "rule-1".into(),
                condition: "file_count > 1".into(),
                action: "notify".into(),
                enabled: true,
                fire_once: true,
                fired_at: Some("2026-01-02T03:04:05Z".into()),
                promote_to_chat: true,
                created_at: "2026-01-01T00:00:00Z".into(),
            }])
            .with_cron_jobs(vec![WireStoredCronJob {
                id: "job-1".into(),
                schedule: "*/5 * * * *".into(),
                action: "run".into(),
                enabled: true,
                running_trace_id: Some("trace-1".into()),
                last_due_at: Some("2026-01-02T03:04:05Z".into()),
                last_fired_at: Some("2026-01-02T03:04:06Z".into()),
                last_completed_at: Some("2026-01-02T03:04:07Z".into()),
                last_error: Some("boom".into()),
                skipped_overlap_count: 2,
                stateful: true,
                created_at: "2026-01-01T00:00:00Z".into(),
            }]),
    );
    let (addr, _server) = start_storage_server(ops.clone()).await;

    // Act: exercise both the constructor and `RemoteRuntimeStorage::addr`.
    let storage = remote_runtime_storage(&addr).await.unwrap();
    let direct = RemoteRuntimeStorage::connect(&addr).await.unwrap();
    assert_eq!(direct.addr(), addr);

    let transcript_store = storage.job_transcript_store(std::path::Path::new("/tmp/cwd"));
    let messages = vec![serde_json::json!({ "role": "user", "content": "hi" })];
    transcript_store.save(&JobTranscript {
        job_id: "job-1",
        run_id: Some("run-1"),
        node_id: Some("node-1"),
        messages: &messages,
    });
    assert_eq!(
        transcript_store.load_node("run-1", "node-1").as_deref(),
        Some(messages.as_slice())
    );
    let loaded_rules = storage
        .load_dynamic_triggers(std::path::Path::new("/tmp/cwd"), "sess-1")
        .await
        .unwrap();
    let loaded_jobs = storage
        .load_cron_jobs(std::path::Path::new("/tmp/cwd"), "sess-1")
        .await
        .unwrap();
    storage
        .save_dynamic_triggers(
            std::path::Path::new("/tmp/cwd"),
            "sess-1",
            &[dynamic_rule("rule-2")],
        )
        .await
        .unwrap();
    storage
        .save_cron_jobs(
            std::path::Path::new("/tmp/cwd"),
            "sess-1",
            &[cron_job("job-2")],
        )
        .await
        .unwrap();

    // Assert
    assert_eq!(loaded_rules.len(), 1);
    assert_eq!(loaded_rules[0].id, "rule-1");
    assert_eq!(loaded_rules[0].fired_at, Some(dt("2026-01-02T03:04:05Z")));
    assert!(loaded_rules[0].promote_to_chat);
    assert_eq!(loaded_jobs.len(), 1);
    assert_eq!(loaded_jobs[0].id, "job-1");
    assert_eq!(loaded_jobs[0].running_trace_id.as_deref(), Some("trace-1"));
    assert!(loaded_jobs[0].stateful);
    assert_eq!(loaded_jobs[0].last_error.as_deref(), Some("boom"));

    let saved_triggers = ops.saved_triggers.lock().unwrap();
    assert_eq!(saved_triggers.len(), 1);
    assert_eq!(saved_triggers[0].session_id, "sess-1");
    assert_eq!(saved_triggers[0].rules.len(), 1);
    assert_eq!(saved_triggers[0].rules[0].id, "rule-2");
    assert_eq!(
        saved_triggers[0].rules[0].created_at,
        dt("2026-01-01T00:00:00Z").to_rfc3339()
    );

    let saved_cron = ops.saved_cron.lock().unwrap();
    assert_eq!(saved_cron.len(), 1);
    assert_eq!(saved_cron[0].session_id, "sess-1");
    assert_eq!(saved_cron[0].jobs.len(), 1);
    assert_eq!(saved_cron[0].jobs[0].id, "job-2");
    assert!(saved_cron[0].jobs[0].stateful);
}

#[tokio::test]
async fn remote_runtime_storage_load_dag_runs_parses_snapshots() {
    // Arrange
    let snapshot = serde_json::to_string(&persisted_run("sess-1", "dag-1")).unwrap();
    let ops = Arc::new(
        FakeStorageOps::default().with_dag_runs(vec![WireStoredDagRun {
            session_id: "sess-1".into(),
            run_id: "dag-1".into(),
            snapshot,
        }]),
    );
    let (addr, _server) = start_storage_server(ops).await;
    let storage = remote_runtime_storage(&addr).await.unwrap();

    // Act
    let runs = storage
        .load_dag_runs(std::path::Path::new("/tmp/cwd"), "sess-1")
        .await
        .unwrap();

    // Assert
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "dag-1");
    assert_eq!(runs[0].session_id.as_deref(), Some("sess-1"));
    assert_eq!(runs[0].nodes.len(), 1);
    assert_eq!(runs[0].nodes[0].status, NodeStatus::Ready);
}

#[tokio::test]
async fn remote_runtime_storage_load_dag_runs_rejects_bad_snapshot() {
    // Arrange
    let ops = Arc::new(
        FakeStorageOps::default().with_dag_runs(vec![WireStoredDagRun {
            session_id: "sess-1".into(),
            run_id: "dag-1".into(),
            snapshot: "not json".into(),
        }]),
    );
    let (addr, _server) = start_storage_server(ops).await;
    let storage = remote_runtime_storage(&addr).await.unwrap();

    // Act
    let err = storage
        .load_dag_runs(std::path::Path::new("/tmp/cwd"), "sess-1")
        .await
        .unwrap_err();

    // Assert
    assert!(
        err.to_string().contains("parse remote DAG snapshot"),
        "{err}"
    );
}

#[tokio::test]
async fn remote_dag_persist_saves_running_runs_as_snapshots() {
    // Arrange
    let ops = Arc::new(FakeStorageOps::default());
    let (addr, _server) = start_storage_server(ops.clone()).await;
    let storage = RemoteRuntimeStorage::connect(&addr).await.unwrap();
    let engine = Arc::new(DagEngine::new());
    engine.set_launcher(Some(Arc::new(NoopLauncher)));
    engine
        .plan(run_def("remote-run"), None, Some("sess-1".into()))
        .unwrap();
    let handle = RemoteDagPersistHandle::spawn(
        engine.clone(),
        std::path::PathBuf::from("/tmp/cwd"),
        storage,
    );

    // Act
    handle.flush().await;

    // Assert
    let saved = ops.saved_dag.lock().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].session_id, "sess-1");
    assert_eq!(saved[0].run_id, "dag-1");
    let persisted: PersistedRun = serde_json::from_str(&saved[0].snapshot).unwrap();
    assert_eq!(persisted.name, "remote-run");
    assert_eq!(persisted.nodes.len(), 1);

    if let Some(task) = handle.task.lock().take() {
        task.abort();
    }
}

// ── wire conversion helpers ───────────────────────────────────────────────────

#[test]
fn trigger_wire_conversions_round_trip() {
    // Arrange
    let rule = dynamic_rule("rule-1");

    // Act
    let wire = trigger_to_wire(&rule);
    let back = trigger_from_wire(&wire).unwrap();

    // Assert
    assert_eq!(back, rule);
    assert_eq!(
        wire.fired_at.as_deref(),
        Some(dt("2026-01-02T03:04:05Z").to_rfc3339()).as_deref()
    );
}

#[test]
fn trigger_from_wire_rejects_invalid_timestamps() {
    // Arrange
    let wire = WireStoredTriggerRule {
        id: "rule-1".into(),
        condition: "c".into(),
        action: "a".into(),
        enabled: true,
        fire_once: true,
        fired_at: Some("not-a-time".into()),
        promote_to_chat: false,
        created_at: "2026-01-01T00:00:00Z".into(),
    };

    // Act
    let err = trigger_from_wire(&wire).unwrap_err();

    // Assert
    assert!(err.to_string().contains("invalid RFC3339"), "{err}");
}

#[test]
fn cron_wire_conversions_round_trip() {
    // Arrange
    let job = cron_job("job-1");

    // Act
    let wire = cron_to_wire(&job);
    let back = cron_from_wire(&wire).unwrap();

    // Assert
    assert_eq!(back, job);
    assert_eq!(
        wire.last_completed_at.as_deref(),
        Some(dt("2026-01-02T03:04:07Z").to_rfc3339()).as_deref()
    );
}

#[test]
fn cron_from_wire_rejects_invalid_created_at() {
    // Arrange
    let wire = WireStoredCronJob {
        id: "job-1".into(),
        schedule: "*/5 * * * *".into(),
        action: "run".into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: "not-a-time".into(),
    };

    // Act
    let err = cron_from_wire(&wire).unwrap_err();

    // Assert
    assert!(err.to_string().contains("invalid RFC3339"), "{err}");
}

#[test]
fn parse_rfc3339_rejects_invalid_timestamps() {
    // Act
    let err = parse_rfc3339("yesterday").unwrap_err();

    // Assert
    assert!(err.to_string().contains("invalid RFC3339"), "{err}");
}
