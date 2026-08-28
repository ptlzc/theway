use super::*;

#[tonic::async_trait]
impl SessionService for GrpcState {
    async fn get_state(
        &self,
        request: Request<theway_grpc::SessionStateRequest>,
    ) -> Result<Response<SessionState>, Status> {
        let request = request.into_inner();
        if request.session_id.is_empty() {
            let latest = self.latest.lock();
            return Ok(Response::new(session_state(&latest)));
        }
        let session_states = self.session_states.lock();
        let Some(state) = session_states.get(&request.session_id) else {
            return Err(Status::not_found(format!(
                "session {} is not available",
                request.session_id
            )));
        };
        Ok(Response::new(session_state(state)))
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
            .map(session_summary_wire);
        Ok(Response::new(CreateSessionResponse { session }))
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

    async fn update_session_metadata(
        &self,
        request: Request<theway_grpc::UpdateSessionMetadataRequest>,
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
        // session (or empty). There is no session-switch RPC; clients address
        // sessions explicitly by id.
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
        // Tell the event loop to drop the deleted session's runtime (and swap
        // the active runtime when the deleted session was current), so later
        // attaches never land on a deleted session id.
        let _ = self
            .commands
            .send(WireCommand::SessionDeleted { id: full_id });
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

    async fn activate_session(
        &self,
        request: Request<theway_grpc::ActivateSessionRequest>,
    ) -> Result<Response<theway_grpc::ActivateSessionResponse>, Status> {
        let request =
            activate_session_request_from_proto(&request.into_inner()).map_err(rpc_status)?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.commands
            .send(WireCommand::ActivateSession {
                request,
                response: response_tx,
            })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        let response = match response_rx.await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(rpc_status(error)),
            Err(_) => {
                return Err(Status::unavailable(
                    "event loop closed before activation reply",
                ));
            }
        };
        if let Some(summary) = &response.session {
            *self.session_id.write().unwrap() = summary.session_id.clone();
            let mut latest = self.latest.lock();
            latest.session_id = summary.session_id.clone();
            latest.cwd = summary.cwd.clone();
            latest.model = summary.model.clone();
            latest.busy = false;
        }
        Ok(Response::new(activate_session_response_to_proto(&response)))
    }

    async fn set_credential(
        &self,
        request: Request<theway_grpc::SetCredentialRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = set_credential_request_from_proto(&request.into_inner());
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.commands
            .send(WireCommand::SetCredential {
                request,
                response: response_tx,
            })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        match response_rx.await {
            Ok(Ok(())) => Ok(Response::new(CommandResult { accepted: true })),
            Ok(Err(error)) => Err(rpc_status(error)),
            Err(_) => Err(Status::unavailable(
                "event loop closed before credential reply",
            )),
        }
    }

    async fn clear_credential(
        &self,
        request: Request<theway_grpc::ClearCredentialRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let request = clear_credential_request_from_proto(&request.into_inner());
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.commands
            .send(WireCommand::ClearCredential {
                request,
                response: response_tx,
            })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        match response_rx.await {
            Ok(Ok(())) => Ok(Response::new(CommandResult { accepted: true })),
            Ok(Err(error)) => Err(rpc_status(error)),
            Err(_) => Err(Status::unavailable(
                "event loop closed before credential reply",
            )),
        }
    }
}
