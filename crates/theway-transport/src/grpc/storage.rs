//! `StorageService` gRPC implementation (issue #84): the runtime-state
//! externalization RPC contract. Session methods mirror the existing
//! `SessionService` surface so external storage can drive the same resource
//! lifecycle; DAG run / trigger-rule / cron-job methods forward to the
//! [`crate::transport::StorageOps`] handler seam via the [`crate::state`]
//! codecs.

use std::sync::Arc;

use anyhow::Result;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};

use super::{GrpcState, HealthServer};
use crate::proto::theway_grpc::storage_service_server::{StorageService, StorageServiceServer};
use crate::proto::theway_grpc::{
    CommandResult, CreateSessionRequest, CreateSessionResponse, DeleteSessionRequest,
    DeleteSessionResponse, Empty, ListSessionsResponse, LoadCronJobsRequest, LoadCronJobsResponse,
    LoadDagRunsRequest, LoadDagRunsResponse, LoadTriggerRulesRequest, LoadTriggerRulesResponse,
    RenameSessionRequest, SaveCronJobsRequest, SaveCronJobsResponse, SaveDagRunRequest,
    SaveDagRunResponse, SaveTriggerRulesRequest, SaveTriggerRulesResponse,
    UpdateSessionMetadataRequest,
};
use crate::state as codec;
use crate::transport::{SessionOps, StorageOps};

/// Minimal `StorageService` server state (issue #85): only the session and
/// storage handler seams — no daemon command/snapshot channels. Clients or
/// controllers that serve runtime storage back to a daemon (the TUI's
/// controller-side SQLite/filesystem) construct one directly and hand it to
/// [`serve_storage_service`].
#[derive(Clone)]
pub struct StorageServiceState {
    pub session_ops: Arc<dyn SessionOps>,
    pub storage_ops: Arc<dyn StorageOps>,
}

impl StorageServiceState {
    pub fn new(session_ops: Arc<dyn SessionOps>, storage_ops: Arc<dyn StorageOps>) -> Self {
        Self {
            session_ops,
            storage_ops,
        }
    }
}

/// Spawn a storage-only tonic server on a bound listener (issue #85): the
/// `StorageService` surface plus the standard health service — nothing from
/// the daemon channel stack. The handle resolves when the server exits.
pub fn serve_storage_service(
    listener: TcpListener,
    state: StorageServiceState,
) -> tokio::task::JoinHandle<Result<()>> {
    let server = tonic::transport::Server::builder()
        .add_service(StorageServiceServer::new(state))
        .add_service(HealthServer::new(super::HealthService))
        .serve_with_incoming(TcpListenerStream::new(listener));
    tokio::spawn(async move {
        server.await?;
        Ok(())
    })
}

#[tonic::async_trait]
impl StorageService for StorageServiceState {
    async fn list_sessions(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(ListSessionsResponse {
            sessions: sessions
                .iter()
                .map(crate::proto::session_summary_wire)
                .collect(),
            current_session_id: String::new(),
        }))
    }

    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let request = request.into_inner();
        let new_id = self
            .session_ops
            .create(request.session_id.as_deref(), &request.metadata)
            .await
            .map_err(|e| {
                if e.to_string().contains("already exists") {
                    Status::already_exists(e.to_string())
                } else {
                    Status::internal(e.to_string())
                }
            })?;
        if let Some(name) = request.name.as_deref()
            && !name.trim().is_empty()
        {
            self.session_ops
                .rename(&new_id, name)
                .await
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
        }
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let session = sessions
            .iter()
            .find(|s| s.session_id == new_id)
            .map(crate::proto::session_summary_wire);
        Ok(Response::new(CreateSessionResponse { session }))
    }

    async fn update_session_metadata(
        &self,
        request: Request<UpdateSessionMetadataRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        self.session_ops
            .update_metadata(&request.session_id, &request.metadata)
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
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let full_id = crate::proto::resolve_session_id(&sessions, &requested)
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
        Ok(Response::new(DeleteSessionResponse {
            running_run_ids: Vec::new(),
        }))
    }

    // ── DAG run persistence ───────────────────────────────────────────

    async fn save_dag_run(
        &self,
        request: Request<SaveDagRunRequest>,
    ) -> Result<Response<SaveDagRunResponse>, Status> {
        let request = codec::save_dag_run_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .save_dag_run(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::save_dag_run_response_to_proto(
            &result,
        )))
    }

    async fn load_dag_runs(
        &self,
        request: Request<LoadDagRunsRequest>,
    ) -> Result<Response<LoadDagRunsResponse>, Status> {
        let request = codec::load_dag_runs_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .load_dag_runs(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::load_dag_runs_response_to_proto(
            &result,
        )))
    }

    // ── trigger rules ─────────────────────────────────────────────────

    async fn save_trigger_rules(
        &self,
        request: Request<SaveTriggerRulesRequest>,
    ) -> Result<Response<SaveTriggerRulesResponse>, Status> {
        let request = codec::save_trigger_rules_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .save_trigger_rules(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::save_trigger_rules_response_to_proto(
            &result,
        )))
    }

    async fn load_trigger_rules(
        &self,
        request: Request<LoadTriggerRulesRequest>,
    ) -> Result<Response<LoadTriggerRulesResponse>, Status> {
        let request = codec::load_trigger_rules_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .load_trigger_rules(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::load_trigger_rules_response_to_proto(
            &result,
        )))
    }

    // ── cron jobs ─────────────────────────────────────────────────────

    async fn save_cron_jobs(
        &self,
        request: Request<SaveCronJobsRequest>,
    ) -> Result<Response<SaveCronJobsResponse>, Status> {
        let request = codec::save_cron_jobs_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .save_cron_jobs(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::save_cron_jobs_response_to_proto(
            &result,
        )))
    }

    async fn load_cron_jobs(
        &self,
        request: Request<LoadCronJobsRequest>,
    ) -> Result<Response<LoadCronJobsResponse>, Status> {
        let request = codec::load_cron_jobs_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .load_cron_jobs(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::load_cron_jobs_response_to_proto(
            &result,
        )))
    }
}

#[tonic::async_trait]
impl StorageService for GrpcState {
    // ── session resources (mirror SessionService) ─────────────────────

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
            sessions: sessions
                .iter()
                .map(crate::proto::session_summary_wire)
                .collect(),
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
            .create(request.session_id.as_deref(), &request.metadata)
            .await
            .map_err(|e| {
                if e.to_string().contains("already exists") {
                    Status::already_exists(e.to_string())
                } else {
                    Status::internal(e.to_string())
                }
            })?;
        if let Some(name) = request.name.as_deref()
            && !name.trim().is_empty()
        {
            self.session_ops
                .rename(&new_id, name)
                .await
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
        }
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let session = sessions
            .iter()
            .find(|s| s.session_id == new_id)
            .map(crate::proto::session_summary_wire);
        Ok(Response::new(CreateSessionResponse { session }))
    }

    async fn update_session_metadata(
        &self,
        request: Request<UpdateSessionMetadataRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = request.into_inner();
        self.session_ops
            .update_metadata(&request.session_id, &request.metadata)
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
        let sessions = self
            .session_ops
            .list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let full_id = crate::proto::resolve_session_id(&sessions, &requested)
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
        }
        Ok(Response::new(DeleteSessionResponse {
            running_run_ids: Vec::new(),
        }))
    }

    // ── DAG run persistence ───────────────────────────────────────────

    async fn save_dag_run(
        &self,
        request: Request<SaveDagRunRequest>,
    ) -> Result<Response<SaveDagRunResponse>, Status> {
        let request = codec::save_dag_run_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .save_dag_run(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::save_dag_run_response_to_proto(
            &result,
        )))
    }

    async fn load_dag_runs(
        &self,
        request: Request<LoadDagRunsRequest>,
    ) -> Result<Response<LoadDagRunsResponse>, Status> {
        let request = codec::load_dag_runs_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .load_dag_runs(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::load_dag_runs_response_to_proto(
            &result,
        )))
    }

    // ── trigger rules ─────────────────────────────────────────────────

    async fn save_trigger_rules(
        &self,
        request: Request<SaveTriggerRulesRequest>,
    ) -> Result<Response<SaveTriggerRulesResponse>, Status> {
        let request = codec::save_trigger_rules_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .save_trigger_rules(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::save_trigger_rules_response_to_proto(
            &result,
        )))
    }

    async fn load_trigger_rules(
        &self,
        request: Request<LoadTriggerRulesRequest>,
    ) -> Result<Response<LoadTriggerRulesResponse>, Status> {
        let request = codec::load_trigger_rules_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .load_trigger_rules(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::load_trigger_rules_response_to_proto(
            &result,
        )))
    }

    // ── cron jobs ─────────────────────────────────────────────────────

    async fn save_cron_jobs(
        &self,
        request: Request<SaveCronJobsRequest>,
    ) -> Result<Response<SaveCronJobsResponse>, Status> {
        let request = codec::save_cron_jobs_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .save_cron_jobs(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::save_cron_jobs_response_to_proto(
            &result,
        )))
    }

    async fn load_cron_jobs(
        &self,
        request: Request<LoadCronJobsRequest>,
    ) -> Result<Response<LoadCronJobsResponse>, Status> {
        let request = codec::load_cron_jobs_request_from_proto(&request.into_inner());
        let result = self
            .storage_ops
            .load_cron_jobs(&request)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(codec::load_cron_jobs_response_to_proto(
            &result,
        )))
    }
}
