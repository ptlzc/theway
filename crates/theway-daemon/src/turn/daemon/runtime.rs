impl TurnHost {
    pub(crate) fn new(config: DaemonConfig) -> Self {
        // Scan claude-code-format file commands once at startup; `/reload`
        // rescans them (issue #37).
        let registry = Arc::new(config.registry);
        registry.set_file_commands(crate::file_commands::scan_file_commands(
            &config.cwd,
            &config.paths.home,
        ));
        let completer = SlashCompleter::from_commands(slash_commands(&registry));
        // Bind the application-owned reload slot after the initial session runtime exists.
        let reload_runtime = config.services.reload.install(ReloadRuntime::new(
            registry.clone(),
            config.cwd.clone(),
            config.trigger_executor.clone(),
            Arc::new(AtomicU64::new(0)),
        ));
        // Shared wire path context (issue #68): home/base/work_dir are fixed
        // at startup; `skills_dirs` starts as the CLI-supplied extras and is
        // the only part mutated at runtime (`SetSkillDirs`).
        let path_context = Arc::new(std::sync::RwLock::new(WirePathContext {
            home: config.paths.home.to_string_lossy().into_owned(),
            base: config.paths.base.to_string_lossy().into_owned(),
            work_dir: config.paths.work_dir.to_string_lossy().into_owned(),
            skills_dirs: config
                .paths
                .current_extra_skill_dirs()
                .into_iter()
                .map(|dir| dir.to_string_lossy().into_owned())
                .collect(),
        }));
        // Shared daemon configuration view (issue #72): seeded from the
        // startup-resolved settings (active model, skill dirs, trigger poll
        // interval, feed-history limit, enabled builtin skills). Issue #73: the
        // seed values come from the in-memory `StartupConfig` (defaults +
        // controller initial payload) — no local config file is read.
        // `Configure` commands merge into the view at runtime and the
        // transport servers serve it via `GetConfig`.
        let startup_state = config.harness.agent().state();
        let startup_model = startup_state.model.clone();
        let startup_thinking = startup_state
            .thinking_level
            .map(|level| level != theway_core::ThinkingLevel::Off);
        let startup_thinking_level = startup_state
            .thinking_level
            .map(|level| level.as_str().to_string());
        drop(startup_state);
        let daemon_config = Arc::new(std::sync::RwLock::new(WireDaemonConfig {
            provider: startup_model.as_ref().map(|model| model.provider.0.clone()),
            model: startup_model.as_ref().map(|model| model.id.clone()),
            base_url: startup_model
                .as_ref()
                .map(|model| model.base_url.clone())
                .filter(|url| !url.is_empty()),
            thinking: startup_thinking,
            thinking_level: startup_thinking_level,
            builtin_skills: config.startup.builtin_skills.clone(),
            skills: config
                .provisioned_skills
                .read()
                .unwrap()
                .iter()
                .map(|skill| theway_transport::wire::WireProvisionedSkill {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    content: skill.content.clone(),
                    file_path: skill.file_path.clone(),
                    source: match skill.source {
                        theway_core::SkillSource::Project => "project".to_string(),
                        _ => "user".to_string(),
                    },
                    disable_model_invocation: skill.disable_model_invocation,
                })
                .collect(),
            skills_dirs: config
                .paths
                .current_extra_skill_dirs()
                .into_iter()
                .map(|dir| dir.to_string_lossy().into_owned())
                .collect(),
            trigger_poll_secs: Some(config.startup.trigger_poll_secs),
            tui_max_feed_lines: config.startup.tui_max_feed_lines,
            tool_service_addr: None,
            storage_service_addr: config.startup.storage_service_addr.clone(),
            clear_fields: Vec::new(),
        }));
        let tool_ops: Arc<dyn ToolOps> = Arc::new(ForwardingToolOps::new(daemon_config.clone()));
        let mut kernel = ReplKernel::new(config.harness, config.trigger_executor, config.retry.clone());
        kernel.set_extension_host(config.extension_host);
        // Resume replay (issue #87): a rehydrated session's transcript has no
        // live `FeedUpdate`s left to drive the feed, so replay it into the
        // initial projection — the startup snapshot then carries the full
        // history to TUI/headless clients, capped at `tui_max_feed_lines`.
        let mut projection = FeedProjectionState {
            feed: Feed::new(),
            plain_lines_cache: theway_transport::feed::PlainLinesCache::new(100),
            block_versions: Vec::new(),
            dirty_blocks: BTreeSet::new(),
            latest_trigger_poll: None,
            latest_goal: None,
            thinking_summary: config.thinking_summary,
            thinking_burst: super::thinking_summary::ThinkingBurst::default(),
            control_plane_prompt: None,
            capabilities: config.capabilities,
        };
        crate::feed_replay::replay_transcript(
            &mut projection.feed,
            &kernel.harness().agent().state().messages,
            config.startup.tui_max_feed_lines,
        );
        Self {
            session: SessionRuntimeState {
                kernel,
                id: config.session_id,
                cwd: config.cwd.clone(),
                log_path: config.log_path,
                tool_count: config.tool_count,
                retry: config.retry.clone(),
                factory: config.session_factory,
                repository: config.session_repo,
                busy: false,
                queue: VecDeque::new(),
                cumulative_usage: WireContextUsage::default(),
                projection: FeedProjectionState::new(
                    projection.capabilities.clone(),
                    projection.thinking_summary.clone(),
                ),
                aborted: false,
            },
            sessions: SessionRegistry::new(),
            automation: AutomationRuntime {
                services: config.services,
                reload: reload_runtime,
                dag: config.dag_engine,
                subagents: config.subagent_registry,
            },
            runtime: RuntimeConfiguration {
                registry,
                completer,
                cwd: config.cwd,
                paths: config.paths,
                path_context,
                config: daemon_config,
                provisioned_skills: config.provisioned_skills,
                tool_ops,
                model_catalog: model_catalog(),
                feed_history_limit: config.startup.tui_max_feed_lines,
                latest: None,
                snapshot_tx: None,
                session_states: None,
            },
            projection,
            inputs: RuntimeEventInputs {
                feed_rx: Some(config.feed_rx),
                feed_tx: config.feed_tx,
                main_run_rx: Some(config.main_run_rx),
                control_plane_prompt_rx: config.control_plane_prompt_rx,
            },
        }
    }

    fn system_line(&mut self, text: impl AsRef<str>) {
        self.projection
            .feed
            .push_plain_untimed(text.as_ref(), Level::System);
    }

    fn error_line(&mut self, text: impl AsRef<str>) {
        self.projection
            .feed
            .push_error(text.as_ref(), None, false);
    }

    /// Build the public transport channels and wire the event planes
    /// ([`theway_transport::host::TransportHost::transport_endpoints`] implementation).
    pub(crate) fn transport_endpoints(&mut self) -> TransportEndpoints {
        let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(128);
        let initial_snapshot = self.wire_snapshot();
        let latest = Arc::new(Mutex::new(initial_snapshot.clone()));
        let session_states = Arc::new(Mutex::new(HashMap::from([(
            initial_snapshot.session_id.clone(),
            initial_snapshot,
        )])));
        self.runtime.session_states = Some(session_states.clone());
        let (event_tx, _) = broadcast::channel::<WireAgentEvent>(256);
        let (dag_event_tx, _) = broadcast::channel::<WireDagEvent>(256);
        let (core_dag_event_tx, _) = broadcast::channel::<DagEvent>(256);
        self.automation.dag
            .set_event_sender(Some(core_dag_event_tx.clone()));
        let agent_fwd = {
            let mut agent_rx = self.automation.subagents.subscribe();
            let agent_tx = event_tx.clone();
            let mut dag_rx = core_dag_event_tx.subscribe();
            let dag_tx = dag_event_tx.clone();
            tokio::spawn(async move {
                let agent_loop = async move {
                    loop {
                        match agent_rx.recv().await {
                            Ok(event) => {
                                let _ = agent_tx.send(agent_event(event));
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("SubagentJobEvent broadcast lagged by {n}, skipping");
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    tracing::debug!(
                        "SubagentJobEvent registry channel closed; forwarder task exiting"
                    );
                };
                let dag_loop = async move {
                    loop {
                        match dag_rx.recv().await {
                            Ok(event) => {
                                let _ = dag_tx.send(dag_event(event));
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("DagEvent broadcast lagged by {n}, skipping");
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    tracing::debug!("DagEvent channel closed; forwarder task exiting");
                };
                tokio::join!(agent_loop, dag_loop);
            })
            .abort_handle()
        };
        let session_graph_path = theway_contract::config::sessions_dir_for_cwd(&self.session.cwd)
            .join(theway_storage::session_graph::SESSION_GRAPH_DB_FILE);
        let session_ops: Arc<dyn theway_transport::transport::SessionOps> = Arc::new(
            crate::session_ops::AppSessionOps::with_session_graph(
                self.session.repository.clone(),
                self.automation.dag.clone(),
                self.session.cwd.display().to_string(),
                self.automation.services.session_execution.clone(),
                self.automation.subagents.clone(),
                session_graph_path,
            ),
        );
        let graph_ops: Arc<dyn theway_transport::transport::GraphOps> =
            Arc::new(CoreGraphOps::new(self.automation.dag.clone()));
        let tool_ops: Arc<dyn theway_transport::transport::ToolOps> = self.runtime.tool_ops.clone();
        let storage_ops: Arc<dyn theway_transport::transport::StorageOps> =
            std::sync::Arc::new(theway_transport::UnavailableStorageOps);
        let observability: Arc<
            dyn theway_transport::session_observability::SessionObservabilityOps,
        > = Arc::new(crate::session_observability::DaemonSessionObservability::new(
            session_ops.clone(),
            session_states.clone(),
            latest.clone(),
            self.session.repository.clone(),
        ));
        let external_ops: Arc<dyn theway_transport::ExternalProtocolOps> = Arc::new(
            crate::external_protocol_ops::DaemonExternalProtocolOps::new(
                command_tx.clone(),
                session_ops.clone(),
                observability,
                graph_ops.clone(),
                tool_ops.clone(),
                storage_ops.clone(),
                self.runtime.path_context.clone(),
                self.runtime.config.clone(),
            ),
        );
        TransportEndpoints {
            command_tx,
            command_rx,
            snapshot_tx,
            latest,
            session_states,
            events: event_tx,
            dag_events: dag_event_tx,
            completer: self.runtime.completer.clone(),
            job_ops: Arc::new(CoreJobOps::new(
                self.automation.subagents.clone(),
                self.automation.dag.clone(),
            )),
            graph_ops,
            session_ops,
            // Issue #68: the transport servers serve `GetPathContext` from
            // this handle and apply the `SetSkillDirs` optimistic update
            // against it; the event loop holds the authoritative copy.
            path_context: self.runtime.path_context.clone(),
            // The transport servers serve `GetConfig` from this authoritative
            // handle; only the event loop updates it after applying a patch.
            daemon_config: self.runtime.config.clone(),
            // Issue #76: file/process operations are forwarded to the
            // controller's ToolService endpoint through the shared config's
            // `tool_service_addr`.
            tool_ops,
            // Issue #84: runtime state externalization is wired as an RPC
            // contract first; the storage-backed implementation lands with
            // the controller-storage phase (#85/#86).
            storage_ops,
            external_ops,
            session_id: self.session.id.clone(),
            agent_fwd,
        }
    }

    /// Serialized transport event loop: drains the endpoint channels into the host
    /// and drives the selected transport server until shutdown.
    pub(crate) async fn run_transport_loop(
        mut self,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        mut server_task: tokio::task::JoinHandle<Result<()>>,
    ) -> Result<()> {
        let label = mode.label();
        let mut command_rx = endpoints.command_rx;
        self.runtime.latest = Some(endpoints.latest.clone());
        self.runtime.snapshot_tx = Some(endpoints.snapshot_tx.clone());
        let latest = endpoints.latest;
        let snapshot_tx = endpoints.snapshot_tx;

        let mut feed_rx = self.inputs.feed_rx.take().expect("feed_rx taken once");
        let mut main_run_rx = self.inputs.main_run_rx.take().expect("main_run_rx taken once");
        let mut control_plane_prompt_rx = self.inputs.control_plane_prompt_rx.take();
        let mut turn = TurnState::default();
        let mut parked_turns: FuturesUnordered<
            std::pin::Pin<
                Box<dyn std::future::Future<Output = (String, Result<Option<String>, theway_core::AgentRunError>)>>,
            >,
        > = FuturesUnordered::new();
        self.refresh_goal_state().await;
        self.publish_snapshot(&latest, &snapshot_tx, true).await;

        // Snapshot coalescing (issue #35): events mark the state dirty and a
        // 50ms tick flushes at most one snapshot per tick, so token floods
        // publish ~20fps instead of once per chunk. Command latency stays
        // within one tick.
        let mut dirty = false;
        let mut metadata_dirty = false;
        let mut publish_tick = tokio::time::interval(Duration::from_millis(50));
        publish_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        publish_tick.reset();

        loop {
            tokio::select! {
                biased;
                result = poll_turn(&mut turn.fut), if turn.fut.is_some() => {
                    self.finish_turn(&mut turn, result).await;
                    dirty = true;
                    metadata_dirty = true;
                }
                Some((session_id, result)) = parked_turns.next(), if !parked_turns.is_empty() => {
                    self.finish_parked_turn(&session_id, result, &mut parked_turns).await;
                    dirty = true;
                    metadata_dirty = true;
                }
                Some(command) = command_rx.recv() => {
                    self.handle_web_command(command, &mut turn).await;
                    self.start_parked_turns(&mut parked_turns);
                    dirty = true;
                    metadata_dirty = true;
                }
                Some((session_id, update)) = feed_rx.recv() => {
                    metadata_dirty |= self.apply_feed_update(&session_id, update);
                    while let Ok((session_id, update)) = feed_rx.try_recv() {
                        metadata_dirty |= self.apply_feed_update(&session_id, update);
                    }
                    self.start_parked_turns(&mut parked_turns);
                    dirty = true;
                }
                Some(trace_id) = main_run_rx.recv(), if turn.fut.is_none() => {
                    self.start_triggered_turn(trace_id, &mut turn);
                    dirty = true;
                    metadata_dirty = true;
                }
                Some(prompt) = async {
                    match control_plane_prompt_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                }, if control_plane_prompt_rx.is_some() => {
                    self.show_control_plane_prompt(prompt);
                    dirty = true;
                    metadata_dirty = true;
                }
                _ = publish_tick.tick(), if dirty => {
                    dirty = false;
                    self.publish_snapshot(&latest, &snapshot_tx, metadata_dirty).await;
                    self.publish_parked_snapshots(&snapshot_tx);
                    metadata_dirty = false;
                }
                _ = tokio::signal::ctrl_c() => {
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx, true).await;
                    }
                    break;
                }
                _ = sigterm_received() => {
                    self.system_line(format!("[{label}] received SIGTERM, shutting down"));
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx, true).await;
                    }
                    break;
                }
                server_result = &mut server_task => {
                    match server_result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => self.error_line(format!("{label} server: {e}")),
                        Err(e) => self.error_line(format!("{label} server task: {e}")),
                    }
                    break;
                }
            }
        }
        // Daemon shutdown: zeroize every memory-only provider credential before
        // the process exits.
        self.automation.services.session_execution.clear_all_credentials();
        Ok(())
    }
}

/// Resolves when the process receives `SIGTERM` (unix only). On other
/// platforms, and when the handler cannot be registered, it never resolves,
/// so the transport-loop select arm awaiting it stays inert there.
async fn sigterm_received() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        terminate.recv().await;
        return;
    }
    std::future::pending::<()>().await;
}
