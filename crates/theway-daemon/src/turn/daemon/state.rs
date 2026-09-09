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
        if !self.sessions.contains(session_id) {
            // Same lazy-build rule as `set_thinking_for_session`: a fresh
            // TUI-created session receiving its configured default must not
            // pay for a full runtime build on the serialized command loop.
            // Persist the model change in the transcript; the build on first
            // submit rehydrates it and refreshes the DAG launcher.
            return self.persist_model_for_session(session_id, spec).await;
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

    /// Persist a model change directly into a session that has no live
    /// runtime. The next build rehydrates the transcript, exactly like a
    /// resumed session, and `assemble_opened` refreshes the DAG launcher with
    /// the rehydrated model.
    async fn persist_model_for_session(&mut self, session_id: &str, spec: &str) -> bool {
        let model = match self.resolve_model_from_spec(spec) {
            Ok(model) => model,
            Err(message) => {
                self.error_line(message);
                return false;
            }
        };
        let store = match self.session.repository.open(session_id).await {
            Ok(Some(store)) => store,
            Ok(None) => {
                self.error_line(format!("set model: no session matches id {session_id}"));
                return false;
            }
            Err(error) => {
                self.error_line(format!("set model for session {session_id}: {error:#}"));
                return false;
            }
        };
        let provider = model.provider.0.clone();
        let model_id = model.id.clone();
        let session = theway_core::Session::from_store(store);
        match session.append_model_change(&provider, &model_id).await {
            Ok(_) => {
                self.system_line(format!(
                    "selected {provider}:{model_id} for session {session_id}"
                ));
                true
            }
            Err(error) => {
                self.error_line(format!("set model for session {session_id}: {error}"));
                false
            }
        }
    }

    async fn set_thinking_for_session(&mut self, session_id: &str, level: &str) -> bool {
        if session_id == self.session.id {
            return self.set_thinking_level(level).await;
        }
        if !self.sessions.contains(session_id) {
            // The session has no live runtime yet (e.g. a TUI-created fresh
            // session receiving its configured default). Building the full
            // runtime here just to flip one flag can take longer than the
            // client's 15s RPC bound and wedges the serialized command loop.
            // Persist the change in the session transcript instead; the lazy
            // build on first submit rehydrates it into agent state.
            return self
                .persist_thinking_level_for_session(session_id, level)
                .await;
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

    /// Persist a thinking-level change directly into a session that has no
    /// live runtime. The next `SessionRuntimeBuilder` build rehydrates the
    /// transcript and applies the level, exactly like a resumed session.
    async fn persist_thinking_level_for_session(&mut self, session_id: &str, level: &str) -> bool {
        let parsed = match parse_thinking_level(level) {
            Ok(level) => level,
            Err(message) => {
                self.error_line(message);
                return false;
            }
        };
        let store = match self.session.repository.open(session_id).await {
            Ok(Some(store)) => store,
            Ok(None) => {
                self.error_line(format!(
                    "set thinking level: no session matches id {session_id}"
                ));
                return false;
            }
            Err(error) => {
                self.error_line(format!(
                    "set thinking level for session {session_id}: {error:#}"
                ));
                return false;
            }
        };
        let session = theway_core::Session::from_store(store);
        match session.append_thinking_level_change(parsed.as_str()).await {
            Ok(_) => {
                self.system_line(format!(
                    "thinking level (session {session_id}): {}",
                    parsed.as_str()
                ));
                // Keep the shared GetConfig view in sync with the runtime, as
                // the parked-runtime path does through `set_thinking_level`.
                let mut view = self.runtime.config.write().unwrap();
                view.thinking_level = Some(parsed.as_str().to_string());
                drop(view);
                true
            }
            Err(error) => {
                self.error_line(format!(
                    "set thinking level for session {session_id}: {error}"
                ));
                false
            }
        }
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
        self.projection.latest_goal =
            theway_core::multiagent::goal::current(self.session.kernel.harness()).await;
    }

    fn current_model_accepts_images(&self) -> bool {
        self.session.kernel.current_model_accepts_images()
    }

    async fn set_model_from_spec(&mut self, spec: &str) -> bool {
        match self.resolve_model_from_spec(spec) {
            Ok(model) => self.apply_model(model).await,
            Err(message) => {
                self.error_line(message);
                false
            }
        }
    }

    /// Resolve a model spec against the registered catalog without applying
    /// it. Accepts `provider:model` / `provider/model` pairs and unambiguous
    /// bare model ids (the daemon's base URL disambiguates when set).
    fn resolve_model_from_spec(&self, spec: &str) -> Result<theway_llm_provider::Model, String> {
        if let Some((provider, id)) = commands::parse_model_spec(spec) {
            let provider_obj = theway_llm_provider::Provider::from(provider);
            if let Some(model) = theway_llm_provider::get_model(&provider_obj, id) {
                return Ok(model);
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
                return Err(format!("unknown model: {provider}:{id}"));
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
            return Ok(candidates.into_iter().next().unwrap());
        }
        if candidates.len() > 1 {
            Err(format!(
                "ambiguous model id: {id}; use provider:model to disambiguate"
            ))
        } else {
            Err(format!("invalid model spec: {spec}"))
        }
    }

    async fn apply_model(&mut self, model: theway_llm_provider::Model) -> bool {
        let provider = model.provider.0.clone();
        let id = model.id.clone();
        match self.session.kernel.harness().set_model(model.clone()).await {
            Ok(_) => {
                // Propagate the switch to the session's DAG launcher: node jobs inherit the
                // model snapshot taken at session activation, so a model set after attach
                // must rebuild the launcher (reusing the session's skill harness cell).
                if let Some(activator) = self.automation.services.session_activator.get()
                    && let Some(builder) = activator.builder()
                {
                    builder.refresh_dag_launcher(&self.session.id, model.clone());
                }
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
        let parsed = match parse_thinking_level(level) {
            Ok(level) => level,
            Err(message) => {
                self.error_line(message);
                return false;
            }
        };
        match self
            .session
            .kernel
            .harness()
            .set_thinking_level(parsed)
            .await
        {
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

    /// Move the active projection into a parked session's projection slot and
    /// reset the active projection for the incoming session.
    ///
    /// Active sessions render from `self.projection`; parked sessions render from
    /// `SessionRuntimeState::projection`. Without this handoff, a previously active
    /// session would be parked with an empty feed and any later authoritative
    /// snapshot for it would clear the client's history.
    fn take_active_projection(&mut self) -> FeedProjectionState {
        let next = FeedProjectionState::new(
            self.projection.capabilities.clone(),
            self.projection.thinking_summary.clone(),
        );
        std::mem::replace(&mut self.projection, next)
    }

    /// Release a collapsed source session's runtime from memory: a parked
    /// source is dropped outright (any in-flight turn is aborted first); the
    /// ACTIVE source is replaced by the child, which becomes the active
    /// session, and the old runtime is dropped instead of parked. Either way
    /// only the source's persisted record remains — its runtime is rebuilt
    /// lazily if a client addresses it again. `note` (the collapse
    /// confirmation) is re-emitted into the surviving feed, because the
    /// source feed is dropped with its runtime.
    async fn handle_collapse_unload(
        &mut self,
        request: crate::commands::CollapseUnloadRequest,
        turn: &mut TurnState,
    ) {
        let crate::commands::CollapseUnloadRequest {
            source_id,
            child_id,
            note,
        } = request;
        // Drop session-scoped execution state (credentials + work-dir
        // context) for the collapsed source.
        self.automation.services.session_execution.remove(&source_id);

        if source_id != self.session.id {
            // Parked source: abort any in-flight turn and drop the runtime
            // (kernel + projection + queue) outright.
            if let Some(state) = self.sessions.get_mut(&source_id) {
                state.kernel.abort();
                state.aborted = true;
                state.queue.clear();
            }
            self.sessions.remove(&source_id);
            if let Some(session_states) = &self.runtime.session_states {
                session_states.lock().remove(&source_id);
            }
            self.system_line(note);
            return;
        }

        // Active source: promote the child to the active slot, dropping the
        // source runtime entirely (it is NOT parked). If the child runtime
        // cannot be built, keep the source active.
        if self.ensure_session_runtime(&child_id).await.is_err() {
            self.error_line(format!(
                "collapse unload: cannot build child runtime {child_id}; \
                 source session kept active"
            ));
            return;
        }
        if turn.fut.is_some() {
            self.request_abort(turn);
            if let Some(future) = turn.fut.take() {
                let _ = future.await;
            }
        }
        let previous = self.session.kernel.harness().clone();
        previous.shutdown_runtime_extensions().await;
        drop(self.take_active_projection());
        let Some(mut incoming) = self.sessions.remove(&child_id) else {
            self.error_line(format!(
                "collapse unload: child runtime {child_id} vanished"
            ));
            return;
        };
        let child_cwd = incoming.cwd.clone();
        // The active session's projection lives on `TurnHost::projection`;
        // the dormant per-state projection is reset like `apply_activation`.
        incoming.projection = FeedProjectionState::new(
            self.projection.capabilities.clone(),
            self.projection.thinking_summary.clone(),
        );
        drop(std::mem::replace(&mut self.session, incoming));
        if self.runtime.cwd != child_cwd {
            self.runtime.cwd = child_cwd;
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
        // Resume replay: the child runtime rehydrated its transcript (the
        // compact summary), so rebuild the feed from history.
        crate::feed_replay::replay_transcript(
            &mut self.projection.feed,
            &self.session.kernel.harness().agent().state().messages,
            self.runtime.feed_history_limit,
        );
        self.system_line(note);
        self.session.busy = false;
        self.session.queue.clear();
        self.session.cumulative_usage = WireContextUsage::default();
        self.projection.control_plane_prompt = None;
        turn.aborted = false;
        turn.prefix = "";
        self.refresh_goal_state().await;
        self.publish_current_snapshot().await;
    }

    async fn apply_activation(
        &mut self,
        activation: crate::session_activation::SessionActivation,
        turn: &mut TurnState,
    ) {
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
        // Session-level MCP overlay (session-scoped-mcp): the activated context
        // carries the per-session slot the harness was just built from. Keep it
        // on the session state so snapshots, `/reload`, and a later `Configure`
        // address this session's MCP state rather than the daemon's.
        let mcp_overlay = context.mcp.overlay.clone();
        let mcp_capabilities = context.mcp.capabilities();
        if let Some(overlay) = mcp_overlay.as_ref() {
            // The build that just ran registered exactly the slot's hooks.
            overlay.slot.write().unwrap().mark_hooks_registered();
        }
        let cwd = runtime.cwd.clone();
        let old_projection = self.take_active_projection();
        let mut new_state = SessionRuntimeState::from_runtime(
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
        new_state.mcp_overlay = mcp_overlay;
        apply_mcp_capabilities(&mut new_state.projection.capabilities, &mcp_capabilities);
        apply_mcp_capabilities(&mut self.projection.capabilities, &mcp_capabilities);
        let mut old = std::mem::replace(&mut self.session, new_state);
        old.projection = old_projection;
        self.sessions.insert(old);
        self.runtime.cwd = cwd;
        self.runtime.paths = context.paths.clone();
        self.runtime
            .registry
            .set_file_commands(crate::file_commands::scan_file_commands(
                &self.runtime.cwd,
                &self.runtime.paths.home,
            ));
        self.runtime.completer =
            SlashCompleter::from_commands(slash_commands(&self.runtime.registry));
        self.session
            .kernel
            .harness()
            .session_switched(&self.session.id)
            .await;
        self.automation
            .reload
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

    /// Re-merge the active session's MCP overlay after the daemon-level
    /// provision slot changed (`Configure`): recompute the per-session slot
    /// with same-name daemon entries replaced, swap the live harness's MCP
    /// tools, and register the hooks that are new instances. Sessions whose
    /// daemon layer is the local `mcp.toml` scan are left alone — `Configure`
    /// does not govern that layer.
    pub(crate) fn remerge_active_session_mcp(&mut self) {
        let Some(overlay) = self.session.mcp_overlay.clone() else {
            return;
        };
        if !overlay.from_global_slot {
            return;
        }
        let (daemon, daemon_configs) = {
            let slot = self.runtime.mcp_provision.read().unwrap();
            (slot.layer(), slot.configs.clone())
        };
        let overlay_names: std::collections::HashSet<String> = overlay
            .configs
            .iter()
            .map(|config| config.name.clone())
            .collect();
        // Read the session layer back out of its own slot rather than the
        // activation-time connection result: `/reload` replaces those
        // instances, and re-merging stale ones would resurrect dead clients.
        let session_layer = {
            let slot = overlay.slot.read().unwrap();
            crate::mcp_loader::McpLayer {
                servers: slot
                    .servers
                    .iter()
                    .filter(|server| overlay_names.contains(&server.name))
                    .cloned()
                    .collect(),
                inject_summary: slot
                    .inject_summary
                    .iter()
                    .filter(|name| overlay_names.contains(*name))
                    .cloned()
                    .collect(),
                inject_and_run: slot
                    .inject_and_run
                    .iter()
                    .filter(|name| overlay_names.contains(*name))
                    .cloned()
                    .collect(),
                errors: slot
                    .errors
                    .iter()
                    .filter(|(name, _)| overlay_names.contains(name))
                    .cloned()
                    .collect(),
            }
        };
        let merged = crate::mcp_loader::merge_mcp_layers(&daemon, &overlay_names, &session_layer);
        let effective_configs: Vec<_> = daemon_configs
            .into_iter()
            .filter(|config| !overlay_names.contains(&config.name))
            .chain(overlay.configs.iter().cloned())
            .collect();
        let (old_tools, new_tools, new_hooks, capabilities) = {
            use crate::trigger_engine::notification_hook::NotificationHook;
            let mut slot = overlay.slot.write().unwrap();
            let old_tools = slot.tools.clone();
            slot.replace_connection_result(effective_configs, (merged, Vec::new()));
            // Session-level hooks were registered by the session build and are
            // still the same instances; every other hook is a new instance.
            slot.registered_labels = session_layer
                .hooks()
                .iter()
                .map(|hook| hook.label().to_string())
                .collect();
            let capabilities = crate::orchestration::SessionMcpCapabilities {
                servers: slot.server_names.len(),
                tools: slot.tool_names.len(),
                notification_hooks: slot.hooks.len(),
                server_names: slot.server_names.clone(),
                tool_names: slot.tool_names.clone(),
                errors: slot.errors.clone(),
            };
            (
                old_tools,
                slot.tools.clone(),
                slot.hooks.clone(),
                capabilities,
            )
        };
        self.session
            .kernel
            .harness()
            .replace_mcp_tools(&old_tools, new_tools);
        {
            use crate::orchestration::session::NotificationHookSink;
            use crate::trigger_engine::notification_hook::NotificationHook;
            let executor = self.session.kernel.trigger_executor().clone();
            let mut slot = overlay.slot.write().unwrap();
            for hook in &new_hooks {
                let label = hook.label().to_string();
                if slot.registered_labels.insert(label) {
                    executor.register(hook.clone());
                }
            }
        }
        apply_mcp_capabilities(&mut self.projection.capabilities, &capabilities);
        apply_mcp_capabilities(&mut self.session.projection.capabilities, &capabilities);
    }
}

/// Copy the MCP fields of an activated session's capability view into a
/// projection's runtime capabilities.
fn apply_mcp_capabilities(
    target: &mut RuntimeCapabilities,
    mcp: &crate::orchestration::SessionMcpCapabilities,
) {
    target.mcp_servers = mcp.servers;
    target.mcp_tools = mcp.tools;
    target.mcp_notification_hooks = mcp.notification_hooks;
    target.mcp_server_names = mcp.server_names.clone();
    target.mcp_tool_names = mcp.tool_names.clone();
    target.mcp_server_errors = mcp.errors.clone();
}

/// Parse and validate a wire thinking level, returning the shared error line
/// text on failure.
fn parse_thinking_level(level: &str) -> Result<theway_core::ThinkingLevel, String> {
    match level.trim().parse() {
        Ok(level) => Ok(level),
        Err(_) => Err(format!(
            "invalid thinking level: {level} (expected one of {})",
            theway_transport::commands::THINKING_LEVEL_VALUES.join(", ")
        )),
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
        | FeedUpdate::ToolEnd { tool_call_id, .. } => {
            projection.feed.tool_result_index(tool_call_id)
        }
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
