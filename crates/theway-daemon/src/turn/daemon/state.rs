impl TurnHost {
    /// Ensure a session runtime exists for `id`, building it through the active
    /// session's factory when needed. Parked runtimes are stored in the registry;
    /// the active session is always present.
    async fn ensure_session_runtime(&mut self, id: &str) -> Result<(), String> {
        if id == self.session.id || self.sessions.contains(id) {
            return Ok(());
        }
        let runtime = (self.session.factory)(id.to_string())
            .await
            .map_err(|e| format!("build runtime for session {id}: {e:#}"))?;
        // Resume replay: the freshly built runtime rehydrated its transcript,
        // so seed the parked projection's feed with the history (capped at the
        // TUI's max feed lines) instead of showing an empty conversation.
        let mut feed_state = FeedProjectionState::new(
            self.projection.capabilities.clone(),
            self.projection.thinking_summary.clone(),
        );
        crate::feed_replay::replay_transcript(
            &mut feed_state.feed,
            &runtime.harness.agent().state().messages,
            self.runtime.feed_history_limit,
        );
        let state = SessionRuntimeState::from_runtime(
            runtime,
            self.session.factory.clone(),
            self.session.repository.clone(),
            self.session.retry.clone(),
            self.session.log_path.clone(),
            feed_state,
        );
        self.sessions.insert(state);
        Ok(())
    }

    async fn set_model_for_session(&mut self, session_id: &str, spec: &str) -> bool {
        if session_id == self.session.id {
            return self.set_model_from_spec(spec).await;
        }
        if self.ensure_session_runtime(session_id).await.is_err() {
            return false;
        }
        let Some(incoming) = self.sessions.remove(session_id) else {
            return false;
        };
        let old = std::mem::replace(&mut self.session, incoming);
        let ok = self.set_model_from_spec(spec).await;
        let restored = std::mem::replace(&mut self.session, old);
        self.sessions.insert(restored);
        ok
    }

    async fn set_thinking_for_session(&mut self, session_id: &str, level: &str) -> bool {
        if session_id == self.session.id {
            return self.set_thinking_level(level).await;
        }
        if self.ensure_session_runtime(session_id).await.is_err() {
            return false;
        }
        let Some(incoming) = self.sessions.remove(session_id) else {
            return false;
        };
        let old = std::mem::replace(&mut self.session, incoming);
        let ok = self.set_thinking_level(level).await;
        let restored = std::mem::replace(&mut self.session, old);
        self.sessions.insert(restored);
        ok
    }

    fn cancel_session(&mut self, session_id: &str) {
        if session_id == self.session.id {
            // Active cancellation needs the event-loop turn; handled by caller
            // through `request_abort(turn)`.
            return;
        }
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.kernel.abort();
            session.aborted = true;
            session.queue.clear();
        }
    }

    fn apply_feed_update(&mut self, session_id: &str, update: FeedUpdate) -> bool {
        if session_id == self.session.id {
            apply_feed_update_to_projection(
                &self.inputs.feed_tx,
                session_id,
                &mut self.projection,
                update,
            )
        } else if let Some(session) = self.sessions.get_mut(session_id) {
            apply_feed_update_to_projection(
                &self.inputs.feed_tx,
                session_id,
                &mut session.projection,
                update,
            )
        } else {
            false
        }
    }

    async fn refresh_goal_state(&mut self) {
        self.projection.latest_goal = theway_core::multiagent::goal::current(self.session.kernel.harness()).await;
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

    /// Apply a thinking level to the active harness (typed-RPC twin of the
    /// `/thinking` slash command). Returns `true` when the level parsed and
    /// the harness accepted it.
    async fn set_thinking_level(&mut self, level: &str) -> bool {
        let parsed: theway_core::ThinkingLevel = match level.trim().parse() {
            Ok(level) => level,
            Err(_) => {
                self.error_line(format!(
                    "invalid thinking level: {level} (expected one of {})",
                    theway_transport::commands::THINKING_LEVEL_VALUES.join(", ")
                ));
                return false;
            }
        };
        match self.session.kernel.harness().set_thinking_level(parsed).await {
            Ok(_) => {
                self.system_line(format!("thinking level: {}", parsed.as_str()));
                // Keep the shared GetConfig view in sync with the runtime.
                let mut view = self.runtime.config.write().unwrap();
                view.thinking_level = Some(parsed.as_str().to_string());
                drop(view);
                true
            }
            Err(e) => {
                self.error_line(format!("set_thinking_level failed: {e}"));
                false
            }
        }
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
            runtime,
            repository,
            context,
            ..
        } = activation;
        let cwd = runtime.cwd.clone();
        let new_state = SessionRuntimeState::from_runtime(
            runtime,
            self.session.factory.clone(),
            repository,
            self.session.retry.clone(),
            self.session.log_path.clone(),
            FeedProjectionState::new(
                self.projection.capabilities.clone(),
                self.projection.thinking_summary.clone(),
            ),
        );
        let old = std::mem::replace(&mut self.session, new_state);
        self.sessions.insert(old);
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
        // Resume replay: the activated runtime rehydrated its transcript, so
        // rebuild the feed from history (capped at `tui_max_feed_lines`).
        crate::feed_replay::replay_transcript(
            &mut self.projection.feed,
            &self.session.kernel.harness().agent().state().messages,
            self.runtime.feed_history_limit,
        );
        self.system_line(format!("activated session {}", self.session.id));
        self.session.busy = false;
        self.session.queue.clear();
        self.session.cumulative_usage = WireContextUsage::default();
        self.projection.control_plane_prompt = None;
        turn.aborted = false;
        turn.prefix = "";
        self.refresh_goal_state().await;
        self.publish_current_snapshot().await;
    }

    fn show_control_plane_prompt(&mut self, prompt: PendingControlPlanePrompt) {
        let session_id = prompt.session_id.clone();
        let label = prompt.request.label.clone();
        let tool_name = prompt.request.tool_name.clone();
        if session_id == self.session.id {
            self.projection.control_plane_prompt = Some(prompt);
            self.system_line(format!("approval required: {label} ({tool_name})"));
        } else if let Some(session) = self.sessions.get_mut(&session_id) {
            session.projection.control_plane_prompt = Some(prompt);
            session.projection.feed.push_plain_untimed(
                format!("approval required: {label} ({tool_name})"),
                Level::System,
            );
        }
    }

    #[allow(dead_code)]
    fn resolve_control_plane_prompt(&mut self, decision: theway_core::ControlPlanePromptDecision) {
        let session_id = self.session.id.clone();
        self.resolve_control_plane_prompt_for_session(&session_id, decision);
    }

    fn resolve_control_plane_prompt_for_session(
        &mut self,
        session_id: &str,
        decision: theway_core::ControlPlanePromptDecision,
    ) {
        let prompt = if session_id == self.session.id {
            self.projection.control_plane_prompt.take()
        } else {
            self.sessions
                .get_mut(session_id)
                .and_then(|session| session.projection.control_plane_prompt.take())
        };
        let Some(prompt) = prompt else {
            return;
        };
        if prompt.session_id != session_id {
            if session_id == self.session.id {
                self.projection.control_plane_prompt = Some(prompt);
            } else if let Some(session) = self.sessions.get_mut(session_id) {
                session.projection.control_plane_prompt = Some(prompt);
            }
            return;
        }
        let outcome = match decision {
            theway_core::ControlPlanePromptDecision::Allow => "allowed",
            theway_core::ControlPlanePromptDecision::Deny { .. } => "denied",
            theway_core::ControlPlanePromptDecision::Timeout => "timed out",
        };
        let line = format!("permission {outcome}: {}", prompt.request.tool_name);
        if session_id == self.session.id {
            self.system_line(line);
        } else if let Some(session) = self.sessions.get_mut(session_id) {
            session
                .projection
                .feed
                .push_plain_untimed(line, Level::System);
        }
        prompt.resolve(decision);
    }
}

fn apply_feed_update_to_projection(
    feed_tx: &mpsc::UnboundedSender<(String, FeedUpdate)>,
    session_id: &str,
    projection: &mut FeedProjectionState,
    update: FeedUpdate,
) -> bool {
    let metadata_dirty = matches!(
        &update,
        FeedUpdate::TriggerPollStatus(_) | FeedUpdate::SkillsReloaded { .. }
    );
    let before_len = projection.feed.blocks().len();
    let targeted = match &update {
        FeedUpdate::ThinkingSummary { block_index, .. } => Some(*block_index),
        FeedUpdate::TextDelta(_) | FeedUpdate::ThinkingDelta(_) => before_len.checked_sub(1),
        FeedUpdate::ToolProgress { tool_call_id, .. }
        | FeedUpdate::ToolEnd { tool_call_id, .. } => projection.feed.tool_result_index(tool_call_id),
        _ => None,
    };
    match update {
        FeedUpdate::TriggerPollStatus(status) => {
            projection.latest_trigger_poll = Some(status);
        }
        FeedUpdate::SkillsReloaded { .. } => {}
        update => super::thinking_summary::apply(
            session_id,
            &mut projection.feed,
            &mut projection.thinking_burst,
            projection.thinking_summary.as_ref(),
            feed_tx,
            update,
        ),
    }
    if let Some(index) = targeted {
        projection.dirty_blocks.insert(index);
    }
    metadata_dirty
}
