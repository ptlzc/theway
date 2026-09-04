impl TurnHost {
    async fn submit_web_text(
        &mut self,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
        turn: &mut TurnState,
    ) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() && images.is_empty() {
            return;
        }
        let loaded_images = match load_web_prompt_images(&images) {
            Ok(images) => images,
            Err(e) => {
                self.error_line(format!("pasted image: {e}"));
                return;
            }
        };
        if !loaded_images.is_empty() && !self.current_model_accepts_images() {
            self.error_line(format!(
                "current model does not support image input; switch to a vision-capable model before sending {} image attachment(s)",
                loaded_images.len()
            ));
            return;
        }

        if trimmed.starts_with('/') && loaded_images.is_empty() {
            self.projection.feed.push_user(&trimmed);
            self.dispatch_web_slash(&trimmed, turn).await;
            return;
        }

        let expanded = if trimmed.is_empty() {
            String::new()
        } else {
            mentions::expand(&trimmed, &self.runtime.cwd).await.0
        };
        let prompt_text = commands::attach_skill_prompt(expanded, None);
        let display = prompt_display(&trimmed, loaded_images.len());
        if interrupt {
            self.request_abort(turn);
            self.session.queue.clear();
            self.system_line("interrupt: stopping current turn for new message");
            if turn.fut.is_some() {
                self.queue_user_prompt(display, prompt_text, loaded_images);
            } else {
                self.projection.feed.push_user(display);
                self.start_user_prompt_turn(prompt_text, loaded_images, turn);
            }
        } else if turn.fut.is_some() {
            // Issue #102: a busy tool-calling turn must see the new user
            // message on its NEXT LLM request, not after the whole turn
            // finishes. Inject into the core steering queue + interrupt the
            // in-flight LLM call (a no-op mid-tool, where the steering is
            // drained at the turn boundary anyway).
            self.interleave_user_message(display, prompt_text, loaded_images);
        } else {
            self.projection.feed.push_user(display);
            self.start_user_prompt_turn(prompt_text, loaded_images, turn);
        }
    }

    /// Issue #102: push a queued user message into the running turn's steering
    /// queue so the model sees it before its next LLM call, instead of waiting
    /// for the turn to finish. The message is also echoed into the feed now.
    fn interleave_user_message(
        &mut self,
        display: String,
        prompt_text: String,
        images: Vec<ImageContent>,
    ) {
        self.projection.feed.push_user(display);
        let message = interleaved_user_message(prompt_text, images);
        self.session.kernel.harness().enqueue_steering(message);
        self.session.kernel.harness().interrupt();
        self.system_line("interleaved new message into the running turn");
    }

    /// Route a message to a non-active session's own queue. The active session
    /// keeps its existing fast path in [`Self::submit_web_text`]; this method
    /// ensures a runtime exists for `session_id` and enqueues without requiring
    /// a global current-session switch first.
    async fn submit_web_text_for_session(
        &mut self,
        session_id: &str,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() && images.is_empty() {
            return;
        }
        let loaded_images = match load_web_prompt_images(&images) {
            Ok(images) => images,
            Err(e) => {
                self.error_line(format!("pasted image: {e}"));
                return;
            }
        };
        if self.ensure_session_runtime(session_id).await.is_err() {
            self.error_line(format!("send_message: no session runtime for {session_id}"));
            return;
        }
        // Slash commands addressed to a non-active session must run in that
        // session's own runtime/context (issue: `/collapse` typed after a
        // client-side `/resume` was being queued as a normal user prompt).
        if trimmed.starts_with('/') && loaded_images.is_empty() {
            self.dispatch_web_slash_for_session(session_id, &trimmed).await;
            return;
        }
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        if !loaded_images.is_empty() && !session.kernel.current_model_accepts_images() {
            return;
        }
        let display = prompt_display(&trimmed, loaded_images.len());
        let prompt_text = commands::attach_skill_prompt(trimmed, None);
        if interrupt {
            session.queue.clear();
        }
        if !interrupt && session.busy {
            // Issue #102: interleave into the running turn instead of waiting
            // for it to finish.
            session.projection.feed.push_user(display);
            let message = interleaved_user_message(prompt_text, loaded_images);
            session.kernel.harness().enqueue_steering(message);
            session.kernel.harness().interrupt();
            session
                .projection
                .feed
                .push_plain_untimed("interleaved new message into the running turn", Level::System);
        } else {
            session.queue.push_back(QueuedTurn::UserPrompt {
                display,
                prompt: prompt_text,
                images: loaded_images,
            });
        }
    }

    async fn dispatch_web_slash(&mut self, input: &str, turn: &mut TurnState) {
        let outcome = {
            let ctx = CommandCtx {
                harness: self.session.kernel.harness(),
                trigger_executor: self.session.kernel.trigger_executor(),
                session_id: &self.session.id,
                log_path: self.session.log_path.as_ref(),
                tool_count: self.session.tool_count,
                cwd: &self.runtime.cwd,
                inherit_slot: &self.runtime.inherit_slot,
            };
            commands::dispatch(input, &self.runtime.registry, &ctx).await
        };
        // Issue #100: a dispatched command may have created a child session
        // (collapse) and requested runtime-settings inheritance. Apply the
        // carried model + thinking level to the child now — the command layer
        // has no &mut TurnHost, so the host consumes the slot.
        let inherit = self.runtime.inherit_slot.lock().unwrap().take();
        if let Some(inherit) = inherit {
            let ok = self
                .set_model_for_session(&inherit.session_id, &inherit.model_spec)
                .await;
            if !ok {
                self.error_line(format!(
                    "inherit model '{}' for child session {} failed",
                    inherit.model_spec, inherit.session_id
                ));
            }
            if let Some(level) = inherit.thinking_level {
                self.set_thinking_for_session(&inherit.session_id, &level)
                    .await;
            }
        }
        match outcome {
            CommandOutcome::Quit => {
                self.system_line("daemon stays running; stop it with Ctrl-C / SIGTERM");
            }
            CommandOutcome::ClearScreen => {
                self.clear_feed();
            }
            CommandOutcome::Error(e) => self.error_line(e),
            CommandOutcome::AttachSkill { name } => {
                self.system_line(format!("skill `{name}` attached for the next prompt"));
            }
            CommandOutcome::RunAgentPrompt {
                prompt,
                error_context,
            } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::AgentPrompt {
                        display: input.to_string(),
                        prompt,
                        error_context,
                    });
                } else {
                    self.start_prompt_turn(prompt, error_context, turn);
                }
            }
            CommandOutcome::RunPromptTemplate { name, vars } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::PromptTemplate {
                        display: input.to_string(),
                        name,
                        vars,
                    });
                } else {
                    self.start_template_turn(name, vars, turn);
                }
            }
            CommandOutcome::RunCompaction { custom } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::Compaction {
                        display: input.to_string(),
                        custom,
                    });
                } else {
                    self.start_compaction_turn(custom, turn);
                }
            }
            CommandOutcome::WebRelay(_) => {
                self.system_line("web relay is a client feature; the daemon is already a server");
            }
            CommandOutcome::SessionImportActivation {
                session_path,
                trigger_ids,
                cron_ids,
            } => {
                self.system_line(format!(
                    "imported session {} has automation that was left disabled (imports always \
                     disable triggers/cron)",
                    session_path.display()
                ));
                // Actionable guidance, not a reference to a nonexistent flag: the daemon
                // has no `--activate-triggers` (that is a CLI subcommand flag), so list
                // the ids the source had enabled with the enable commands that do exist.
                const ID_PREVIEW: usize = 5;
                let list_ids = |ids: &[String], what: &str, enable_cmd: &str| {
                    let shown: Vec<&str> =
                        ids.iter().take(ID_PREVIEW).map(String::as_str).collect();
                    let mut line =
                        format!("{what} not enabled ({}): {}", ids.len(), shown.join(", "));
                    if ids.len() > ID_PREVIEW {
                        line.push_str(&format!(" … (+{} more)", ids.len() - ID_PREVIEW));
                    }
                    line.push_str(&format!(" — enable with `{enable_cmd} <id>`"));
                    line
                };
                if !trigger_ids.is_empty() {
                    self.system_line(list_ids(&trigger_ids, "triggers", "/triggers enable"));
                }
                if !cron_ids.is_empty() {
                    self.system_line(list_ids(&cron_ids, "cron jobs", "/cron enable"));
                }
            }
            CommandOutcome::LoginSecret {
                provider,
                recovery_command,
                ..
            } => {
                let command = recovery_command.unwrap_or_else(|| format!("/login {provider}"));
                self.error_line(format!(
                    "login is not implemented in the daemon; run `{command}` from a client"
                ));
            }
            CommandOutcome::OpenModelPicker => {
                let active = match self.session.kernel.harness().agent().state().model.clone() {
                    Some(m) => format!("active model: {}:{}", m.provider.0, m.id),
                    None => "(no model active)".into(),
                };
                self.system_line(format!("{active} — switch via SetModel (web/grpc client)"));
            }
            CommandOutcome::Handled => {}
        }
        if input.trim_start().starts_with("/goal") {
            self.refresh_goal_state().await;
        }
    }

    /// Dispatch a slash command against a parked (non-active) session's own
    /// harness/context. Command output is rerouted to that session's feed.
    async fn dispatch_web_slash_for_session(&mut self, session_id: &str, input: &str) {
        let output = commands::CommandOutput::new({
            let tx = self.inputs.feed_tx.clone();
            let session_id = session_id.to_string();
            move |line| {
                let _ = tx.send((
                    session_id.clone(),
                    FeedUpdate::Plain {
                        text: line,
                        level: Level::Output,
                    },
                ));
            }
        });
        let outcome = {
            let Some(session) = self.sessions.get_mut(session_id) else {
                return;
            };
            let ctx = CommandCtx {
                harness: session.kernel.harness(),
                trigger_executor: session.kernel.trigger_executor(),
                session_id: &session.id,
                log_path: session.log_path.as_ref(),
                tool_count: session.tool_count,
                cwd: &session.cwd,
                inherit_slot: &self.runtime.inherit_slot,
            };
            commands::dispatch_with_output(input, &self.runtime.registry, &ctx, output).await
        };
        self.handle_parked_command_outcome(session_id, input, outcome);
    }

    fn handle_parked_command_outcome(
        &mut self,
        session_id: &str,
        input: &str,
        outcome: CommandOutcome,
    ) {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        // Run-style outcomes push the display in `start_parked_turn`; all
        // other outcomes mirror the active slash path and show the command
        // line immediately.
        let queued_outcome = matches!(
            outcome,
            CommandOutcome::RunAgentPrompt { .. }
                | CommandOutcome::RunPromptTemplate { .. }
                | CommandOutcome::RunCompaction { .. }
        );
        if !queued_outcome {
            session.projection.feed.push_user(input.to_string());
        }
        match outcome {
            CommandOutcome::Quit => {
                session.projection.feed.push_plain_untimed(
                    "daemon stays running; stop it with Ctrl-C / SIGTERM".to_string(),
                    Level::System,
                );
            }
            CommandOutcome::ClearScreen => {
                session.projection.feed.clear();
            }
            CommandOutcome::Error(e) => {
                session.projection.feed.push_error(e, None, false);
            }
            CommandOutcome::AttachSkill { name } => {
                session.projection.feed.push_plain_untimed(
                    format!("skill `{name}` attached for the next prompt"),
                    Level::System,
                );
            }
            CommandOutcome::RunAgentPrompt {
                prompt,
                error_context,
            } => {
                session.queue.push_back(QueuedTurn::AgentPrompt {
                    display: input.to_string(),
                    prompt,
                    error_context,
                });
            }
            CommandOutcome::RunPromptTemplate { name, vars } => {
                session.queue.push_back(QueuedTurn::PromptTemplate {
                    display: input.to_string(),
                    name,
                    vars,
                });
            }
            CommandOutcome::RunCompaction { custom } => {
                session.queue.push_back(QueuedTurn::Compaction {
                    display: input.to_string(),
                    custom,
                });
            }
            CommandOutcome::WebRelay(_) => {
                session.projection.feed.push_plain_untimed(
                    "web relay is a client feature; the daemon is already a server".to_string(),
                    Level::System,
                );
            }
            CommandOutcome::SessionImportActivation {
                session_path,
                trigger_ids,
                cron_ids,
            } => {
                session.projection.feed.push_plain_untimed(
                    format!(
                        "imported session {} has automation that was left disabled (imports always \
                         disable triggers/cron)",
                        session_path.display()
                    ),
                    Level::System,
                );
                const ID_PREVIEW: usize = 5;
                let list_ids = |ids: &[String], what: &str, enable_cmd: &str| {
                    let shown: Vec<&str> =
                        ids.iter().take(ID_PREVIEW).map(String::as_str).collect();
                    let mut line =
                        format!("{what} not enabled ({}): {}", ids.len(), shown.join(", "));
                    if ids.len() > ID_PREVIEW {
                        line.push_str(&format!(" … (+{} more)", ids.len() - ID_PREVIEW));
                    }
                    line.push_str(&format!(" — enable with `{enable_cmd} <id>`"));
                    line
                };
                if !trigger_ids.is_empty() {
                    session.projection.feed.push_plain_untimed(
                        list_ids(&trigger_ids, "triggers", "/triggers enable"),
                        Level::System,
                    );
                }
                if !cron_ids.is_empty() {
                    session.projection.feed.push_plain_untimed(
                        list_ids(&cron_ids, "cron jobs", "/cron enable"),
                        Level::System,
                    );
                }
            }
            CommandOutcome::LoginSecret {
                provider,
                recovery_command,
                ..
            } => {
                let command =
                    recovery_command.unwrap_or_else(|| format!("/login {provider}"));
                session.projection.feed.push_plain_untimed(
                    format!(
                        "login is not implemented in the daemon; run `{command}` from a client"
                    ),
                    Level::System,
                );
            }
            CommandOutcome::OpenModelPicker => {
                let active = match session.kernel.harness().agent().state().model.clone() {
                    Some(m) => format!("active model: {}:{}", m.provider.0, m.id),
                    None => "(no model active)".into(),
                };
                session.projection.feed.push_plain_untimed(
                    format!("{active} — switch via SetModel (web/grpc client)"),
                    Level::System,
                );
            }
            CommandOutcome::Handled => {}
        }
    }
}
