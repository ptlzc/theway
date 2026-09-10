// `WireCommand` routing: every transport command is dispatched here to the
// `TurnHost` handler that owns its domain.
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    async fn handle_web_command(&mut self, command: WireCommand, turn: &mut TurnState) {
        match command {
            WireCommand::Submit {
                session_id,
                text,
                images,
                interrupt,
            } => {
                if session_id.is_empty() || session_id == self.session.id {
                    self.submit_web_text(text, images, interrupt, turn).await;
                } else {
                    self.submit_web_text_for_session(&session_id, text, images, interrupt)
                        .await;
                }
            }
            WireCommand::TriggerRuleNow { id } => self.trigger_web_rule_now(id, turn).await,
            WireCommand::Abort { session_id } => {
                if session_id.is_empty() || session_id == self.session.id {
                    self.request_abort(turn);
                } else if !self.sessions.contains(&session_id) {
                    self.error_line(format!(
                        "abort ignored: session {session_id} is not an active or registered session {}",
                        self.session.id
                    ));
                } else {
                    self.cancel_session(&session_id);
                }
            }
            WireCommand::ResolveControlPlane {
                session_id,
                approve,
            } => {
                let session_id = if session_id.is_empty() {
                    self.session.id.clone()
                } else {
                    session_id
                };
                let decision = if approve {
                    theway_core::ControlPlanePromptDecision::Allow
                } else {
                    theway_core::ControlPlanePromptDecision::Deny {
                        reason: Some("denied by user".into()),
                    }
                };
                self.resolve_control_plane_prompt_for_session(&session_id, decision);
            }
            WireCommand::SetModel {
                session_id,
                spec,
                response,
            } => {
                let active_session = session_id.is_empty() || session_id == self.session.id;
                let ok = if active_session {
                    self.set_model_from_spec(&spec).await
                } else {
                    self.set_model_for_session(&session_id, &spec).await
                };
                if ok && active_session {
                    // A model-less session can now run the messages it has been
                    // holding in its queue.
                    self.start_next_queued_turn(turn);
                }
                let _ = response.send(ok);
            }
            WireCommand::SetThinking {
                session_id,
                level,
                response,
            } => {
                let ok = if session_id.is_empty() || session_id == self.session.id {
                    self.set_thinking_level(&level).await
                } else {
                    self.set_thinking_for_session(&session_id, &level).await
                };
                let _ = response.send(ok);
            }
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
            WireCommand::SessionDeleted { id } => {
                self.handle_session_deleted(&id, turn).await;
            }
        }
    }
}
