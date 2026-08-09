//! Local gRPC server for the coding-agent REPL (`--grpc` mode).
//!
//! The gRPC surface mirrors the `--web` (axum) surface: commands are queued
//! into the same single-turn event loop via [`WebCommand`], and state is served
//! as structured binary protobuf ([`SessionState`]) streamed over server-streaming
//! RPC instead of SSE. Loopback-only, same bind policy as the web UI.

use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::{Stream, StreamExt as _};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, TcpListenerStream};
use tonic::{Request, Response, Status};

use theway::ui::App;
use theway::ui::web_loop::TransportMode;
use theway::wire::{WebCommand, WebPromptImage, WebStatus};

use crate::proto::health::health_check_response::ServingStatus;
use crate::proto::health::health_server::{Health, HealthServer};
use crate::proto::health::{HealthCheckRequest, HealthCheckResponse};
use crate::proto::theway_grpc;
use crate::proto::{dag_event_wire, session_state, stream_event_wire};
use theway_core::runtime::graph_engineering::types::DagEvent;
use theway_core::runtime::subagents::registry::{SubagentEvent, SubagentJobRegistry};

use theway_grpc::theway_grpc_server::{ThewayGrpc, ThewayGrpcServer};
use theway_grpc::{
    ApproveRequest, CommandResult, Empty, GetNodeOutputRequest, GetNodeOutputResponse,
    GraphCancelRequest, GraphCheckpointRequest, GraphCheckpointResponse, GraphKind,
    GraphRestoreRequest, GraphRestoreResponse, GraphRetryRequest, GraphRetryResponse,
    GraphSkipRequest, GraphSkipResponse, MessageMode, SendMessageRequest, SessionState,
    SetModelRequest, StreamFrame,
};

#[derive(Clone, Debug)]
pub struct GrpcOptions {
    pub host: String,
    pub port: u16,
}

#[derive(Clone)]
pub struct GrpcState {
    pub commands: mpsc::UnboundedSender<WebCommand>,
    pub snapshots: broadcast::Sender<WebStatus>,
    pub latest: Arc<Mutex<WebStatus>>,
    /// Event plane (graph mode): subagent started/output/metrics/completed.
    pub events: broadcast::Sender<SubagentEvent>,
    /// Event plane (graph mode): DAG engine node_status / run_status.
    pub dag_events: broadcast::Sender<DagEvent>,
    /// Job registry backing GetNodeOutput.
    pub registry: SubagentJobRegistry,
    /// DAG orchestration engine (graph engineering mode): GraphCancel/Retry/…
    pub dag_engine: Arc<theway_core::runtime::graph_engineering::engine::DagEngine>,
    /// Owning session id: default scope for GraphCheckpoint and the mount key
    /// under which `SessionState.dags` is served.
    pub session_id: String,
}

#[tonic::async_trait]
impl ThewayGrpc for GrpcState {
    type StreamEventsStream = Pin<Box<dyn Stream<Item = Result<StreamFrame, Status>> + Send>>;

    async fn get_state(&self, _request: Request<Empty>) -> Result<Response<SessionState>, Status> {
        let latest = self.latest.lock().await;
        Ok(Response::new(session_state(&latest)))
    }

    async fn stream_events(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        // Merge the snapshot broadcast (low-frequency full state) with the event plane
        // (high-frequency increments) into one typed frame stream.
        let snapshots = BroadcastStream::new(self.snapshots.subscribe()).filter_map(|item| {
            async move {
                match item {
                    Ok(snapshot) => Some(Ok(StreamFrame {
                        payload: Some(theway_grpc::stream_frame::Payload::Snapshot(session_state(
                            &snapshot,
                        ))),
                    })),
                    // Lagged subscribers drop stale frames and catch up on the next
                    // publish; a closed channel ends the stream (broadcast is dropped
                    // when the event loop exits).
                    Err(BroadcastStreamRecvError::Lagged(_)) => None,
                }
            }
        });
        let events = BroadcastStream::new(self.events.subscribe()).filter_map(|item| async move {
            match item {
                Ok(event) => Some(Ok(StreamFrame {
                    payload: Some(theway_grpc::stream_frame::Payload::Event(
                        stream_event_wire(&event),
                    )),
                })),
                Err(BroadcastStreamRecvError::Lagged(_)) => None,
            }
        });
        let dag_events =
            BroadcastStream::new(self.dag_events.subscribe()).filter_map(|item| async move {
                match item {
                    Ok(event) => Some(Ok(StreamFrame {
                        payload: Some(theway_grpc::stream_frame::Payload::Event(dag_event_wire(
                            &event,
                        ))),
                    })),
                    Err(BroadcastStreamRecvError::Lagged(_)) => None,
                }
            });
        // Three sources: snapshot broadcast (low-frequency full state) + subagent
        // events + DAG engine events (both high-frequency increments).
        let stream =
            futures::stream::select(snapshots, futures::stream::select(events, dag_events));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_node_output(
        &self,
        request: Request<GetNodeOutputRequest>,
    ) -> Result<Response<GetNodeOutputResponse>, Status> {
        let request = request.into_inner();
        let Some(job) = self.registry.find_node(&request.run_id, &request.node_id) else {
            return Err(Status::not_found(format!(
                "no job for node {} in run {}",
                request.node_id, request.run_id
            )));
        };
        let offset = request.offset as usize;
        let text = job.output;
        let fragment = if offset < text.len() {
            text[offset..].to_string()
        } else {
            String::new()
        };
        Ok(Response::new(GetNodeOutputResponse {
            text: fragment,
            offset: request.offset,
            total: text.len() as u64,
            truncated: job.truncated,
        }))
    }

    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        let interrupt = request.mode() == MessageMode::Interrupt;
        let accepted = self
            .commands
            .send(WebCommand::Submit {
                text: request.text,
                images: request
                    .images
                    .into_iter()
                    .map(|image| WebPromptImage {
                        data: image.data,
                        name: image.name,
                    })
                    .collect(),
                interrupt,
            })
            .is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn set_model(
        &self,
        request: Request<SetModelRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let accepted = self
            .commands
            .send(WebCommand::SetModel {
                spec: request.into_inner().spec,
            })
            .is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn cancel(&self, _request: Request<Empty>) -> Result<Response<CommandResult>, Status> {
        let accepted = self.commands.send(WebCommand::Abort).is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn approve(
        &self,
        request: Request<ApproveRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let accepted = self
            .commands
            .send(WebCommand::ResolveControlPlane {
                approve: request.into_inner().approve,
            })
            .is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }

    // ── graph orchestration (DAG + goal runs) ────────────────────────────

    async fn graph_cancel(
        &self,
        request: Request<GraphCancelRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let run_id = request.into_inner().run_id;
        self.dag_engine
            .cancel_run(&run_id, Some("cancelled via rpc"));
        Ok(Response::new(CommandResult { accepted: true }))
    }

    async fn graph_retry(
        &self,
        request: Request<GraphRetryRequest>,
    ) -> Result<Response<GraphRetryResponse>, Status> {
        let request = request.into_inner();
        let node_ids = request.node_id.as_deref().map(|id| vec![id.to_string()]);
        let reset = self.dag_engine.retry(&request.run_id, node_ids.as_deref());
        Ok(Response::new(GraphRetryResponse {
            reset_node_ids: reset,
        }))
    }

    async fn graph_skip(
        &self,
        request: Request<GraphSkipRequest>,
    ) -> Result<Response<GraphSkipResponse>, Status> {
        let request = request.into_inner();
        let skipped = self.dag_engine.skip(&request.run_id, &request.node_id);
        Ok(Response::new(GraphSkipResponse { skipped }))
    }

    async fn graph_checkpoint(
        &self,
        request: Request<GraphCheckpointRequest>,
    ) -> Result<Response<GraphCheckpointResponse>, Status> {
        use theway_core::runtime::graph_engineering::persist::to_persisted;
        use theway_core::runtime::graph_engineering::types::RunKind;
        let request = request.into_inner();
        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| self.session_id.clone());

        // Single-run export, or every run owned by the session.
        let runs: Vec<theway_core::runtime::graph_engineering::types::DagRun> = match request.run_id
        {
            Some(run_id) => self
                .dag_engine
                .get_run(&run_id)
                .into_iter()
                .filter(|r| r.session_id.as_deref().is_none_or(|sid| sid == session_id))
                .collect(),
            None => self
                .dag_engine
                .list_runs()
                .into_iter()
                .filter(|r| r.session_id.as_deref().is_none_or(|sid| sid == session_id))
                .collect(),
        };

        let mut checkpoints = Vec::new();
        for run in &runs {
            let persisted = to_persisted(run);
            let snapshot =
                serde_json::to_string(&persisted).map_err(|e| Status::internal(e.to_string()))?;
            checkpoints.push(theway_grpc::GraphSnapshotEntry {
                kind: match run.kind {
                    RunKind::Goal => GraphKind::GraphGoal as i32,
                    _ => GraphKind::GraphDag as i32,
                },
                run_id: run.id.clone(),
                snapshot,
            });
        }
        let error = if runs.is_empty() {
            Some(format!("no runs for session {session_id}"))
        } else {
            None
        };
        Ok(Response::new(GraphCheckpointResponse {
            session_id,
            checkpoints,
            error,
        }))
    }

    async fn graph_restore(
        &self,
        request: Request<GraphRestoreRequest>,
    ) -> Result<Response<GraphRestoreResponse>, Status> {
        let request = request.into_inner();
        let mut persisted: theway_core::runtime::graph_engineering::persist::PersistedRun =
            serde_json::from_str(&request.snapshot)
                .map_err(|e| Status::invalid_argument(format!("invalid snapshot: {e}")))?;
        // Re-attach to the requesting session (snapshots are portable).
        persisted.session_id = Some(request.session_id.clone());
        let ids = self.dag_engine.restore(vec![persisted]);
        let Some(run_id) = ids.first() else {
            return Ok(Response::new(GraphRestoreResponse {
                run_id: String::new(),
                error: Some("restore produced no run".into()),
            }));
        };
        Ok(Response::new(GraphRestoreResponse {
            run_id: run_id.clone(),
            error: None,
        }))
    }
}

/// Standard `grpc.health.v1` service: the server is live as long as the
/// event loop owns it, so every probe answers SERVING regardless of the
/// requested service name.
#[derive(Clone, Default)]
pub struct HealthService;

#[tonic::async_trait]
impl Health for HealthService {
    type WatchStream = Pin<Box<dyn Stream<Item = Result<HealthCheckResponse, Status>> + Send>>;

    async fn check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            status: ServingStatus::Serving as i32,
        }))
    }

    async fn watch(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        // Emit a single SERVING frame, then end the stream.
        let stream = futures::stream::once(async {
            Ok(HealthCheckResponse {
                status: ServingStatus::Serving as i32,
            })
        });
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Full `--grpc` driver: bind, wire the transport channels, spawn the tonic
/// server, then hand the App into the shared event loop.
pub async fn run_grpc(mut app: App, options: GrpcOptions) -> Result<()> {
    let addr = crate::http::bind_addr(&options.host, options.port)?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind grpc ui on {addr}"))?;
    let actual = listener.local_addr()?;

    let endpoints = app.transport_endpoints();
    let grpc_state = GrpcState {
        commands: endpoints.command_tx.clone(),
        snapshots: endpoints.snapshot_tx.clone(),
        latest: endpoints.latest.clone(),
        events: endpoints.events.clone(),
        dag_events: endpoints.dag_events.clone(),
        registry: endpoints.registry.clone(),
        dag_engine: endpoints.dag_engine.clone(),
        session_id: endpoints.session_id.clone(),
    };
    let server_task = serve_grpc(listener, grpc_state);

    println!("theway grpc listening on {actual}");
    println!("  service: theway.grpc.v1.ThewayGrpc + grpc.health.v1.Health · UI: workmate (独立)");

    app.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
}

/// Spawn the tonic server on a bound listener; the handle resolves when the
/// server exits (the event loop selects on it).
pub fn serve_grpc(listener: TcpListener, state: GrpcState) -> tokio::task::JoinHandle<Result<()>> {
    let server = tonic::transport::Server::builder()
        .add_service(ThewayGrpcServer::new(state))
        .add_service(HealthServer::new(HealthService))
        .serve_with_incoming(TcpListenerStream::new(listener));
    tokio::spawn(async move {
        server.await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fixture_snapshot(feed_line: &str) -> WebStatus {
        WebStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: false,
            queued_count: 0,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: crate::http::empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_lines: vec![feed_line.into()],
            dags: Vec::new(),
            subagents: Vec::new(),
        }
    }

    fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<WebCommand>) {
        let (command_tx, command_rx) = mpsc::unbounded_channel::<WebCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WebStatus>(16);
        let latest = Arc::new(Mutex::new(fixture_snapshot("ready")));
        let (event_tx, _) = broadcast::channel::<SubagentEvent>(16);
        let registry = SubagentJobRegistry::new();
        registry.set_event_sender(Some(event_tx.clone()));
        let (dag_event_tx, _) = broadcast::channel::<DagEvent>(16);
        (
            GrpcState {
                commands: command_tx,
                snapshots: snapshot_tx,
                latest,
                events: event_tx,
                dag_events: dag_event_tx,
                registry,
                dag_engine: Arc::new(
                    theway_core::runtime::graph_engineering::engine::DagEngine::new(),
                ),
                session_id: "test-session".into(),
            },
            command_rx,
        )
    }

    #[tokio::test]
    async fn get_state_returns_structured_session_state() {
        let (state, _command_rx) = grpc_state();
        let state = state
            .get_state(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(state.session_id, "sess-1");
        assert_eq!(state.cwd, "/tmp/theway");
        assert_eq!(state.feed_lines, vec!["ready"]);
    }

    #[tokio::test]
    async fn commands_queue_with_accepted_semantics() {
        let (state, mut command_rx) = grpc_state();

        let result = state
            .send_message(Request::new(SendMessageRequest {
                text: "hello".into(),
                images: vec![theway_grpc::Image {
                    data: "data".into(),
                    name: Some("clip.png".into()),
                }],
                mode: MessageMode::Guide.into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(result.accepted);
        match command_rx.recv().await.unwrap() {
            WebCommand::Submit {
                text,
                images,
                interrupt: _,
            } => {
                assert_eq!(text, "hello");
                assert_eq!(images.len(), 1);
                assert_eq!(images[0].data, "data");
                assert_eq!(images[0].name.as_deref(), Some("clip.png"));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let result = state
            .cancel(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        assert!(result.accepted);
        assert!(matches!(
            command_rx.recv().await.unwrap(),
            WebCommand::Abort
        ));

        let result = state
            .set_model(Request::new(SetModelRequest {
                spec: "anthropic:claude-haiku-4-5".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(result.accepted);
        match command_rx.recv().await.unwrap() {
            WebCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-haiku-4-5"),
            other => panic!("unexpected command: {other:?}"),
        }

        let result = state
            .approve(Request::new(ApproveRequest { approve: true }))
            .await
            .unwrap()
            .into_inner();
        assert!(result.accepted);
        match command_rx.recv().await.unwrap() {
            WebCommand::ResolveControlPlane { approve } => assert!(approve),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stream_events_emits_published_snapshots() {
        let (state, _command_rx) = grpc_state();
        let response = state
            .stream_events(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        tokio::pin!(response);

        state.snapshots.send(fixture_snapshot("streamed")).unwrap();
        let item = tokio::time::timeout(Duration::from_secs(2), response.next())
            .await
            .expect("timed out")
            .expect("stream ended");
        let frame = item.unwrap();
        match frame.payload {
            Some(theway_grpc::stream_frame::Payload::Snapshot(state)) => {
                assert_eq!(state.feed_lines, vec!["streamed"]);
            }
            other => panic!("expected snapshot payload, got {other:?}"),
        }

        // Stream ends once all three broadcast senders are dropped (merged stream).
        drop(state.snapshots);
        state.registry.set_event_sender(None);
        drop(state.events);
        drop(state.dag_events);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), response.next())
                .await
                .expect("timed out")
                .is_none(),
            "stream should end after broadcast close"
        );
    }

    #[tokio::test]
    async fn get_node_output_returns_fragment_from_offset() {
        let (state, _command_rx) = grpc_state();
        let job_id = state
            .registry
            .register(theway_core::runtime::subagents::registry::JobInit {
                agent: "explorer".into(),
                source: "dag".into(),
                run_id: Some("run-1".into()),
                node_id: Some("node-1".into()),
            });
        state.registry.update(&job_id, |job| {
            job.output = "hello graph".into();
        });

        let response = state
            .get_node_output(Request::new(GetNodeOutputRequest {
                run_id: "run-1".into(),
                node_id: "node-1".into(),
                offset: 6,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.text, "graph");
        assert_eq!(response.offset, 6);
        assert_eq!(response.total, 11);
        assert!(!response.truncated);

        // Unknown node → not found.
        let err = state
            .get_node_output(Request::new(GetNodeOutputRequest {
                run_id: "run-1".into(),
                node_id: "nope".into(),
                offset: 0,
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);

        // Offset past the end → empty fragment, total preserved.
        let response = state
            .get_node_output(Request::new(GetNodeOutputRequest {
                run_id: "run-1".into(),
                node_id: "node-1".into(),
                offset: 100,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.text, "");
        assert_eq!(response.total, 11);
    }

    #[tokio::test]
    async fn stream_events_merges_snapshot_and_event_payloads() {
        let (state, _command_rx) = grpc_state();
        let response = state
            .stream_events(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        tokio::pin!(response);

        state.snapshots.send(fixture_snapshot("snap")).unwrap();
        state
            .events
            .send(SubagentEvent::Output {
                id: "job-1".into(),
                chunk: "hi".into(),
            })
            .unwrap();
        state
            .dag_events
            .send(DagEvent::RunStatus {
                run_id: "goal-1".into(),
                status: theway_core::runtime::graph_engineering::types::DagStatus::Running,
                error: None,
            })
            .unwrap();

        let mut kinds = Vec::new();
        for _ in 0..3 {
            let item = tokio::time::timeout(Duration::from_secs(2), response.next())
                .await
                .expect("timed out")
                .expect("stream ended");
            let frame = item.unwrap();
            match frame.payload {
                Some(theway_grpc::stream_frame::Payload::Snapshot(_)) => kinds.push("snapshot"),
                Some(theway_grpc::stream_frame::Payload::Event(event)) => match event.kind {
                    Some(theway_grpc::stream_event::Kind::SubagentOutput(o)) => {
                        assert_eq!(o.chunk, "hi");
                        kinds.push("subagent");
                    }
                    Some(theway_grpc::stream_event::Kind::RunStatus(run)) => {
                        assert_eq!(run.run_id, "goal-1");
                        assert_eq!(run.status, "running");
                        kinds.push("dag");
                    }
                    other => panic!("unexpected event: {other:?}"),
                },
                None => panic!("empty frame"),
            }
        }
        kinds.sort();
        assert_eq!(kinds, ["dag", "snapshot", "subagent"]);
    }

    #[tokio::test]
    async fn stream_events_forwards_dag_node_status_frames() {
        let (state, _command_rx) = grpc_state();
        let response = state
            .stream_events(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner();
        tokio::pin!(response);

        state
            .dag_events
            .send(DagEvent::NodeStatus {
                run_id: "goal-1".into(),
                node_id: "main".into(),
                status: theway_core::runtime::graph_engineering::types::NodeStatus::Failed,
                error: Some("condition broken".into()),
            })
            .unwrap();
        let item = tokio::time::timeout(Duration::from_secs(2), response.next())
            .await
            .expect("timed out")
            .expect("stream ended");
        let frame = item.unwrap();
        match frame.payload {
            Some(theway_grpc::stream_frame::Payload::Event(event)) => match event.kind {
                Some(theway_grpc::stream_event::Kind::NodeStatus(node)) => {
                    assert_eq!(node.run_id, "goal-1");
                    assert_eq!(node.node_id, "main");
                    assert_eq!(node.status, "failed");
                    assert_eq!(node.error.as_deref(), Some("condition broken"));
                }
                other => panic!("expected NodeStatus, got {other:?}"),
            },
            other => panic!("expected event payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn grpc_server_over_transport_serves_client() {
        let (state, mut command_rx) = grpc_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = serve_grpc(listener, state);

        let mut client =
            theway_grpc::theway_grpc_client::ThewayGrpcClient::connect(format!("http://{addr}"))
                .await
                .unwrap();

        let state = client.get_state(Empty {}).await.unwrap().into_inner();
        assert_eq!(state.session_id, "sess-1");

        let result = client
            .send_message(SendMessageRequest {
                text: "via transport".into(),
                images: Vec::new(),
                mode: MessageMode::Guide.into(),
            })
            .await
            .unwrap()
            .into_inner();
        assert!(result.accepted);
        match command_rx.recv().await.unwrap() {
            WebCommand::Submit { text, .. } => assert_eq!(text, "via transport"),
            other => panic!("unexpected command: {other:?}"),
        }

        server.abort();
    }

    #[tokio::test]
    async fn health_service_serves_serving_over_transport() {
        let (state, _command_rx) = grpc_state();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = serve_grpc(listener, state);

        let mut client =
            crate::proto::health::health_client::HealthClient::connect(format!("http://{addr}"))
                .await
                .unwrap();

        // Check answers SERVING.
        let response = client
            .check(crate::proto::health::HealthCheckRequest {
                service: String::new(),
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.status, ServingStatus::Serving as i32);

        // Watch emits one SERVING frame, then ends.
        let mut watch = client
            .watch(crate::proto::health::HealthCheckRequest {
                service: String::new(),
            })
            .await
            .unwrap()
            .into_inner();
        let first = watch.message().await.unwrap().expect("first frame");
        assert_eq!(first.status, ServingStatus::Serving as i32);
        assert!(
            watch.message().await.unwrap().is_none(),
            "watch stream should end after the single SERVING frame"
        );

        server.abort();
    }
}
