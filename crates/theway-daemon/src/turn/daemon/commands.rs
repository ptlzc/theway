impl TurnHost {
    async fn handle_web_command(&mut self, command: WireCommand, turn: &mut TurnState) {
        match command {
            WireCommand::Submit {
                session_id: _,
                text,
                images,
                interrupt,
            } => self.submit_web_text(text, images, interrupt, turn).await,
            WireCommand::TriggerRuleNow { id } => self.trigger_web_rule_now(id, turn),
            WireCommand::Abort { session_id: _ } => self.request_abort(turn),
            WireCommand::ResolveControlPlane {
                session_id: _,
                approve,
            } => {
                let decision = if approve {
                    theway_core::ControlPlanePromptDecision::Allow
                } else {
                    theway_core::ControlPlanePromptDecision::Deny {
                        reason: Some("denied by user".into()),
                    }
                };
                self.resolve_control_plane_prompt(decision);
            }
            WireCommand::SetModel {
                session_id: _,
                spec,
                response,
            } => {
                let ok = self.set_model_from_spec(&spec).await;
                let _ = response.send(ok);
            }
            WireCommand::SetThinking {
                session_id: _,
                level,
                response,
            } => {
                let ok = self.set_thinking_level(&level).await;
                let _ = response.send(ok);
            }
            WireCommand::SwitchSession { id } => self.handle_switch_session(id, turn).await,
            WireCommand::SetSkillDirs { dirs } => self.handle_set_skill_dirs(dirs, turn).await,
            WireCommand::Configure { config } => self.handle_configure(config, turn).await,
            WireCommand::InvokeExtensionCommand {
                name,
                arguments,
                has_interactive_client,
                response,
            } => {
                let result = self
                    .handle_extension_command(name, arguments, has_interactive_client)
                    .await;
                let _ = response.send(result);
            }
            WireCommand::ReloadExtensions {
                cancel_active,
                response,
            } => {
                let result = self
                    .handle_extension_reload(cancel_active, turn)
                    .await;
                let _ = response.send(result);
            }
            WireCommand::DecideExtensionTrust { request, response } => {
                let result = self.handle_extension_trust(request).await;
                let _ = response.send(result);
            }
            WireCommand::ActivateSession { request, response } => {
                let result = self.handle_activate_session(request, turn).await;
                let _ = response.send(result);
            }
            WireCommand::SetCredential { request, response } => {
                let result = self.handle_set_credential(request);
                let _ = response.send(result);
            }
            WireCommand::ClearCredential { request, response } => {
                let result = self.handle_clear_credential(request);
                let _ = response.send(result);
            }
        }
    }

    fn handle_set_credential(
        &mut self,
        request: WireSetCredentialRequest,
    ) -> Result<(), WireRpcError> {
        if request.session_id.trim().is_empty() {
            return Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "session_id must not be empty".into(),
            });
        }
        if request.provider.trim().is_empty() {
            return Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "provider must not be empty".into(),
            });
        }
        self.automation
            .services
            .session_execution
            .set_credential(&request.session_id, &request.provider, request.secret)
            .map_err(|error| match error {
                crate::session_execution::RegistryError::SessionNotRegistered(id) => WireRpcError {
                    code: "not_found".into(),
                    message: format!("session {id} is not registered; activate it first"),
                },
                other => WireRpcError {
                    code: "failed_precondition".into(),
                    message: other.to_string(),
                },
            })
    }

    fn handle_clear_credential(
        &mut self,
        request: WireClearCredentialRequest,
    ) -> Result<(), WireRpcError> {
        let session_id = request.session_id.trim();
        if session_id.is_empty() {
            return Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "session_id must not be empty".into(),
            });
        }
        if self
            .automation
            .services
            .session_execution
            .get(session_id)
            .is_none()
        {
            return Err(WireRpcError {
                code: "not_found".into(),
                message: format!("session {session_id} is not registered; activate it first"),
            });
        }
        match request.provider.as_deref() {
            Some(provider) if provider.trim().is_empty() => Err(WireRpcError {
                code: "invalid_argument".into(),
                message: "provider must not be empty".into(),
            }),
            Some(provider) => {
                self.automation
                    .services
                    .session_execution
                    .clear_credential(session_id, provider);
                Ok(())
            }
            None => {
                self.automation
                    .services
                    .session_execution
                    .clear_credentials(session_id);
                Ok(())
            }
        }
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

    /// Apply a configuration patch on the serialized event loop. Only values
    /// whose runtime applier succeeds are committed to the shared GetConfig
    /// view; transport admission never mutates that view optimistically.
    async fn handle_configure(&mut self, config: WireDaemonConfig, turn: &mut TurnState) {
        let unknown = config.unknown_clear_fields();
        if !unknown.is_empty() {
            self.error_line(format!(
                "configure: unknown clear field(s): {}",
                unknown.join(", ")
            ));
            return;
        }

        let mut applied = WireDaemonConfig::default();

        if (config.clears("provider") && config.provider.is_none())
            || (config.clears("model") && config.model.is_none())
        {
            self.error_line("configure: the active provider/model cannot be cleared");
        } else if config.provider.is_some() != config.model.is_some() {
            self.error_line("configure: provider and model must be supplied together");
        } else if config.provider.is_some()
            || config.base_url.is_some()
            || config.clears("base_url")
        {
            let mut model = match (config.provider.as_deref(), config.model.as_deref()) {
                (Some(provider), Some(id)) => theway_llm_provider::get_model(
                    &theway_llm_provider::Provider::from(provider),
                    id,
                ),
                _ => self.session.kernel.harness().agent().state().model.clone(),
            };
            if config.clears("base_url")
                && let Some(current) = model.as_ref()
            {
                model = theway_llm_provider::get_model(&current.provider, &current.id)
                    .or_else(|| Some(current.clone()));
            }
            if let Some(model) = model.as_mut()
                && let Some(base_url) = config.base_url.as_ref()
            {
                model.base_url = base_url.clone();
            }
            match model {
                Some(model) if self.apply_model(model.clone()).await => {
                    applied.provider = Some(model.provider.0.clone());
                    applied.model = Some(model.id.clone());
                    if model.base_url.is_empty() {
                        applied.clear_fields.push("base_url".into());
                    } else {
                        applied.base_url = Some(model.base_url);
                    }
                }
                Some(_) => {}
                None => self.error_line("configure: no active or matching model to update"),
            }
        }

        if config.thinking.is_some() || config.clears("thinking") {
            let enabled = config.thinking.unwrap_or(false);
            let level = if enabled {
                theway_core::ThinkingLevel::High
            } else {
                theway_core::ThinkingLevel::Off
            };
            match self.session.kernel.harness().set_thinking_level(level).await {
                Ok(_) if config.thinking.is_none() => applied.clear_fields.push("thinking".into()),
                Ok(_) => applied.thinking = Some(enabled),
                Err(err) => self.error_line(format!("configure thinking: {err}")),
            }
        }

        // Full thinking level (persisted last-choice default): applies the
        // exact level, finer-grained than the legacy `thinking` toggle.
        if config.thinking_level.is_some() || config.clears("thinking_level") {
            let requested = config.thinking_level.as_deref();
            let level = match requested {
                Some(raw) => match raw.parse::<theway_core::ThinkingLevel>() {
                    Ok(level) => Some(level),
                    Err(err) => {
                        self.error_line(format!(
                            "configure thinking_level: invalid level {raw:?}: {err}"
                        ));
                        None
                    }
                },
                // Clearing the level falls back to the runtime default (off).
                None => Some(theway_core::ThinkingLevel::Off),
            };
            if let Some(level) = level {
                match self.session.kernel.harness().set_thinking_level(level).await {
                    Ok(_) if requested.is_none() => {
                        applied.clear_fields.push("thinking_level".into())
                    }
                    Ok(_) => applied.thinking_level = config.thinking_level.clone(),
                    Err(err) => self.error_line(format!("configure thinking_level: {err}")),
                }
            }
        }

        if !config.builtin_skills.is_empty() || config.clears("builtin_skills") {
            let requested = if config.clears("builtin_skills") && config.builtin_skills.is_empty() {
                Vec::new()
            } else {
                config.builtin_skills.clone()
            };
            let resolved = crate::builtin_skills::resolve_builtins(&[], &requested)
                .expect("an empty CLI list cannot produce a hard builtin error");
            for diagnostic in resolved.diagnostics {
                self.error_line(diagnostic);
            }
            let enabled: Vec<String> = resolved
                .skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect();
            let non_builtin: Vec<_> = self
                .session
                .kernel
                .harness()
                .skills()
                .into_iter()
                .filter(|skill| !matches!(skill.source, theway_core::SkillSource::Builtin))
                .collect();
            self.session.kernel
                .harness()
                .replace_skills(crate::builtin_skills::merge_with_user_project(
                    resolved.skills,
                    &non_builtin,
                ));
            if enabled.is_empty() {
                applied.clear_fields.push("builtin_skills".into());
            } else {
                applied.builtin_skills = enabled;
            }
        }

        if !config.skills_dirs.is_empty() || config.clears("skills_dirs") {
            let dirs = if config.skills_dirs.is_empty() {
                Vec::new()
            } else {
                config.skills_dirs.clone()
            };
            self.handle_set_skill_dirs(dirs, turn).await;
            let actual = self.runtime.path_context.read().unwrap().skills_dirs.clone();
            if actual.is_empty() {
                applied.clear_fields.push("skills_dirs".into());
            } else {
                applied.skills_dirs = actual;
            }
        }

        if let Some(secs) = config.trigger_poll_secs {
            if secs == 0 {
                self.error_line("configure: trigger_poll_secs must be greater than zero");
            } else {
                self.automation.services.dynamic_triggers.set_poll_interval_secs(secs);
                applied.trigger_poll_secs = Some(secs);
            }
        } else if config.clears("trigger_poll_secs") {
            self.automation.services.dynamic_triggers.set_poll_interval_secs(
                theway_transport::triggers::DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS,
            );
            applied.clear_fields.push("trigger_poll_secs".into());
        }

        if let Some(lines) = config.tui_max_feed_lines {
            if lines == 0 {
                self.error_line("configure: tui_max_feed_lines must be greater than zero");
            } else {
                self.runtime.feed_history_limit = Some(lines);
                applied.tui_max_feed_lines = Some(lines);
            }
        } else if config.clears("tui_max_feed_lines") {
            self.runtime.feed_history_limit = None;
            applied.clear_fields.push("tui_max_feed_lines".into());
        }

        if let Some(addr) = config.tool_service_addr.as_ref() {
            if addr.trim().is_empty() {
                self.error_line("configure: tool_service_addr must not be empty; clear it instead");
            } else {
                applied.tool_service_addr = Some(addr.clone());
            }
        } else if config.clears("tool_service_addr") {
            applied.clear_fields.push("tool_service_addr".into());
        }

        if config.storage_service_addr.is_some() || config.clears("storage_service_addr") {
            self.error_line(
                "configure: storage_service_addr is startup-only and cannot be changed at runtime",
            );
        }

        let touched = self.runtime.config.write().unwrap().merge_from(&applied);
        if touched == 0 {
            self.system_line("configure: no applicable settings changed");
        } else {
            self.system_line(format!("configure: applied {touched} setting(s)"));
        }
    }

    /// Apply a `SetSkillDirs` command authoritatively (issue #68): replace
    /// the daemon's extra skill dirs, refresh the shared wire path context,
    /// abort any in-flight turn (its context predates the new catalog), and
    /// hot-reload skills from disk through the harness's reload closure. The
    /// gRPC server applies an optimistic `path_context` update with the same
    /// dirs before enqueuing this command; this step makes it durable.
    async fn handle_set_skill_dirs(&mut self, dirs: Vec<String>, turn: &mut TurnState) {
        let dirs: Vec<PathBuf> = dirs.into_iter().map(PathBuf::from).collect();
        self.runtime.paths.set_extra_skill_dirs(dirs);
        // Keep the shared wire path context in sync with the authoritative
        // value (`GetPathContext` readers observe it immediately).
        self.runtime.path_context.write().unwrap().skills_dirs = self
            .runtime
            .paths
            .current_extra_skill_dirs()
            .into_iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        if turn.fut.is_some() {
            self.request_abort(turn);
        }
        match self.session.kernel.harness().reload_skills_from_disk().await {
            Ok(out) => self.system_line(format!(
                "set skill dirs: {} loaded, {} diagnostics",
                out.skills.len(),
                out.diagnostics.len()
            )),
            Err(e) => self.error_line(format!("set skill dirs: {e:#}")),
        }
    }

    async fn handle_switch_session(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim().to_string();
        if id.is_empty() {
            self.error_line("switch session: missing session id");
            return;
        }
        if id == self.session.id {
            self.system_line(format!("already on session {id}"));
            return;
        }
        match self.session.repository.contains(&id).await {
            Ok(true) => {}
            Ok(false) => {
                self.error_line(format!("switch session: no session matches id {id}"));
                return;
            }
            Err(e) => {
                self.error_line(format!("switch session: {e}"));
                return;
            }
        }
        if turn.fut.is_some() {
            self.request_abort(turn);
            if let Some(future) = turn.fut.take() {
                let _ = future.await;
            }
        }
        if let Err(e) = self.switch_session(id).await {
            self.error_line(format!("switch session: {e:#}"));
        }
    }

    fn trigger_web_rule_now(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim();
        if id.is_empty() {
            self.error_line("trigger: missing rule id");
            return;
        }
        let Some(rule) = self.automation.services.dynamic_triggers
            .list()
            .into_iter()
            .find(|rule| rule.id == id)
        else {
            self.error_line(format!("trigger: no dynamic trigger rule with id `{id}`"));
            return;
        };
        let display = format!(
            "trigger now {}: {}",
            feed::truncate_chars(&rule.id, 18),
            wire_preview(&rule.action)
        );
        if turn.fut.is_some() {
            self.queue_user_prompt(display, rule.action, Vec::new());
        } else {
            self.projection.feed.push_user(display);
            self.start_user_prompt_turn(rule.action, Vec::new(), turn);
        }
    }
}
