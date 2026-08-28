impl GrpcClient {
    /// Connect to `host:port` (no scheme). Fails fast when nothing listens.
    pub async fn connect(addr: &str) -> Result<Self> {
        let channel = Channel::from_shared(format!("http://{addr}"))
            .with_context(|| format!("connect to daemon at {addr}"))?
            .connect()
            .await
            .with_context(|| format!("connect to daemon at {addr}"))?;
        Ok(Self {
            session: SessionServiceClient::new(channel.clone()),
            command: CommandServiceClient::new(channel.clone()),
            extensions: ExtensionServiceClient::new(channel.clone()),
            graph: GraphEngineServiceClient::new(channel.clone()),
            events: EventServiceClient::new(channel.clone()),
            settings: SettingsServiceClient::new(channel.clone()),
            storage: StorageServiceClient::new(channel.clone()),
            tools: ToolServiceClient::new(channel),
            addr: addr.to_string(),
        })
    }

    /// Address this client is connected to (`host:port`).
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Full structured state for the daemon's current session (health probe).
    pub async fn get_state(&mut self) -> Result<SessionState> {
        self.get_state_for_session("").await
    }

    /// Full structured state for an explicit session.
    #[allow(deprecated)]
    pub async fn get_state_for_session(&mut self, session_id: &str) -> Result<SessionState> {
        let state = self
            .session
            .get_state(SessionStateRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_state: {e}"))?
            .into_inner();
        Ok(state)
    }

    /// Full nested session snapshot for the daemon's current session.
    pub async fn get_snapshot(&mut self) -> Result<SessionSnapshot> {
        self.get_snapshot_for_session("").await
    }

    /// Full nested session snapshot for an explicit session.
    pub async fn get_snapshot_for_session(&mut self, session_id: &str) -> Result<SessionSnapshot> {
        let snapshot = self
            .session
            .get_snapshot(SessionStateRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_snapshot: {e}"))?
            .into_inner();
        Ok(snapshot)
    }

    /// Session history as a snapshot-shaped transcript for an explicit session.
    pub async fn get_history(&mut self, session_id: &str) -> Result<SessionSnapshot> {
        let snapshot = self
            .session
            .get_history(SessionStateRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_history: {e}"))?
            .into_inner();
        Ok(snapshot)
    }

    /// Collapse a session into a graph node (session-snapshot-collapse).
    pub async fn collapse_session(
        &mut self,
        request: CollapseSessionRequest,
    ) -> Result<CollapseSessionResponse> {
        let response = self
            .session
            .collapse_session(request)
            .await
            .map_err(|e| anyhow::anyhow!("collapse_session: {e}"))?
            .into_inner();
        Ok(response)
    }

    /// Open the snapshot/event frame stream for all sessions.
    pub async fn stream_events(&mut self) -> Result<Streaming<StreamFrame>> {
        self.stream_events_for_session(None).await
    }

    /// Open the snapshot/event frame stream, optionally filtered to one session.
    pub async fn stream_events_for_session(
        &mut self,
        session_id: Option<&str>,
    ) -> Result<Streaming<StreamFrame>> {
        let response = self
            .events
            .stream_events(StreamEventsRequest {
                session_id: session_id.map(str::to_string),
            })
            .await
            .map_err(|e| anyhow::anyhow!("stream_events: {e}"))?;
        Ok(response.into_inner())
    }

    /// Submit a message to the daemon's current session. `interrupt` = stop the
    /// current turn and run now (INTERRUPT), otherwise queue after the current
    /// turn (QUEUE).
    pub async fn send_message(
        &mut self,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        self.send_message_to_session(None, text, images, interrupt)
            .await
    }

    /// Submit a message to an explicit session (`None` = current/default).
    pub async fn send_message_to_session(
        &mut self,
        session_id: Option<&str>,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        let accepted = self
            .command
            .send_message(SendMessageRequest {
                text,
                images: images
                    .into_iter()
                    .map(|image| proto::Image {
                        data: image.data,
                        name: image.name,
                    })
                    .collect(),
                mode: if interrupt {
                    theway_grpc::MessageMode::Interrupt
                } else {
                    theway_grpc::MessageMode::Queue
                }
                .into(),
                session_id: session_id.map(str::to_string),
            })
            .await
            .map_err(|e| anyhow::anyhow!("send_message: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Switch the daemon's active model for the current session.
    pub async fn set_model(&mut self, spec: &str) -> Result<bool> {
        self.set_model_for_session("", spec).await
    }

    /// Switch the daemon's active model for an explicit session.
    pub async fn set_model_for_session(&mut self, session_id: &str, spec: &str) -> Result<bool> {
        let accepted = self
            .command
            .set_model(SetModelRequest {
                session_id: session_id.to_string(),
                spec: spec.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("set_model: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Set the daemon's active thinking level for the current session.
    pub async fn set_thinking(&mut self, level: &str) -> Result<bool> {
        self.set_thinking_for_session("", level).await
    }

    /// Set the daemon's active thinking level for an explicit session.
    pub async fn set_thinking_for_session(
        &mut self,
        session_id: &str,
        level: &str,
    ) -> Result<bool> {
        let accepted = self
            .command
            .set_thinking(SetThinkingRequest {
                session_id: session_id.to_string(),
                level: level.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("set_thinking: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Stop the in-flight turn for the current session.
    pub async fn cancel(&mut self) -> Result<bool> {
        self.cancel_session("").await
    }

    /// Stop the in-flight turn for an explicit session.
    pub async fn cancel_session(&mut self, session_id: &str) -> Result<bool> {
        let accepted = self
            .command
            .cancel(CancelRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("cancel: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Resolve a pending control-plane prompt for the current session.
    pub async fn approve(&mut self, approve: bool) -> Result<bool> {
        self.approve_for_session("", approve).await
    }

    /// Resolve a pending control-plane prompt for an explicit session.
    pub async fn approve_for_session(&mut self, session_id: &str, approve: bool) -> Result<bool> {
        let accepted = self
            .command
            .approve(ApproveRequest {
                session_id: session_id.to_string(),
                approve,
            })
            .await
            .map_err(|e| anyhow::anyhow!("approve: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Current client-neutral runtime-extension catalog and diagnostics.
    pub async fn get_extensions(&mut self) -> Result<WireExtensionSnapshot> {
        let response = self
            .extensions
            .get_extensions(Empty {})
            .await
            .map_err(|error| anyhow::anyhow!("get_extensions: {error}"))?
            .into_inner();
        Ok(crate::proto::extension_snapshot_wire(Some(&response)))
    }

    /// Invoke a registered extension command without requiring a TUI.
    pub async fn invoke_extension_command(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
        has_interactive_client: bool,
    ) -> Result<WireExtensionCommandOutcome> {
        let response = self
            .extensions
            .invoke_command(InvokeExtensionCommandRequest {
                name: name.to_string(),
                arguments_json: arguments.to_string(),
                has_interactive_client,
            })
            .await
            .map_err(|error| anyhow::anyhow!("invoke_extension_command: {error}"))?
            .into_inner();
        Ok(WireExtensionCommandOutcome {
            status: response.status,
            code: response.code,
            message: response.message,
            data: response
                .data_json
                .map(|data| serde_json::from_str(&data))
                .transpose()
                .context("decode extension command data")?,
        })
    }

    /// Re-discover runtime extensions. `pending` is applied at the next
    /// quiescent run/tool settlement boundary.
    pub async fn reload_extensions(
        &mut self,
        cancel_active: bool,
    ) -> Result<WireExtensionReloadResult> {
        let response = self
            .extensions
            .reload(ReloadExtensionsRequest { cancel_active })
            .await
            .map_err(|error| anyhow::anyhow!("reload_extensions: {error}"))?
            .into_inner();
        Ok(WireExtensionReloadResult {
            status: response.status,
            revision: response.revision,
        })
    }

    /// Persist a project or exact-package trust decision and request reload.
    pub async fn decide_extension_trust(
        &mut self,
        request: WireExtensionTrustRequest,
    ) -> Result<WireExtensionTrustResult> {
        let response = self
            .extensions
            .decide_trust(DecideExtensionTrustRequest {
                subject: request.subject,
                extension_id: request.extension_id,
                decision: request.decision,
                granted_permissions: request.granted_permissions,
            })
            .await
            .map_err(|error| anyhow::anyhow!("decide_extension_trust: {error}"))?
            .into_inner();
        let reload = response
            .reload
            .context("trust response omitted reload result")?;
        Ok(WireExtensionTrustResult {
            accepted: response.accepted,
            reload: WireExtensionReloadResult {
                status: reload.status,
                revision: reload.revision,
            },
        })
    }

    /// List sessions (oldest → newest) plus the daemon's current session id.
    pub async fn list_sessions(&mut self) -> Result<(Vec<SessionSummary>, String)> {
        let response = self
            .session
            .list_sessions(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("list_sessions: {e}"))?
            .into_inner();
        let sessions = response
            .sessions
            .iter()
            .map(crate::proto::session_summary_from_proto)
            .collect();
        Ok((sessions, response.current_session_id))
    }

    /// Create a session (becoming current flows through the daemon's event loop).
    pub async fn create_session(&mut self, name: Option<String>) -> Result<SessionSummary> {
        self.create_session_with_metadata(None, name, std::collections::HashMap::new())
            .await
    }

    /// Create a session with an optional custom id and initial metadata.
    pub async fn create_session_with_metadata(
        &mut self,
        session_id: Option<&str>,
        name: Option<String>,
        metadata: std::collections::HashMap<String, String>,
    ) -> Result<SessionSummary> {
        let response = self
            .session
            .create_session(CreateSessionRequest {
                name,
                session_id: session_id.map(str::to_string),
                metadata,
            })
            .await
            .map_err(|e| anyhow::anyhow!("create_session: {e}"))?
            .into_inner();
        let session = response
            .session
            .context("create_session returned no session summary")?;
        Ok(crate::proto::session_summary_from_proto(&session))
    }

    /// Update arbitrary session metadata KV.
    pub async fn update_session_metadata(
        &mut self,
        session_id: &str,
        metadata: std::collections::HashMap<String, String>,
    ) -> Result<bool> {
        let accepted = self
            .session
            .update_session_metadata(UpdateSessionMetadataRequest {
                session_id: session_id.to_string(),
                metadata,
            })
            .await
            .map_err(|e| anyhow::anyhow!("update_session_metadata: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Rename a session (full id or unique prefix).
    pub async fn rename_session(&mut self, id: &str, name: &str) -> Result<bool> {
        let accepted = self
            .session
            .rename_session(RenameSessionRequest {
                session_id: id.to_string(),
                name: name.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("rename_session: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Delete a session. `Ok(non_empty)` = refused, these run ids still running;
    /// `Ok(empty)` = deleted.
    pub async fn delete_session(&mut self, id: &str) -> Result<Vec<String>> {
        let response = self
            .session
            .delete_session(DeleteSessionRequest {
                session_id: id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("delete_session: {e}"))?;
        Ok(response.into_inner().running_run_ids)
    }

    // ── session activation and credentials (issue #26) ────────────────

    /// Activate or resume a client-bound session atomically. The daemon applies
    /// the requested runtime before replying.
    pub async fn activate_session(
        &mut self,
        request: proto::ActivateSessionRequest,
    ) -> Result<proto::ActivateSessionResponse> {
        let response = self
            .session
            .activate_session(request)
            .await
            .map_err(|e| anyhow::anyhow!("activate_session: {e}"))?;
        Ok(response.into_inner())
    }

    /// Install a memory-only provider credential for a session. Secrets are
    /// never persisted or echoed.
    pub async fn set_credential(
        &mut self,
        session_id: &str,
        provider: &str,
        secret: Vec<u8>,
    ) -> Result<bool> {
        let accepted = self
            .session
            .set_credential(proto::SetCredentialRequest {
                session_id: session_id.to_string(),
                provider: provider.to_string(),
                secret,
            })
            .await
            .map_err(|e| anyhow::anyhow!("set_credential: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Clear one provider credential, or all credentials for the session when
    /// `provider` is `None`.
    pub async fn clear_credential(
        &mut self,
        session_id: &str,
        provider: Option<&str>,
    ) -> Result<bool> {
        let accepted = self
            .session
            .clear_credential(proto::ClearCredentialRequest {
                session_id: session_id.to_string(),
                provider: provider.map(String::from),
            })
            .await
            .map_err(|e| anyhow::anyhow!("clear_credential: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    // ── path context (issue #68) ───────────────────────────────────────

    /// Daemon path context: home / base / work_dir plus the current skill
    /// search directories.
    pub async fn get_path_context(&mut self) -> Result<WirePathContext> {
        let response = self
            .session
            .get_path_context(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("get_path_context: {e}"))?
            .into_inner();
        Ok(crate::proto::wire_path_context_from_proto(&response))
    }

    /// Replace the extra skill directories dynamically. `Ok(true)` = the
    /// daemon queued the command; the event loop applies it authoritatively
    /// (hot-reload).
    pub async fn set_skill_dirs(&mut self, dirs: &[String]) -> Result<bool> {
        let accepted = self
            .session
            .set_skill_dirs(SetSkillDirsRequest {
                dirs: dirs.to_vec(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("set_skill_dirs: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    // ── settings / config (issue #72) ─────────────────────────────────

    /// Current daemon configuration view (fields the daemon knows about).
    pub async fn get_config(&mut self) -> Result<WireDaemonConfig> {
        let response = self
            .settings
            .get_config(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("get_config: {e}"))?
            .into_inner();
        Ok(crate::proto::daemon_config_from_proto(&response))
    }

    /// Push a partial configuration update. `Ok(true)` = the daemon queued
    /// the command; the serialized event loop applies it authoritatively.
    pub async fn set_config(&mut self, config: &WireDaemonConfig) -> Result<bool> {
        let accepted = self
            .settings
            .set_config(crate::proto::daemon_config_to_proto(config))
            .await
            .map_err(|e| anyhow::anyhow!("set_config: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Same operation as [`set_config`](Self::set_config), on the `Configure`
    /// method (kept so JSON-RPC / WS / MCP clients can align on one verb).
    pub async fn configure(&mut self, config: &WireDaemonConfig) -> Result<bool> {
        let accepted = self
            .settings
            .configure(crate::proto::daemon_config_to_proto(config))
            .await
            .map_err(|e| anyhow::anyhow!("configure: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }
}
