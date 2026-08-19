//! Local gRPC server for the coding-agent REPL (`--grpc` mode).
//!
//! The gRPC surface mirrors the `--http` (axum) surface: commands are queued
//! into the same single-turn event loop via [`WireCommand`], and state is served
//! as structured binary protobuf ([`SessionState`]) streamed over server-streaming
//! RPC instead of SSE. Loopback-only, same bind policy as the web UI.
//!
//! Domain split: the `ToolService` implementation (issue #75) lives in
//! [`tools`](crate::grpc::tools).

mod storage;
mod tools;

pub use storage::{StorageServiceState, serve_storage_service};
pub use tools::{ToolServiceState, serve_tool_service};

use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use futures::{Stream, StreamExt as _};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, TcpListenerStream};
use tonic::{Request, Response, Status};

use crate::host::TransportHost;
use crate::transport::SessionOps;
use crate::transport::ToolOps;
use crate::transport::TransportMode;
use crate::wire::{WireCommand, WireDaemonConfig, WirePathContext, WirePromptImage, WireStatus};

use crate::proto::health::health_check_response::ServingStatus;
use crate::proto::health::health_server::{Health, HealthServer};
use crate::proto::health::{HealthCheckRequest, HealthCheckResponse};
use crate::proto::theway_grpc;
use crate::proto::{
    dag_event_wire, dag_run_wire, resolve_session_id, session_state, session_summary_wire,
    stream_event_wire,
};
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};

use theway_grpc::DaemonConfig;
use theway_grpc::command_service_server::{CommandService, CommandServiceServer};
use theway_grpc::event_service_server::{EventService, EventServiceServer};
use theway_grpc::graph_engine_service_server::{GraphEngineService, GraphEngineServiceServer};
use theway_grpc::session_service_server::{SessionService, SessionServiceServer};
use theway_grpc::settings_service_server::{SettingsService, SettingsServiceServer};
use theway_grpc::storage_service_server::StorageServiceServer;
use theway_grpc::tool_service_server::ToolServiceServer;
use theway_grpc::{
    ApproveRequest, CommandResult, CreateSessionRequest, CreateSessionResponse,
    DeleteSessionRequest, DeleteSessionResponse, Empty, GetNodeOutputRequest,
    GetNodeOutputResponse, GraphCancelRequest, GraphCheckpointRequest, GraphCheckpointResponse,
    GraphKind, GraphListRequest, GraphListResponse, GraphNodeInterruptRequest,
    GraphNodeSteerRequest, GraphRestoreRequest, GraphRestoreResponse, GraphRetryRequest,
    GraphRetryResponse, GraphSkipRequest, GraphSkipResponse, ListSessionsResponse, MessageMode,
    RenameSessionRequest, SendMessageRequest, SessionState, SetModelRequest, StreamFrame,
    SwitchSessionRequest,
};

#[derive(Clone)]
pub struct GrpcOptions {
    pub host: String,
    pub port: u16,
    /// Called with the actual bound address after the listener is up (used to
    /// publish the port when `port: 0` requested a random one).
    pub on_listen: Option<std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync>>,
}

#[derive(Clone)]
pub struct GrpcState {
    pub commands: mpsc::UnboundedSender<WireCommand>,
    pub snapshots: broadcast::Sender<WireStatus>,
    pub latest: Arc<Mutex<WireStatus>>,
    /// Event plane (graph mode): subagent started/output/metrics/completed.
    pub events: broadcast::Sender<AgentJobEvent>,
    /// Event plane (graph mode): DAG engine node_status / run_status.
    pub dag_events: broadcast::Sender<DagEvent>,
    /// Job registry backing GetNodeOutput.
    pub registry: AgentJobRegistry,
    /// DAG orchestration engine (graph engineering mode): GraphCancel/Retry/…
    pub dag_engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    /// session-resource-model: session lifecycle ops (list/create/rename/delete).
    /// Switching the *current* session goes through `WireCommand::SwitchSession`.
    pub session_ops: Arc<dyn SessionOps>,
    /// Abort handle for the registry→events forwarder task spawned at startup.
    pub agent_fwd: tokio::task::AbortHandle,
    /// Owning session id: default scope for GraphCheckpoint and the mount key
    /// under which `SessionState.dags` is served. Mutable: SwitchSession (and
    /// the DeleteSession fallback) rebind it; the event loop re-syncs it via
    /// snapshots.
    pub session_id: Arc<std::sync::RwLock<String>>,
    /// Shared daemon path context (issue #68): served by `GetPathContext`;
    /// `SetSkillDirs` optimistically updates `skills_dirs` before the event
    /// loop applies the change authoritatively.
    pub path_context: Arc<std::sync::RwLock<WirePathContext>>,
    /// Shared daemon configuration view (issue #72): served by `GetConfig`;
    /// `SetConfig` / `Configure` optimistically merge the patch before the
    /// event loop applies it authoritatively.
    pub daemon_config: Arc<std::sync::RwLock<WireDaemonConfig>>,
    /// File/tool operation handler (issue #75): backs the `ToolService`
    /// surface (`ReadFile` / … / `SkillInstall`). The daemon kernel
    /// implements the seam against its execution environment.
    pub tool_ops: Arc<dyn ToolOps>,
    /// Runtime state storage handler (issue #84): backs the `StorageService`
    /// surface (`SaveDagRun` / `LoadDagRuns` / trigger/cron persistence). The
    /// daemon kernel implements the seam against the `RuntimeStorage` adapter.
    pub storage_ops: Arc<dyn crate::transport::StorageOps>,
}

#[tonic::async_trait]
impl CommandService for GrpcState {
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        // Explicit session targeting: only the current (live) session can receive
        // messages — the process runs a single agent loop. Other sessions must be
        // switched to first (connection-level binding is client-side state).
        if let Some(target) = request.session_id.as_deref() {
            let current = self.session_id.read().unwrap().clone();
            if target != current {
                return Err(Status::failed_precondition(format!(
                    "session {target} is not the active session ({current}); SwitchSession first"
                )));
            }
        }
        let interrupt = request.mode() == MessageMode::Interrupt;
        let accepted = self
            .commands
            .send(WireCommand::Submit {
                text: request.text,
                images: request
                    .images
                    .into_iter()
                    .map(|image| WirePromptImage {
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
            .send(WireCommand::SetModel {
                spec: request.into_inner().spec,
            })
            .is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn cancel(&self, _request: Request<Empty>) -> Result<Response<CommandResult>, Status> {
        let accepted = self.commands.send(WireCommand::Abort).is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn approve(
        &self,
        request: Request<ApproveRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let accepted = self
            .commands
            .send(WireCommand::ResolveControlPlane {
                approve: request.into_inner().approve,
            })
            .is_ok();
        Ok(Response::new(CommandResult { accepted }))
    }
}

#[tonic::async_trait]
impl SessionService for GrpcState {
    async fn get_state(&self, _request: Request<Empty>) -> Result<Response<SessionState>, Status> {
        let latest = self.latest.lock();
        Ok(Response::new(session_state(&latest)))
    }

    // ── session resources (session-resource-model; backed by SessionOps) ──

    async fn list_sessions(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let current_session_id = self.session_id.read().unwrap().clone();
        Ok(Response::new(ListSessionsResponse {
            sessions: sessions.iter().map(session_summary_wire).collect(),
            current_session_id,
        }))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let request = request.into_inner();
        let new_id = self
            .session_ops
            .create()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        if let Some(name) = request.name.as_deref()
            && !name.trim().is_empty()
        {
            self.session_ops
                .rename(&new_id, name)
                .await
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
        }
        // Becoming current goes through the serialized event loop; the current
        // marker in ListSessions follows on the next snapshot.
        let accepted = self
            .commands
            .send(WireCommand::SwitchSession { id: new_id.clone() })
            .is_ok();
        if !accepted {
            return Err(Status::unavailable("event loop command channel closed"));
        }
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let session = sessions
            .iter()
            .find(|s| s.session_id == new_id)
            .map(session_summary_wire);
        Ok(Response::new(CreateSessionResponse { session }))
    }

    async fn switch_session(
        &self,
        request: Request<SwitchSessionRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let requested = request.into_inner().session_id;
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let target = resolve_session_id(&sessions, &requested)
            .ok_or_else(|| Status::not_found(format!("no session matches id {requested}")))?;
        let accepted = self
            .commands
            .send(WireCommand::SwitchSession { id: target.clone() })
            .is_ok();
        if accepted {
            // Rebind the connection-level current session; the event loop applies
            // the same change on the serialized loop and re-publishes snapshots.
            *self.session_id.write().unwrap() = target.clone();
            self.latest.lock().session_id = target;
        }
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn rename_session(
        &self,
        request: Request<RenameSessionRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        self.session_ops
            .rename(&request.session_id, &request.name)
            .await
            .map_err(|e| {
                if e.to_string().contains("no session matches") {
                    Status::not_found(e.to_string())
                } else {
                    Status::invalid_argument(e.to_string())
                }
            })?;
        Ok(Response::new(CommandResult { accepted: true }))
    }

    async fn delete_session(
        &self,
        request: Request<DeleteSessionRequest>,
    ) -> Result<Response<DeleteSessionResponse>, Status> {
        let requested = request.into_inner().session_id;
        // Resolve to the full id first: delete protection and the current-session
        // fallback compare against the metadata id.
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let full_id = resolve_session_id(&sessions, &requested)
            .ok_or_else(|| Status::not_found(format!("no session matches id {requested}")))?;
        let running = self
            .session_ops
            .delete(&full_id)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        if !running.is_empty() {
            return Err(Status::failed_precondition(format!(
                "session {full_id} still has running graphs: {}; cancel them (GraphCancel) before deleting",
                running.join(", ")
            )));
        }
        // Deleted the current session → fall back to the most recent remaining
        // session (or empty) and tell the event loop to switch to it.
        if self.session_id.read().unwrap().clone() == full_id {
            let remaining = self
                .session_ops
                .list()
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
            let fallback = remaining
                .last()
                .map(|s| s.session_id.clone())
                .unwrap_or_default();
            *self.session_id.write().unwrap() = fallback.clone();
            self.latest.lock().session_id = fallback.clone();
            if !fallback.is_empty() {
                let _ = self
                    .commands
                    .send(WireCommand::SwitchSession { id: fallback });
            }
        }
        Ok(Response::new(DeleteSessionResponse {
            running_run_ids: Vec::new(),
        }))
    }

    // ── path context (issue #68) ───────────────────────────────────────

    async fn get_path_context(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<theway_grpc::PathContext>, Status> {
        let ctx = self.path_context.read().unwrap();
        Ok(Response::new(crate::proto::wire_path_context_to_proto(
            &ctx,
        )))
    }

    async fn set_skill_dirs(
        &self,
        request: Request<theway_grpc::SetSkillDirsRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let dirs = request.into_inner().dirs;
        // Optimistic update: readers (GetPathContext) see the new dirs right
        // away; the event loop applies the same command authoritatively
        // (skills hot-reload) and re-publishes snapshots.
        self.path_context.write().unwrap().skills_dirs = dirs.clone();
        self.commands
            .send(WireCommand::SetSkillDirs { dirs })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        Ok(Response::new(CommandResult { accepted: true }))
    }
}

// ── settings / config (issue #72) ─────────────────────────────────────

#[tonic::async_trait]
impl SettingsService for GrpcState {
    async fn get_config(&self, _request: Request<Empty>) -> Result<Response<DaemonConfig>, Status> {
        let config = self.daemon_config.read().unwrap();
        Ok(Response::new(crate::proto::daemon_config_to_proto(&config)))
    }

    async fn set_config(
        &self,
        request: Request<DaemonConfig>,
    ) -> Result<Response<CommandResult>, Status> {
        let config = crate::proto::daemon_config_from_proto(&request.into_inner());
        Ok(Response::new(CommandResult {
            accepted: self.enqueue_configure(config)?,
        }))
    }

    async fn configure(
        &self,
        request: Request<DaemonConfig>,
    ) -> Result<Response<CommandResult>, Status> {
        let config = crate::proto::daemon_config_from_proto(&request.into_inner());
        Ok(Response::new(CommandResult {
            accepted: self.enqueue_configure(config)?,
        }))
    }
}

impl GrpcState {
    /// Shared SetConfig / Configure body (issue #72): optimistically merge the
    /// patch into the shared config view — `GetConfig` readers observe it right
    /// away — then enqueue `WireCommand::Configure` so the serialized event
    /// loop applies the same patch authoritatively.
    fn enqueue_configure(&self, config: WireDaemonConfig) -> Result<bool, Status> {
        self.daemon_config.write().unwrap().merge_from(&config);
        self.commands
            .send(WireCommand::Configure { config })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        Ok(true)
    }
}

#[tonic::async_trait]
impl GraphEngineService for GrpcState {
    async fn get_node_output(
        &self,
        request: Request<GetNodeOutputRequest>,
    ) -> Result<Response<GetNodeOutputResponse>, Status> {
        let request = request.into_inner();
        // Transcript first (memory, then disk — a finished node's messages
        // survive a process restart via the per-node file), text tail second.
        let messages = self
            .registry
            .node_messages(&request.run_id, &request.node_id);
        let messages_json = messages
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default())
            .unwrap_or_default();
        let messages_truncated = self
            .registry
            .find_node(&request.run_id, &request.node_id)
            .map(|job| job.messages_truncated)
            .unwrap_or(false);
        let Some(job) = self.registry.find_node(&request.run_id, &request.node_id) else {
            // Recovery path: job is gone (restart) but a disk transcript may
            // still exist — serve it instead of 404-ing.
            if !messages_json.is_empty() {
                return Ok(Response::new(GetNodeOutputResponse {
                    text: String::new(),
                    offset: request.offset,
                    total: 0,
                    truncated: false,
                    messages_json: Some(messages_json),
                    messages_truncated,
                }));
            }
            return Err(Status::not_found(format!(
                "no job for node {} in run {}",
                request.node_id, request.run_id
            )));
        };
        let text = job.output;
        let (offset, fragment) = crate::text_cursor::slice_from(&text, request.offset);
        Ok(Response::new(GetNodeOutputResponse {
            text: fragment.to_string(),
            offset,
            total: text.len() as u64,
            truncated: job.truncated,
            messages_json: (!messages_json.is_empty()).then_some(messages_json),
            messages_truncated,
        }))
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

    async fn graph_node_interrupt(
        &self,
        request: Request<GraphNodeInterruptRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        let accepted = self
            .registry
            .interrupt_node(&request.run_id, &request.node_id);
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn graph_node_steer(
        &self,
        request: Request<GraphNodeSteerRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        let accepted = self
            .registry
            .steer_node(&request.run_id, &request.node_id, request.text);
        Ok(Response::new(CommandResult { accepted }))
    }

    async fn graph_checkpoint(
        &self,
        request: Request<GraphCheckpointRequest>,
    ) -> Result<Response<GraphCheckpointResponse>, Status> {
        use theway_core::multiagent::graph::persist::to_persisted;
        use theway_core::multiagent::graph::types::RunKind;
        let request = request.into_inner();
        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| self.session_id.read().unwrap().clone());

        // Single-run export, or every run owned by the session.
        let runs: Vec<theway_core::multiagent::graph::types::DagRun> = match request.run_id {
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
        let mut persisted: theway_core::multiagent::graph::persist::PersistedRun =
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

    async fn graph_list(
        &self,
        request: Request<GraphListRequest>,
    ) -> Result<Response<GraphListResponse>, Status> {
        let session_id = request.into_inner().session_id;
        let runs = self
            .dag_engine
            .list_runs()
            .into_iter()
            .filter(|run| run.session_id.as_deref() == Some(session_id.as_str()))
            .map(|run| dag_run_wire(&WireStatus::from_dag_run(&run)))
            .collect();
        Ok(Response::new(GraphListResponse { runs }))
    }
}

#[tonic::async_trait]
impl EventService for GrpcState {
    type StreamEventsStream = Pin<Box<dyn Stream<Item = Result<StreamFrame, Status>> + Send>>;
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
        // Continuous health stream: emit SERVING every 5 seconds. gRPC load
        // balancers, grpc_health_probe, and k8s probes expect Watch to stay
        // open and periodically re-emit the serving status; a single-frame
        // stream would mark the endpoint as transient/dead after the first
        // frame completes.
        let interval = tokio::time::interval(std::time::Duration::from_secs(5));
        let stream = tokio_stream::wrappers::IntervalStream::new(interval).map(|_| {
            Ok(HealthCheckResponse {
                status: ServingStatus::Serving as i32,
            })
        });
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Full `--grpc` driver: bind, wire the transport channels, spawn the tonic
/// server, then hand the App into the shared event loop.
pub async fn run_grpc(mut app: Box<dyn TransportHost>, options: GrpcOptions) -> Result<()> {
    let addr = crate::http::bind_addr(&options.host, options.port)?;
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind grpc ui on {addr}"))?;
    let actual = listener.local_addr()?;
    if let Some(on_listen) = &options.on_listen {
        on_listen(actual);
    }

    let endpoints = app.transport_endpoints();
    let agent_fwd = endpoints.agent_fwd.clone();
    let grpc_state = GrpcState {
        commands: endpoints.command_tx.clone(),
        snapshots: endpoints.snapshot_tx.clone(),
        latest: endpoints.latest.clone(),
        events: endpoints.events.clone(),
        dag_events: endpoints.dag_events.clone(),
        registry: endpoints.registry.clone(),
        dag_engine: endpoints.dag_engine.clone(),
        session_ops: endpoints.session_ops.clone(),
        agent_fwd,
        session_id: Arc::new(std::sync::RwLock::new(endpoints.session_id.clone())),
        path_context: endpoints.path_context.clone(),
        daemon_config: endpoints.daemon_config.clone(),
        tool_ops: endpoints.tool_ops.clone(),
        storage_ops: endpoints.storage_ops.clone(),
    };
    let server_task = serve_grpc(listener, grpc_state);

    println!("theway grpc listening on {actual}");
    println!(
        "  services: theway.grpc.v1.CommandService / theway.grpc.v1.SessionService / theway.grpc.v1.SettingsService / theway.grpc.v1.StorageService / theway.grpc.v1.ToolService / theway.grpc.v1.GraphEngineService / theway.grpc.v1.EventService + grpc.health.v1.Health · UI: workmate (独立)"
    );

    app.run_transport_loop(TransportMode::Grpc, endpoints, server_task)
        .await
}

/// Spawn the tonic server on a bound listener; the handle resolves when the
/// server exits (the event loop selects on it).
pub fn serve_grpc(listener: TcpListener, state: GrpcState) -> tokio::task::JoinHandle<Result<()>> {
    let server = tonic::transport::Server::builder()
        .add_service(CommandServiceServer::new(state.clone()))
        .add_service(SessionServiceServer::new(state.clone()))
        .add_service(SettingsServiceServer::new(state.clone()))
        .add_service(StorageServiceServer::new(state.clone()))
        .add_service(ToolServiceServer::new(ToolServiceState::new(
            state.tool_ops.clone(),
        )))
        .add_service(GraphEngineServiceServer::new(state.clone()))
        .add_service(EventServiceServer::new(state))
        .add_service(HealthServer::new(HealthService))
        .serve_with_incoming(TcpListenerStream::new(listener));
    tokio::spawn(async move {
        server.await?;
        Ok(())
    })
}

#[cfg(test)]
// Test files live in `tests/grpc/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("grpc");
