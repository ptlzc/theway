impl TurnHost {
    pub fn new(config: DaemonConfig) -> Self {
        // Scan claude-code-format file commands once at startup; `/reload`
        // rescans them (issue #37).
        let registry = Arc::new(config.registry);
        registry.set_file_commands(crate::file_commands::scan_file_commands(
            &config.cwd,
            &config.home,
        ));
        let completer = SlashCompleter::from_commands(slash_commands(&registry));
        // Install the process-level reload runtime (issue #50): the `reload`
        // tool reaches the registry / cwd / trigger executor at execute time
        // and bumps the revision this host publishes in sidebar snapshots.
        let reload_runtime = reload::install_runtime(ReloadRuntime {
            registry: registry.clone(),
            cwd: config.cwd.clone(),
            trigger_executor: config.trigger_executor.clone(),
            revision: Arc::new(AtomicU64::new(0)),
        });
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
        // interval, TUI scrollback, enabled builtin skills). Issue #73: the
        // seed values come from the in-memory `StartupConfig` (defaults +
        // controller initial payload) — no local config file is read.
        // `Configure` commands merge into the view at runtime and the
        // transport servers serve it via `GetConfig`.
        let startup_state = config.harness.agent().state();
        let startup_model = startup_state.model.clone();
        let startup_thinking = startup_state
            .thinking_level
            .map(|level| level != theway_core::ThinkingLevel::Off);
        drop(startup_state);
        let daemon_config = Arc::new(std::sync::RwLock::new(WireDaemonConfig {
            provider: startup_model.as_ref().map(|model| model.provider.0.clone()),
            model: startup_model.as_ref().map(|model| model.id.clone()),
            base_url: startup_model
                .as_ref()
                .map(|model| model.base_url.clone())
                .filter(|url| !url.is_empty()),
            thinking: startup_thinking,
            builtin_skills: config.startup.builtin_skills.clone(),
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
        Self {
            kernel: ReplKernel::new(config.harness, config.trigger_executor, config.retry),
            registry,
            reload_runtime,
            completer,
            cwd: config.cwd,
            paths: config.paths,
            path_context,
            daemon_config,
            tool_ops,
            session_id: config.session_id,
            log_path: config.log_path,
            tool_count: config.tool_count,
            feed: Feed::new(),
            plain_lines_cache: theway_transport::feed::PlainLinesCache::new(100),
            block_versions: Vec::new(),
            dirty_blocks: BTreeSet::new(),
            latest_trigger_poll: None,
            latest_goal: None,
            feed_rx: Some(config.feed_rx),
            feed_tx: config.feed_tx,
            thinking_summary: config.thinking_summary,
            thinking_burst: super::thinking_summary::ThinkingBurst::default(),
            main_run_rx: Some(config.main_run_rx),
            control_plane_prompt_rx: config.control_plane_prompt_rx,
            control_plane_prompt: None,
            model_catalog: model_catalog(),
            panel_status: config.panel_status,
            tui_max_feed_lines: config.startup.tui_max_feed_lines,
            dag_engine: config.dag_engine,
            subagent_registry: config.subagent_registry,
            session_factory: config.session_factory,
            session_repo: config.session_repo,
            current_session_state: config.current_session_state,
            busy: false,
            queued_turns: VecDeque::new(),
        }
    }

    fn system_line(&mut self, text: impl AsRef<str>) {
        self.feed.push_plain_untimed(text.as_ref(), Level::System);
    }

    fn error_line(&mut self, text: impl AsRef<str>) {
        self.feed.push_plain_untimed(text.as_ref(), Level::Error);
    }

    /// Build the public transport channels and wire the event planes
    /// ([`theway_transport::host::TransportHost::transport_endpoints`] implementation).
    pub fn transport_endpoints(&mut self) -> TransportEndpoints {
        let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(128);
        let latest = Arc::new(Mutex::new(self.wire_snapshot()));
        let (event_tx, _) = broadcast::channel::<WireAgentEvent>(256);
        let (dag_event_tx, _) = broadcast::channel::<WireDagEvent>(256);
        let (core_dag_event_tx, _) = broadcast::channel::<DagEvent>(256);
        self.dag_engine
            .set_event_sender(Some(core_dag_event_tx.clone()));
        let agent_fwd = {
            let mut agent_rx = self.subagent_registry.subscribe();
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
                                tracing::warn!("AgentJobEvent broadcast lagged by {n}, skipping");
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    tracing::debug!(
                        "AgentJobEvent registry channel closed; forwarder task exiting"
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
        TransportEndpoints {
            command_tx,
            command_rx,
            snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            completer: self.completer.clone(),
            job_ops: Arc::new(CoreJobOps::new(self.subagent_registry.clone())),
            graph_ops: Arc::new(CoreGraphOps::new(self.dag_engine.clone())),
            session_ops: Arc::new(crate::session_ops::AppSessionOps::new(
                self.session_repo.clone(),
                self.dag_engine.clone(),
                self.current_session_state.clone(),
            )),
            // Issue #68: the transport servers serve `GetPathContext` from
            // this handle and apply the `SetSkillDirs` optimistic update
            // against it; the event loop holds the authoritative copy.
            path_context: self.path_context.clone(),
            // The transport servers serve `GetConfig` from this authoritative
            // handle; only the event loop updates it after applying a patch.
            daemon_config: self.daemon_config.clone(),
            // Issue #76: file/process operations are forwarded to the
            // controller's ToolService endpoint through the shared config's
            // `tool_service_addr`.
            tool_ops: self.tool_ops.clone(),
            // Issue #84: runtime state externalization is wired as an RPC
            // contract first; the storage-backed implementation lands with
            // the controller-storage phase (#85/#86).
            storage_ops: std::sync::Arc::new(theway_transport::UnavailableStorageOps),
            session_id: self.session_id.clone(),
            agent_fwd,
        }
    }

    /// Serialized transport event loop: drains the endpoint channels into the host
    /// and drives the selected transport server until shutdown.
    pub async fn run_transport_loop(
        mut self,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        mut server_task: tokio::task::JoinHandle<Result<()>>,
    ) -> Result<()> {
        let label = mode.label();
        let mut command_rx = endpoints.command_rx;
        let latest = endpoints.latest;
        let snapshot_tx = endpoints.snapshot_tx;

        let mut feed_rx = self.feed_rx.take().expect("feed_rx taken once");
        let mut main_run_rx = self.main_run_rx.take().expect("main_run_rx taken once");
        let mut control_plane_prompt_rx = self.control_plane_prompt_rx.take();
        let mut turn = TurnState::default();
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

        #[cfg(unix)]
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

        loop {
            tokio::select! {
                biased;
                result = poll_turn(&mut turn.fut), if turn.fut.is_some() => {
                    self.finish_turn(&mut turn, result).await;
                    dirty = true;
                    metadata_dirty = true;
                }
                Some(command) = command_rx.recv() => {
                    self.handle_web_command(command, &mut turn).await;
                    dirty = true;
                    metadata_dirty = true;
                }
                Some(update) = feed_rx.recv() => {
                    metadata_dirty |= self.apply_feed_update(update);
                    while let Ok(update) = feed_rx.try_recv() {
                        metadata_dirty |= self.apply_feed_update(update);
                    }
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
                }, if self.control_plane_prompt.is_none() && control_plane_prompt_rx.is_some() => {
                    self.show_control_plane_prompt(prompt);
                    dirty = true;
                    metadata_dirty = true;
                }
                _ = publish_tick.tick(), if dirty => {
                    dirty = false;
                    self.publish_snapshot(&latest, &snapshot_tx, metadata_dirty).await;
                    metadata_dirty = false;
                }
                _ = tokio::signal::ctrl_c() => {
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx, true).await;
                    }
                    break;
                }
                _ = async { sigterm.as_mut().unwrap().recv().await }, if sigterm.is_some() => {
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
        Ok(())
    }
}
