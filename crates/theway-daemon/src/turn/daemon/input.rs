/// The canonical record for a prompt a slash command synthesised.
///
/// A skill envelope (`attach_skill_prompt(text, Some(name))`) carries a human's own text plus
/// the injected skill preamble, so it is recorded as a user-authored round with one injected
/// part. Every other synthesised prompt (goal, trigger, file command, host-injected text) is
/// recorded as host-authored (`InputSource::Host`), so its turn renders with a provenance
/// marker instead of masquerading as a user message. The model-facing prompt the caller holds
/// is never rewritten.
fn command_prompt_record(prompt: &str) -> UserInput {
    let Some((skill_name, text)) = theway_transport::commands::split_skill_prompt(prompt) else {
        return UserInput {
            text: prompt.to_string(),
            parts: Vec::new(),
            source: InputSource::Host,
            source_ref: None,
        };
    };
    let preamble = theway_transport::commands::skill_prompt_preamble(&skill_name);
    let part = InputInjectedPart {
        source: "skill".to_string(),
        name: Some(skill_name),
        text: preamble,
    };
    crate::attachments::PromptAdmission::with_injected(
        UserInput {
            text,
            parts: Vec::new(),
            source: InputSource::User,
            source_ref: None,
        },
        part,
    )
}

impl TurnHost {
    /// Admission for one round of input against a session cwd: mentions resolve and attachment
    /// bytes land in the process-wide content-addressed library.
    fn prompt_admission(&self, cwd: PathBuf) -> crate::attachments::PromptAdmission {
        crate::attachments::PromptAdmission::new(self.automation.services.attachments.clone(), cwd)
    }

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
        let loaded_images = match prompt_images(&images) {
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

        // Admission parses the mentions once and stores every attachment byte before the
        // record exists; the record then travels with the prompt into the feed, the queue,
        // and the translation log.
        let record = match self
            .prompt_admission(self.runtime.cwd.clone())
            .admit(&trimmed, &images, InputSource::User, None)
            .await
        {
            Ok(record) => record,
            Err(e) => {
                self.error_line(format!("pasted image: {e}"));
                return;
            }
        };

        // Two purposes, one parse each: admission already produced the record's `File` parts and
        // stored their bytes, and this expansion is the model's copy of the same files — the
        // client no longer pre-expands, so the file body appears exactly once in the prompt.
        let expanded = if trimmed.is_empty() {
            String::new()
        } else {
            mentions::expand(&trimmed, &self.runtime.cwd).await.0
        };
        let prompt_text = commands::attach_skill_prompt(expanded, None);
        let display = trimmed;
        if interrupt {
            self.request_abort(turn);
            self.session.queue.clear();
            self.system_line("interrupt: stopping current turn for new message");
            if turn.fut.is_some() {
                self.queue_user_prompt_with_input(display, prompt_text, loaded_images, Some(record))
                    .await;
            } else if self.session.kernel.has_model() {
                push_user_record_blocks(&mut self.projection.feed, Some(&record), &display);
                self.start_user_prompt_turn_with_input(
                    prompt_text,
                    loaded_images,
                    Some(record),
                    turn,
                );
            } else {
                push_user_record_blocks(&mut self.projection.feed, Some(&record), &display);
                self.queue_user_prompt_with_input(display, prompt_text, loaded_images, Some(record))
                    .await;
                self.system_line("no model selected — queued until a model is set");
            }
        } else if !self.session.kernel.has_model() {
            // Do not start a turn that is guaranteed to fail inside the LLM
            // call. Keep the message queued; SetModel/Configure start it once
            // a model exists.
            push_user_record_blocks(&mut self.projection.feed, Some(&record), &display);
            self.queue_user_prompt_with_input(display, prompt_text, loaded_images, Some(record))
                .await;
            self.system_line("no model selected — queued until a model is set");
        } else if turn.fut.is_some() {
            // Issue #102: a busy tool-calling turn must see the new user
            // message on its NEXT LLM request, not after the whole turn
            // finishes. Inject into the core steering queue + interrupt the
            // in-flight LLM call (a no-op mid-tool, where the steering is
            // drained at the turn boundary anyway).
            self.interleave_user_message(display, prompt_text, loaded_images, record);
        } else {
            push_user_record_blocks(&mut self.projection.feed, Some(&record), &display);
            self.start_user_prompt_turn_with_input(
                prompt_text,
                loaded_images,
                Some(record),
                turn,
            );
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
        input: UserInput,
    ) {
        push_user_record_blocks(&mut self.projection.feed, Some(&input), &display);
        let message = interleaved_user_message(prompt_text, images);
        // The record enters the steering queue ahead of the message: both are appended to the
        // log in drain order, so the record describes the message that follows it. A record
        // that cannot be written must not leave its message behind unrecorded.
        let recorded = self.session.kernel.harness().enqueue_steering_input(&input);
        if let Err(error) = recorded {
            self.error_line(format!("steering record: {error}"));
            return;
        }
        let harness = self.session.kernel.harness();
        harness.enqueue_steering(message);
        harness.interrupt();
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
        let loaded_images = match prompt_images(&images) {
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
        // An id that is neither the active session nor a parked runtime owns no cwd and no
        // queue; keep the silent no-op the registry lookup produced.
        let Some(cwd) = self
            .sessions
            .get(session_id)
            .map(|session| session.cwd.clone())
        else {
            return;
        };
        let record = match self
            .prompt_admission(cwd.clone())
            .admit(&trimmed, &images, InputSource::User, None)
            .await
        {
            Ok(record) => record,
            Err(e) => {
                self.error_line(format!("pasted image: {e}"));
                return;
            }
        };
        // Slash commands addressed to a non-active session must run in that
        // session's own runtime/context (issue: `/collapse` typed after a
        // client-side `/resume` was being queued as a normal user prompt).
        if trimmed.starts_with('/') && loaded_images.is_empty() {
            self.dispatch_web_slash_for_session(session_id, &trimmed).await;
            return;
        }
        // The model's copy of the mentioned files is built here: the client submits the raw
        // text, so this expansion (not the record) is what puts the file body in the prompt.
        let expanded = if trimmed.is_empty() {
            String::new()
        } else {
            mentions::expand(&trimmed, &cwd).await.0
        };
        let Some(session) = self.sessions.get_mut(session_id) else {
            return;
        };
        if !loaded_images.is_empty() && !session.kernel.current_model_accepts_images() {
            return;
        }
        let display = trimmed;
        let prompt_text = commands::attach_skill_prompt(expanded, None);
        if interrupt {
            session.queue.clear();
        }
        if !session.kernel.has_model() {
            // Keep model-less sessions from consuming messages they cannot run.
            // Persist the prompt first (materializes the lazy session db), then
            // hold the job until SetModel lands; the transport loop re-checks
            // parked queues after every command.
            let persisted = match session
                .kernel
                .harness()
                .record_user_input_prompt(
                    prompt_text.clone(),
                    loaded_images.clone(),
                    Some(record.clone()),
                )
                .await
            {
                Ok(()) => true,
                Err(error) => {
                    session
                        .projection
                        .feed
                        .push_error(format!("persist queued message: {error}"), None, false);
                    false
                }
            };
            push_user_record_blocks(&mut session.projection.feed, Some(&record), &display);
            session.queue.push_back(QueuedTurn::UserPrompt {
                display,
                prompt: prompt_text,
                images: loaded_images,
                input: Some(record),
                persisted,
            });
            session.projection.feed.push_plain_untimed(
                "no model selected — queued until a model is set",
                Level::System,
            );
            return;
        }
        if !interrupt && session.busy {
            // Issue #102: interleave into the running turn instead of waiting
            // for it to finish.
            push_user_record_blocks(&mut session.projection.feed, Some(&record), &display);
            let message = interleaved_user_message(prompt_text, loaded_images);
            // Record before message: the steering queue drains into the log in order, and a
            // record that cannot be written must not leave its message behind unrecorded.
            let recorded = session.kernel.harness().enqueue_steering_input(&record);
            if let Err(error) = recorded {
                session
                    .projection
                    .feed
                    .push_error(format!("steering record: {error}"), None, false);
                return;
            }
            let harness = session.kernel.harness();
            harness.enqueue_steering(message);
            harness.interrupt();
            session
                .projection
                .feed
                .push_plain_untimed("interleaved new message into the running turn", Level::System);
        } else {
            let persisted = match session
                .kernel
                .harness()
                .record_user_input_prompt(
                    prompt_text.clone(),
                    loaded_images.clone(),
                    Some(record.clone()),
                )
                .await
            {
                Ok(()) => true,
                Err(error) => {
                    session
                        .projection
                        .feed
                        .push_error(format!("persist queued message: {error}"), None, false);
                    false
                }
            };
            session.queue.push_back(QueuedTurn::UserPrompt {
                display,
                prompt: prompt_text,
                images: loaded_images,
                input: Some(record),
                persisted,
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
                // session-scoped-mcp: `/reload` reconnects this session's own
                // slot when an overlay is installed, so the session servers
                // stay in the set instead of being replaced by the daemon's.
                mcp_provision: Some(
                    self.session
                        .mcp_overlay
                        .as_ref()
                        .map_or(&self.runtime.mcp_provision, |overlay| &overlay.slot),
                ),
                auth_base: Some(&self.runtime.paths.base),
                collapse_unload_slot: &self.runtime.collapse_unload_slot,
            };
            commands::dispatch(input, &self.runtime.registry, &ctx).await
        };
        // Issue #100: a dispatched command may have created a child session
        // (collapse) and requested runtime-settings inheritance. Apply the
        // carried model + thinking level to the child now — the command layer
        // has no &mut TurnHost, so the host consumes the slot.
        let inherit = lock_mutex(&self.runtime.inherit_slot).take();
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
        // Collapse unload: release the collapsed source session's runtime
        // from memory (the command layer has no &mut TurnHost, so the host
        // consumes the slot).
        let unload = lock_mutex(&self.runtime.collapse_unload_slot).take();
        if let Some(unload) = unload {
            self.handle_collapse_unload(unload, turn).await;
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
                let record = command_prompt_record(&prompt);
                let display = input.to_string();
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::AgentPrompt {
                        display,
                        prompt,
                        error_context,
                        input: Some(record),
                    });
                } else {
                    self.start_prompt_turn(prompt, error_context, Some(record), turn);
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
        // Slash commands like `/model` can assign the first model to a
        // model-less session; run any queued message that was waiting for one.
        self.start_next_queued_turn(turn);
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
                mcp_provision: Some(
                    session
                        .mcp_overlay
                        .as_ref()
                        .map_or(&self.runtime.mcp_provision, |overlay| &overlay.slot),
                ),
                auth_base: Some(&self.runtime.paths.base),
                collapse_unload_slot: &self.runtime.collapse_unload_slot,
            };
            commands::dispatch_with_output(input, &self.runtime.registry, &ctx, output).await
        };
        // Issue #100: consume the inheritance slot here as well — a parked
        // collapse writes it too, and a stale slot must never leak into a
        // later active-session dispatch.
        let inherit = lock_mutex(&self.runtime.inherit_slot).take();
        if let Some(inherit) = inherit {
            let _ = self
                .set_model_for_session(&inherit.session_id, &inherit.model_spec)
                .await;
            if let Some(level) = inherit.thinking_level {
                let _ = self.set_thinking_for_session(&inherit.session_id, &level).await;
            }
        }
        // Collapse unload: a parked session collapsing itself is dropped from
        // the registry outright; the host consumes the slot here too.
        let unload = lock_mutex(&self.runtime.collapse_unload_slot).take();
        if let Some(unload) = unload {
            let mut turn = TurnState::default();
            self.handle_collapse_unload(unload, &mut turn).await;
        }
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
                let record = command_prompt_record(&prompt);
                session.queue.push_back(QueuedTurn::AgentPrompt {
                    display: input.to_string(),
                    prompt,
                    error_context,
                    input: Some(record),
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
