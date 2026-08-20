impl TurnHost {
    fn apply_feed_update(&mut self, update: FeedUpdate) -> bool {
        let metadata_dirty = matches!(
            &update,
            FeedUpdate::TriggerPollStatus(_) | FeedUpdate::SkillsReloaded { .. }
        );
        let before_len = self.projection.feed.blocks().len();
        let targeted = match &update {
            FeedUpdate::ThinkingSummary { block_index, .. } => Some(*block_index),
            FeedUpdate::TextDelta(_) | FeedUpdate::ThinkingDelta(_) => before_len.checked_sub(1),
            FeedUpdate::ToolProgress { tool_call_id, .. }
            | FeedUpdate::ToolEnd { tool_call_id, .. } => self.projection.feed.tool_result_index(tool_call_id),
            _ => None,
        };
        match update {
            FeedUpdate::TriggerPollStatus(status) => {
                self.projection.latest_trigger_poll = Some(status);
            }
            FeedUpdate::SkillsReloaded { .. } => {}
            update => super::thinking_summary::apply(
                &mut self.projection.feed,
                &mut self.projection.thinking_burst,
                self.projection.thinking_summary.as_ref(),
                &self.inputs.feed_tx,
                update,
            ),
        }
        if let Some(index) = targeted {
            self.projection.dirty_blocks.insert(index);
        }
        metadata_dirty
    }

    async fn refresh_goal_state(&mut self) {
        self.projection.latest_goal = theway_core::multiagent::goal::current(self.session.kernel.harness()).await;
    }

    fn sync_current_session_state(&self) {
        let mut state = self.session.shared_state.lock();
        state.session_id = self.session.id.clone();
        state.busy = self.session.busy;
        state.model = current_model_label(self.session.kernel.harness());
        state.cwd = self.runtime.cwd.display().to_string();
    }

    fn current_model_accepts_images(&self) -> bool {
        self.session.kernel.current_model_accepts_images()
    }

    async fn set_model_from_spec(&mut self, spec: &str) -> bool {
        let Some((provider, id)) = commands::parse_model_spec(spec) else {
            self.error_line(format!("invalid model spec: {spec}"));
            return false;
        };
        let (provider, id) = (provider.to_string(), id.to_string());
        let Some(model) = theway_llm_provider::get_model(
            &theway_llm_provider::Provider::from(provider.as_str()),
            &id,
        ) else {
            self.error_line(format!("unknown model: {provider}:{id}"));
            return false;
        };
        self.apply_model(model).await
    }

    async fn apply_model(&mut self, model: theway_llm_provider::Model) -> bool {
        let provider = model.provider.0.clone();
        let id = model.id.clone();
        match self.session.kernel.harness().set_model(model).await {
            Ok(_) => {
                if let Some(hint) = commands::model_credential_hint(&provider) {
                    self.system_line(format!(
                        "selected {provider}:{id}, but login is required: {hint}"
                    ));
                } else {
                    self.system_line(format!("switched to {provider}:{id}"));
                }
                self.runtime.model_catalog = model_catalog();
                true
            }
            Err(e) => {
                self.error_line(format!("set_model failed: {e}"));
                false
            }
        }
    }

    async fn switch_session(&mut self, id: String) -> Result<()> {
        let previous = self.session.kernel.harness().clone();
        previous
            .before_session_switch(&id)
            .await
            .with_context(|| format!("extension gate rejected session {id}"))?;
        let runtime = (self.session.factory)(id.clone())
            .await
            .with_context(|| format!("build runtime for session {id}"))?;
        previous.shutdown_runtime_extensions().await;
        self.session.id = runtime.session_id.clone();
        self.session.kernel.replace_runtime(runtime);
        self.session
            .kernel
            .harness()
            .session_switched(&self.session.id)
            .await;
        self.automation.reload
            .set_trigger_executor(self.session.kernel.trigger_executor().clone());
        self.clear_feed();
        self.system_line(format!("switched to session {}", self.session.id));
        self.session.busy = false;
        self.session.queue.clear();
        self.projection.control_plane_prompt = None;
        self.refresh_goal_state().await;
        self.sync_current_session_state();
        Ok(())
    }

    fn show_control_plane_prompt(&mut self, prompt: PendingControlPlanePrompt) {
        self.projection.control_plane_prompt = Some(prompt);
        if let Some(prompt) = &self.projection.control_plane_prompt {
            self.system_line(format!(
                "approval required: {} ({})",
                prompt.request.label, prompt.request.tool_name
            ));
        }
    }

    fn resolve_control_plane_prompt(&mut self, decision: theway_core::ControlPlanePromptDecision) {
        let Some(prompt) = self.projection.control_plane_prompt.take() else {
            return;
        };
        let outcome = match decision {
            theway_core::ControlPlanePromptDecision::Allow => "allowed",
            theway_core::ControlPlanePromptDecision::Deny { .. } => "denied",
            theway_core::ControlPlanePromptDecision::Timeout => "timed out",
        };
        self.system_line(format!(
            "permission {outcome}: {}",
            prompt.request.tool_name
        ));
        prompt.resolve(decision);
    }
}
