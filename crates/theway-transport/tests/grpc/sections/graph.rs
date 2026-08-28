// ── graph engine coverage (issue #86) ─────────────────────────────────

use crate::transport::SlashCompleter;
use crate::wire::{WireGraphCheckpoint, WireGraphKind};
use crate::TransportEndpoints;

fn sample_run(id: &str) -> WireDagRunSnapshot {
    WireDagRunSnapshot {
        id: id.into(),
        name: "goal".into(),
        kind: "goal".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 1,
        direction: "TD".into(),
        created_at: 1,
        completed_at: None,
        error: None,
        nodes: Vec::new(),
    }
}

struct CheckpointGraphOps {
    checkpoints: Vec<WireGraphCheckpoint>,
    restore_result: Result<String, String>,
}

impl GraphOps for CheckpointGraphOps {
    fn cancel_run(&self, _run_id: &str, _reason: Option<&str>) {}
    fn retry(&self, _run_id: &str, _node_ids: Option<&[String]>) -> Vec<String> {
        _node_ids.map(|ids| ids.to_vec()).unwrap_or_default()
    }
    fn skip(&self, _run_id: &str, _node_id: &str) -> bool {
        _node_id == "skip-me"
    }
    fn checkpoints(
        &self,
        _session_id: &str,
        _run_id: Option<&str>,
    ) -> anyhow::Result<Vec<WireGraphCheckpoint>> {
        Ok(self.checkpoints.clone())
    }
    fn restore(&self, _session_id: &str, _snapshot: &str) -> anyhow::Result<String> {
        match &self.restore_result {
            Ok(run_id) => Ok(run_id.clone()),
            Err(message) => anyhow::bail!("{message}"),
        }
    }
    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        if session_id == "sess-1" {
            vec![sample_run("run-1")]
        } else {
            Vec::new()
        }
    }
}

#[tokio::test]
async fn graph_cancel_retry_skip_interrupt_steer_are_plumbed() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    state.graph_ops = Arc::new(CheckpointGraphOps {
        checkpoints: Vec::new(),
        restore_result: Ok("restored-1".into()),
    });

    let cancel = state
        .graph_cancel(Request::new(theway_grpc::GraphCancelRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(cancel.accepted);

    let retry = state
        .graph_retry(Request::new(theway_grpc::GraphRetryRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: Some("n1".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(retry.reset_node_ids, vec!["n1"]);

    let retry_all = state
        .graph_retry(Request::new(theway_grpc::GraphRetryRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(retry_all.reset_node_ids.is_empty());

    let skip = state
        .graph_skip(Request::new(theway_grpc::GraphSkipRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "skip-me".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(skip.skipped);

    let interrupt = state
        .graph_node_interrupt(Request::new(theway_grpc::GraphNodeInterruptRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "n1".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!interrupt.accepted);

    let steer = state
        .graph_node_steer(Request::new(theway_grpc::GraphNodeSteerRequest {
            session_id: "sess-1".into(),
            run_id: "run-1".into(),
            node_id: "n1".into(),
            text: "go".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!steer.accepted);
}

#[tokio::test]
async fn graph_checkpoint_handles_empty_and_kinds() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    state.graph_ops = Arc::new(CheckpointGraphOps {
        checkpoints: Vec::new(),
        restore_result: Ok("restored-1".into()),
    });

    let empty = state
        .graph_checkpoint(Request::new(theway_grpc::GraphCheckpointRequest {
            session_id: Some("sess-1".into()),
            run_id: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(empty.error.is_some());

    state.graph_ops = Arc::new(CheckpointGraphOps {
        checkpoints: vec![
            WireGraphCheckpoint {
                kind: WireGraphKind::Goal,
                run_id: "goal-1".into(),
                snapshot: "snap-goal".into(),
            },
            WireGraphCheckpoint {
                kind: WireGraphKind::Dag,
                run_id: "dag-1".into(),
                snapshot: "snap-dag".into(),
            },
        ],
        restore_result: Ok("restored-1".into()),
    });
    let filled = state
        .graph_checkpoint(Request::new(theway_grpc::GraphCheckpointRequest {
            session_id: None,
            run_id: Some("goal-1".into()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(filled.session_id, "test-session");
    assert!(filled.error.is_none());
    assert_eq!(filled.checkpoints.len(), 2);
    assert_eq!(filled.checkpoints[0].kind, theway_grpc::GraphKind::GraphGoal as i32);
    assert_eq!(filled.checkpoints[1].kind, theway_grpc::GraphKind::GraphDag as i32);
}

#[tokio::test]
async fn graph_restore_success_and_error() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    state.graph_ops = Arc::new(CheckpointGraphOps {
        checkpoints: Vec::new(),
        restore_result: Ok("restored-1".into()),
    });
    let ok = state
        .graph_restore(Request::new(theway_grpc::GraphRestoreRequest {
            session_id: "sess-1".into(),
            snapshot: "snap".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(ok.run_id, "restored-1");
    assert!(ok.error.is_none());

    state.graph_ops = Arc::new(CheckpointGraphOps {
        checkpoints: Vec::new(),
        restore_result: Err("bad snapshot".into()),
    });
    let err = state
        .graph_restore(Request::new(theway_grpc::GraphRestoreRequest {
            session_id: "sess-1".into(),
            snapshot: "snap".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

#[test]
fn rpc_status_maps_unknown_code_to_internal() {
    let status = rpc_status(WireRpcError {
        code: "something_else".into(),
        message: "boom".into(),
    });
    assert_eq!(status.code(), tonic::Code::Internal);
    assert_eq!(status.message(), "boom");
}

struct FailingCheckpointGraphOps;

impl GraphOps for FailingCheckpointGraphOps {
    fn cancel_run(&self, _run_id: &str, _reason: Option<&str>) {}
    fn retry(&self, _run_id: &str, _node_ids: Option<&[String]>) -> Vec<String> {
        Vec::new()
    }
    fn skip(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }
    fn checkpoints(
        &self,
        _session_id: &str,
        _run_id: Option<&str>,
    ) -> anyhow::Result<Vec<WireGraphCheckpoint>> {
        anyhow::bail!("checkpoint store unavailable")
    }
    fn restore(&self, _session_id: &str, _snapshot: &str) -> anyhow::Result<String> {
        anyhow::bail!("restore unavailable")
    }
    fn list(&self, _session_id: &str) -> Vec<WireDagRunSnapshot> {
        Vec::new()
    }
}

#[tokio::test]
async fn graph_checkpoint_maps_store_errors_to_internal() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    state.graph_ops = Arc::new(FailingCheckpointGraphOps);
    let err = state
        .graph_checkpoint(Request::new(theway_grpc::GraphCheckpointRequest {
            session_id: None,
            run_id: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(err.message().contains("checkpoint store unavailable"));
}

struct FakeHost {
    endpoints: Option<TransportEndpoints>,
}

#[async_trait::async_trait(?Send)]
impl crate::host::TransportHost for FakeHost {
    fn transport_endpoints(&mut self) -> TransportEndpoints {
        self.endpoints.take().expect("endpoints already taken")
    }

    async fn run_transport_loop(
        self: Box<Self>,
        _mode: TransportMode,
        _endpoints: TransportEndpoints,
        server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        server_task.abort();
        Ok(())
    }
}

#[tokio::test]
async fn run_grpc_driver_binds_and_aborts_server_task() {
    let (state, command_rx, _ops, _tools) = grpc_state_with_ops();
    let endpoints = TransportEndpoints {
        command_tx: state.commands.clone(),
        command_rx,
        snapshot_tx: state.snapshots.clone(),
        latest: state.latest.clone(),
        events: state.events.clone(),
        dag_events: state.dag_events.clone(),
        completer: SlashCompleter::from_commands(Vec::new()),
        job_ops: state.job_ops.clone(),
        graph_ops: state.graph_ops.clone(),
        session_ops: state.session_ops.clone(),
        tool_ops: state.tool_ops.clone(),
        storage_ops: state.storage_ops.clone(),
        path_context: state.path_context.clone(),
        daemon_config: state.daemon_config.clone(),
        session_id: state.session_id.read().unwrap().clone(),
        agent_fwd: state.agent_fwd.clone(),
    };
    let host: Box<dyn crate::host::TransportHost> = Box::new(FakeHost {
        endpoints: Some(endpoints),
    });
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen_clone = seen.clone();
    let on_listen: Option<std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync>> =
        Some(std::sync::Arc::new(move |addr| {
            *seen_clone.lock().unwrap() = Some(addr);
        }));
    let options = GrpcOptions {
        host: "127.0.0.1".into(),
        port: 0,
        on_listen,
    };
    run_grpc(host, options).await.unwrap();
    assert!(seen.lock().unwrap().is_some());
}

#[derive(Default)]
struct SessionAwareGraphOps {
    runs: Mutex<HashMap<String, Vec<WireDagRunSnapshot>>>,
    cancelled: Mutex<Vec<String>>,
    retried: Mutex<Vec<String>>,
    skipped: Mutex<Vec<String>>,
    interrupted: Mutex<Vec<String>>,
    steered: Mutex<Vec<String>>,
}

impl GraphOps for SessionAwareGraphOps {
    fn cancel_run(&self, run_id: &str, _reason: Option<&str>) {
        self.cancelled.lock().push(run_id.to_string());
    }

    fn retry(&self, run_id: &str, _node_ids: Option<&[String]>) -> Vec<String> {
        self.retried.lock().push(run_id.to_string());
        Vec::new()
    }

    fn skip(&self, run_id: &str, _node_id: &str) -> bool {
        self.skipped.lock().push(run_id.to_string());
        true
    }

    fn checkpoints(
        &self,
        _session_id: &str,
        _run_id: Option<&str>,
    ) -> anyhow::Result<Vec<WireGraphCheckpoint>> {
        Ok(Vec::new())
    }

    fn restore(&self, _session_id: &str, _snapshot: &str) -> anyhow::Result<String> {
        Ok("restored".into())
    }

    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.runs
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }
}

impl SessionAwareGraphOps {
    fn seed(&self, session_id: &str, run_id: &str) {
        self.runs.lock().entry(session_id.into()).or_default().push(sample_run(run_id));
    }
}

#[tokio::test]
async fn graph_rpcs_reject_runs_from_other_sessions() {
    let (mut state, _rx, _ops, _tools) = grpc_state_with_ops();
    let graph = Arc::new(SessionAwareGraphOps::default());
    graph.seed("session-a", "run-a");
    graph.seed("session-b", "run-b");
    state.graph_ops = graph.clone();

    let err = state
        .graph_cancel(Request::new(theway_grpc::GraphCancelRequest {
            run_id: "run-b".into(),
            session_id: "session-a".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(graph.cancelled.lock().is_empty());

    let ok = state
        .graph_cancel(Request::new(theway_grpc::GraphCancelRequest {
            run_id: "run-a".into(),
            session_id: "session-a".into(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(ok.accepted);
    assert_eq!(graph.cancelled.lock().as_slice(), ["run-a"]);

    let err = state
        .graph_retry(Request::new(theway_grpc::GraphRetryRequest {
            run_id: "run-b".into(),
            node_id: None,
            session_id: "session-a".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(graph.retried.lock().is_empty());

    let err = state
        .graph_skip(Request::new(theway_grpc::GraphSkipRequest {
            run_id: "run-b".into(),
            node_id: "n1".into(),
            session_id: "session-a".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(graph.skipped.lock().is_empty());

    let err = state
        .graph_node_interrupt(Request::new(theway_grpc::GraphNodeInterruptRequest {
            run_id: "run-b".into(),
            node_id: "n1".into(),
            session_id: "session-a".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(graph.interrupted.lock().is_empty());

    let err = state
        .graph_node_steer(Request::new(theway_grpc::GraphNodeSteerRequest {
            run_id: "run-b".into(),
            node_id: "n1".into(),
            text: "go".into(),
            session_id: "session-a".into(),
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
    assert!(graph.steered.lock().is_empty());
}
