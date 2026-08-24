use super::*;

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
