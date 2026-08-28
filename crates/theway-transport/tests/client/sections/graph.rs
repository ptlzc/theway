// ── graph client methods ─────────────────────────────────────────────

use std::sync::Mutex;

use crate::wire::{WireDagRunSnapshot, WireNodeOutput};

#[derive(Default)]
struct ClientGraphOps {
    cancelled: Mutex<Vec<(String, Option<String>)>>,
    retried: Mutex<Vec<(String, Option<String>)>>,
    skipped: Mutex<Vec<(String, String)>>,
    runs: Mutex<Vec<WireDagRunSnapshot>>,
}

impl crate::GraphOps for ClientGraphOps {
    fn cancel_run(&self, run_id: &str, reason: Option<&str>) {
        self.cancelled
            .lock()
            .unwrap()
            .push((run_id.to_string(), reason.map(str::to_string)));
    }

    fn retry(&self, run_id: &str, node_ids: Option<&[String]>) -> Vec<String> {
        self.retried
            .lock()
            .unwrap()
            .push((run_id.to_string(), node_ids.map(|ids| ids.join(","))));
        node_ids
            .map(|ids| ids.to_vec())
            .unwrap_or_else(|| vec!["all".to_string()])
    }

    fn skip(&self, run_id: &str, node_id: &str) -> bool {
        self.skipped
            .lock()
            .unwrap()
            .push((run_id.to_string(), node_id.to_string()));
        true
    }

    fn checkpoints(
        &self,
        _session_id: &str,
        _run_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::wire::WireGraphCheckpoint>> {
        Ok(Vec::new())
    }

    fn restore(&self, _session_id: &str, _snapshot: &str) -> anyhow::Result<String> {
        Ok("restored-run".to_string())
    }

    fn list(&self, _session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.runs.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct ClientJobOps {
    output: WireNodeOutput,
}

impl crate::JobOps for ClientJobOps {
    fn node_output(&self, _run_id: &str, _node_id: &str) -> WireNodeOutput {
        self.output.clone()
    }

    fn interrupt_node(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }

    fn steer_node(&self, _run_id: &str, _node_id: &str, _text: String) -> bool {
        false
    }
}

async fn client_and_server_with_graph(
    graph: Arc<ClientGraphOps>,
    jobs: Arc<ClientJobOps>,
) -> (
    GrpcClient,
    mpsc::UnboundedReceiver<crate::wire::WireCommand>,
    broadcast::Sender<WireStatusUpdate>,
) {
    let (mut state, command_rx) = grpc_state();
    state.graph_ops = graph;
    state.job_ops = jobs;
    let snapshot_tx = state.snapshots.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = serve_grpc(listener, state);
    let client = GrpcClient::connect(&addr.to_string()).await.unwrap();
    (client, command_rx, snapshot_tx)
}

fn client_run(id: &str) -> WireDagRunSnapshot {
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

#[tokio::test]
async fn client_graph_cancel_retry_skip_round_trip() {
    let graph = Arc::new(ClientGraphOps::default());
    graph.runs.lock().unwrap().push(client_run("run-1"));
    let (mut client, _command_rx, _snapshot_tx) =
        client_and_server_with_graph(graph.clone(), Arc::new(ClientJobOps::default())).await;

    assert!(client.graph_cancel("sess-1", "run-1").await.unwrap());
    let cancelled = graph.cancelled.lock().unwrap();
    assert_eq!(cancelled[0].0, "run-1");
    assert_eq!(cancelled[0].1.as_deref(), Some("cancelled via rpc"));
    drop(cancelled);

    let reset = client
        .graph_retry("sess-1", "run-1", Some("node-1"))
        .await
        .unwrap();
    assert_eq!(reset, vec!["node-1"]);
    let retried = graph.retried.lock().unwrap();
    assert_eq!(retried[0].0, "run-1");
    assert_eq!(retried[0].1.as_deref(), Some("node-1"));
    drop(retried);

    let reset_all = client.graph_retry("sess-1", "run-1", None).await.unwrap();
    assert_eq!(reset_all, vec!["all"]);

    assert!(client.graph_skip("sess-1", "run-1", "node-1").await.unwrap());
    let skipped = graph.skipped.lock().unwrap();
    assert_eq!(skipped[0], ("run-1".to_string(), "node-1".to_string()));
}

#[tokio::test]
async fn client_graph_list_returns_runs_for_session() {
    let graph = Arc::new(ClientGraphOps::default());
    graph.runs.lock().unwrap().push(client_run("dag-1"));
    let (mut client, _command_rx, _snapshot_tx) =
        client_and_server_with_graph(graph, Arc::new(ClientJobOps::default())).await;

    let runs = client.graph_list("sess-1").await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "dag-1");
    assert_eq!(runs[0].kind, "goal");
}

#[tokio::test]
async fn client_get_node_output_returns_fragment() {
    let jobs = Arc::new(ClientJobOps {
        output: WireNodeOutput {
            output: Some("hello graph".into()),
            ..Default::default()
        },
    });
    let (mut client, _command_rx, _snapshot_tx) =
        client_and_server_with_graph(Arc::new(ClientGraphOps::default()), jobs).await;

    let response = client
        .get_node_output("sess-1", "run-1", "node-1", 6)
        .await
        .unwrap();
    assert_eq!(response.text, "graph");
    assert_eq!(response.offset, 6);
    assert_eq!(response.total, 11);
    assert!(!response.truncated);
}
