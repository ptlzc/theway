// Session lifecycle commands: activating a provisioned session and reacting to
// a transport-level session deletion.
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    /// A transport `delete_session` succeeded: drop the deleted session's
    /// runtime. A parked session is removed from the registry; the ACTIVE
    /// session is swapped to the most recent remaining session — or kept as
    /// an unreachable runtime when none remain (its id is gone from the
    /// repository, so the next client attach creates a fresh session).
    async fn handle_session_deleted(&mut self, id: &str, turn: &mut TurnState) {
        if id != self.session.id {
            self.sessions.remove(id);
            return;
        }
        let remaining = match self.session.repository.list().await {
            Ok(records) => records,
            Err(error) => {
                self.error_line(format!(
                    "active session deleted: fallback lookup failed: {error}"
                ));
                return;
            }
        };
        let Some(fallback_id) = remaining.last().map(|record| record.id.clone()) else {
            self.system_line(format!("deleted active session {}; no sessions remain", id));
            return;
        };
        let runtime = match (self.session.factory)(fallback_id.clone()).await {
            Ok(runtime) => runtime,
            Err(error) => {
                self.error_line(format!(
                    "active session deleted: cannot open fallback session {fallback_id}: {error:#}"
                ));
                return;
            }
        };
        if turn.fut.is_some() {
            self.request_abort(turn);
            if let Some(future) = turn.fut.take() {
                let _ = future.await;
            }
        }
        let cwd = runtime.cwd.clone();
        let new_state = SessionRuntimeState::from_runtime(
            runtime,
            self.session.factory.clone(),
            self.session.repository.clone(),
            self.session.retry.clone(),
            self.session.log_path.clone(),
            FeedProjectionState::new(
                self.projection.capabilities.clone(),
                self.projection.thinking_summary.clone(),
            ),
        );
        let old = std::mem::replace(&mut self.session, new_state);
        self.sessions.insert(old);
        if self.runtime.cwd != cwd {
            self.runtime.cwd = cwd;
        }
        self.session
            .kernel
            .harness()
            .session_switched(&self.session.id)
            .await;
        self.automation
            .reload
            .set_trigger_executor(self.session.kernel.trigger_executor().clone());
        self.clear_feed();
        // Resume replay: the fallback runtime rehydrated its transcript, so
        // rebuild the feed from history (capped at `tui_max_feed_lines`).
        crate::feed_replay::replay_transcript(
            &mut self.projection.feed,
            &crate::feed_replay::message_entries(
                &self.session.kernel.harness().agent().state().messages,
            ),
            self.runtime.feed_history_limit,
        );
        self.system_line(format!(
            "deleted session {}; switched to {}",
            id, self.session.id
        ));
        self.session.busy = false;
        self.session.queue.clear();
        self.session.cumulative_usage = WireContextUsage::default();
        self.projection.control_plane_prompt = None;
        turn.aborted = false;
        turn.prefix = "";
        self.refresh_goal_state().await;
        self.publish_current_snapshot().await;
    }

    async fn handle_activate_session(
        &mut self,
        request: WireActivateSessionRequest,
        turn: &mut TurnState,
    ) -> Result<WireActivateSessionResponse, WireRpcError> {
        let current_harness = self.session.kernel.harness().clone();
        let activator = self
            .automation
            .services
            .session_activator
            .get()
            .ok_or_else(|| WireRpcError {
                code: "failed_precondition".into(),
                message: "session activator is not installed".into(),
            })?;
        let activation = activator
            .activate(&request, &current_harness)
            .await?;
        let response = WireActivateSessionResponse {
            session: Some(activation.summary.clone()),
            created: activation.created,
        };
        self.apply_activation(activation, turn).await;
        Ok(response)
    }
}
