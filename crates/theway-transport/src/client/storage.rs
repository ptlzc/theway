impl GrpcClient {
    // ── runtime state storage (issue #84) ─────────────────────────────

    /// List sessions through the `StorageService` RPC (mirror of the
    /// `SessionService` list).
    pub async fn state_list_sessions(&mut self) -> Result<(Vec<SessionSummary>, String)> {
        let response = self
            .storage
            .list_sessions(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("state_list_sessions: {e}"))?
            .into_inner();
        let sessions = response
            .sessions
            .iter()
            .map(crate::proto::session_summary_from_proto)
            .collect();
        Ok((sessions, response.current_session_id))
    }

    /// Create a session through the `StorageService` RPC.
    pub async fn state_create_session(&mut self, name: Option<String>) -> Result<SessionSummary> {
        self.state_create_session_with_metadata(None, name, std::collections::HashMap::new())
            .await
    }

    /// Create a session with custom id and metadata through `StorageService`.
    pub async fn state_create_session_with_metadata(
        &mut self,
        session_id: Option<&str>,
        name: Option<String>,
        metadata: std::collections::HashMap<String, String>,
    ) -> Result<SessionSummary> {
        let response = self
            .storage
            .create_session(CreateSessionRequest {
                name,
                session_id: session_id.map(str::to_string),
                metadata,
            })
            .await
            .map_err(|e| anyhow::anyhow!("state_create_session: {e}"))?
            .into_inner();
        let session = response
            .session
            .context("state_create_session returned no session summary")?;
        Ok(crate::proto::session_summary_from_proto(&session))
    }

    /// Update metadata through the `StorageService` RPC.
    pub async fn state_update_session_metadata(
        &mut self,
        session_id: &str,
        metadata: std::collections::HashMap<String, String>,
    ) -> Result<bool> {
        let accepted = self
            .storage
            .update_session_metadata(crate::proto::theway_grpc::UpdateSessionMetadataRequest {
                session_id: session_id.to_string(),
                metadata,
            })
            .await
            .map_err(|e| anyhow::anyhow!("state_update_session_metadata: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Rename a session through the `StorageService` RPC.
    pub async fn state_rename_session(&mut self, id: &str, name: &str) -> Result<bool> {
        let accepted = self
            .storage
            .rename_session(RenameSessionRequest {
                session_id: id.to_string(),
                name: name.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("state_rename_session: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Delete a session through the `StorageService` RPC.
    pub async fn state_delete_session(&mut self, id: &str) -> Result<Vec<String>> {
        let response = self
            .storage
            .delete_session(DeleteSessionRequest {
                session_id: id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("state_delete_session: {e}"))?;
        Ok(response.into_inner().running_run_ids)
    }

    /// Persist one DAG run snapshot through the `StorageService` RPC.
    pub async fn state_save_dag_run(
        &mut self,
        request: &WireSaveDagRunRequest,
    ) -> Result<WireSaveDagRunResult> {
        let response = self
            .storage
            .save_dag_run(crate::state::save_dag_run_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("state_save_dag_run: {e}"))?;
        Ok(crate::state::save_dag_run_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Load stored DAG runs through the `StorageService` RPC.
    pub async fn state_load_dag_runs(
        &mut self,
        request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        let response = self
            .storage
            .load_dag_runs(crate::state::load_dag_runs_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("state_load_dag_runs: {e}"))?;
        Ok(crate::state::load_dag_runs_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Replace trigger rules through the `StorageService` RPC.
    pub async fn state_save_trigger_rules(
        &mut self,
        request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        let response = self
            .storage
            .save_trigger_rules(crate::state::save_trigger_rules_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("state_save_trigger_rules: {e}"))?;
        Ok(crate::state::save_trigger_rules_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Load trigger rules through the `StorageService` RPC.
    pub async fn state_load_trigger_rules(
        &mut self,
        request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        let response = self
            .storage
            .load_trigger_rules(crate::state::load_trigger_rules_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("state_load_trigger_rules: {e}"))?;
        Ok(crate::state::load_trigger_rules_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Replace cron jobs through the `StorageService` RPC.
    pub async fn state_save_cron_jobs(
        &mut self,
        request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        let response = self
            .storage
            .save_cron_jobs(crate::state::save_cron_jobs_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("state_save_cron_jobs: {e}"))?;
        Ok(crate::state::save_cron_jobs_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Load cron jobs through the `StorageService` RPC.
    pub async fn state_load_cron_jobs(
        &mut self,
        request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        let response = self
            .storage
            .load_cron_jobs(crate::state::load_cron_jobs_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("state_load_cron_jobs: {e}"))?;
        Ok(crate::state::load_cron_jobs_response_from_proto(
            &response.into_inner(),
        ))
    }
}
