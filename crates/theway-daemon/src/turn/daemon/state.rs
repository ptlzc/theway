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
        state.cwd = self.session.cwd.display().to_string();
    }

    fn current_model_accepts_images(&self) -> bool {
        self.session.kernel.current_model_accepts_images()
    }

    async fn set_model_from_spec(&mut self, spec: &str) -> bool {
        if let Some((provider, id)) = commands::parse_model_spec(spec) {
            let provider_obj = theway_llm_provider::Provider::from(provider);
            if let Some(model) = theway_llm_provider::get_model(&provider_obj, id) {
                return self.apply_model(model).await;
            }
            // A slash-separated string may be either `provider/model` or a bare
            // model id that itself contains `/` (e.g. Cloudflare model ids).
            // Only fail here when the first component names a real provider or
            // the spec uses `:`; otherwise fall through to bare-id resolution.
            let looks_like_provider_spec = spec.contains(':')
                || theway_llm_provider::list_models()
                    .iter()
                    .any(|model| model.provider.0 == provider);
            if looks_like_provider_spec {
                self.error_line(format!("unknown model: {provider}:{id}"));
                return false;
            }
        }

        // Bare model ids are accepted when they resolve unambiguously against the
        // registered model catalog, using the daemon's base URL to disambiguate.
        let id = spec.trim();
        let base_url = self
            .runtime
            .config
            .read()
            .unwrap()
            .base_url
            .clone()
            .unwrap_or_default();
        let candidates: Vec<_> = if base_url.is_empty() {
            theway_llm_provider::list_models()
                .into_iter()
                .filter(|model| model.id == id)
                .collect()
        } else {
            let exact: Vec<_> = theway_llm_provider::list_models()
                .into_iter()
                .filter(|model| model.id == id && model.base_url == base_url)
                .collect();
            if exact.is_empty() {
                theway_llm_provider::list_models()
                    .into_iter()
                    .filter(|model| model.id == id && model.base_url.is_empty())
                    .collect()
            } else {
                exact
            }
        };
        if candidates.len() == 1 {
            return self.apply_model(candidates.into_iter().next().unwrap()).await;
        }
        if candidates.len() > 1 {
            self.error_line(format!(
                "ambiguous model id: {id}; use provider:model to disambiguate"
            ));
        } else {
            self.error_line(format!("invalid model spec: {spec}"));
        }
        false
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
        self.session.cwd = runtime.cwd.clone();
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
        self.session.cumulative_usage = WireContextUsage::default();
        self.projection.control_plane_prompt = None;
        self.refresh_goal_state().await;
        self.sync_current_session_state();
        Ok(())
    }

    async fn apply_activation(&mut self, activation: crate::session_activation::SessionActivation, turn: &mut TurnState) {
        if turn.fut.is_some() {
            self.request_abort(turn);
            if let Some(future) = turn.fut.take() {
                let _ = future.await;
            }
        }
        let previous = self.session.kernel.harness().clone();
        previous.shutdown_runtime_extensions().await;
        let crate::session_activation::SessionActivation {
            session_id,
            runtime,
            repository,
            context,
            ..
        } = activation;
        let cwd = runtime.cwd.clone();
        let tool_count = runtime.tool_names.len();
        self.session.id = session_id;
        self.session.cwd = cwd.clone();
        self.session.tool_count = tool_count;
        self.session.repository = repository;
        self.session.kernel.replace_runtime(runtime);
        self.runtime.cwd = cwd;
        self.runtime.paths = context.paths.clone();
        self.runtime.registry.set_file_commands(crate::file_commands::scan_file_commands(
            &self.runtime.cwd,
            &self.runtime.paths.home,
        ));
        self.runtime.completer = SlashCompleter::from_commands(slash_commands(&self.runtime.registry));
        self.session
            .kernel
            .harness()
            .session_switched(&self.session.id)
            .await;
        self.automation.reload
            .set_trigger_executor(self.session.kernel.trigger_executor().clone());
        self.clear_feed();
        self.system_line(format!("activated session {}", self.session.id));
        self.session.busy = false;
        self.session.queue.clear();
        self.session.cumulative_usage = WireContextUsage::default();
        self.projection.control_plane_prompt = None;
        turn.aborted = false;
        turn.prefix = "";
        self.refresh_goal_state().await;
        self.sync_current_session_state();
        self.publish_current_snapshot().await;
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
