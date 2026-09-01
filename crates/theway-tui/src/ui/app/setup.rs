impl App {
    pub fn new(config: AppConfig) -> Self {
        let initial = config.initial;
        let initial_runtime_revision = initial.sidebar.runtime_revision;
        let mut feed = Feed::new();
        // Issue #79: on a fresh attach (reused daemon, no explicit resume) the
        // App must NOT show the previous session's feed — it starts empty and
        // the session is created on the first message (issue #46). Any explicit
        // session selection loads that session's feed via `select_session`.
        if !config.fresh_attach {
            feed.replace_blocks(&initial.feed_blocks);
        }
        let mut commands = collect_slash_commands(
            &config.registry,
            &initial.sidebar.skills.items,
            &initial.sidebar.commands,
            &initial.sidebar.mcp.tool_names,
        );
        commands.extend(
            initial
                .extensions
                .commands
                .iter()
                .map(|command| format!("/ext:{}", command.name)),
        );
        let completer = SlashCompleter::from_commands(commands);
        Self {
            client: config.client,
            connector: config.connector,
            session_id: initial.session_id.clone(),
            pending_fresh_attach: config.fresh_attach,
            dag_band_mode: crate::ui::DagBandMode::Show,
            cwd: config.cwd,
            model_config_path: config.model_config_path,
            pending_model_default: None,
            pending_thinking_default: None,
            registry: config.registry,
            completer,
            history: config.history,
            history_idx: None,
            draft: String::new(),
            pending_skill: None,
            pending_images: config.pending_images,
            pending_pasted_images: Vec::new(),
            feed,
            connection_log: Vec::new(),
            panel_status: PanelStatus::from_sidebar(&initial.sidebar),
            model_catalog: initial.model_catalog.clone(),
            model_picker: None,
            control_plane_prompt: initial.control_plane_prompt.clone(),
            latest_goal: initial.goal.clone(),
            latest_trigger_poll: initial.latest_trigger_poll.clone(),
            latest: initial,
            session_snapshot: None,
            input: new_textarea(),
            input_state: TextAreaState::default(),
            completions: Vec::new(),
            completion_idx: 0,
            completion_scroll: 0,
            scroll: 0,
            follow: true,
            scroll_repeat: 0,
            scroll_repeat_up: None,
            thinking_mode: crate::feed_render::ThinkingMode::Full,
            tools_expanded: false,
            color_level: config.color_level,
            theme: Theme::load(),
            last_runtime_revision: initial_runtime_revision,
            feed_cache: crate::feed_cache::FeedRenderCache::new(),
            last_viewport_h: 1,
            last_feed_area: None,
            last_display_scroll: 0,
            mouse_select: None,
            busy: false,
            spinner_frame: 0,
            cps_meter: stats::CpsMeter::new(),
            token_meter: stats::CpsMeter::new(),
            spinner: pixel_loader::RainbowSpinner::new(),
            dag_meters: std::collections::HashMap::new(),
            dag_tick: 0,
            side_panel_mode: SidePanelMode::Auto,
            status_panel_menu: None,
            extension_view: false,
            fork_picker: None,
            resume_picker: None,
            last_status_area: None,
            last_input_area: None,
            last_cascade_area: None,
            last_panel_area: None,
            last_ctrlc: None,
            quit: false,
            connected: true,
            resync_pending: false,
            resubscribe_session: None,
            auto_session: config.auto_session,
            messaged_sessions: std::collections::HashSet::new(),
        }
    }

    // ── startup feed seeding (called by main.rs before run) ─────────────────────────────

    /// Daemon address this client is connected to (for the banner / diagnostics).
    pub fn client_addr(&self) -> &str {
        self.client.addr()
    }

    pub fn banner(&mut self) {
        self.feed
            .push_plain_untimed("──────── theway ────────", Level::Header);
        self.feed.push_plain_untimed(
            format!(
                "model:   {} (daemon: {})",
                self.latest.model,
                self.client.addr()
            ),
            Level::Output,
        );
        self.feed
            .push_plain_untimed(format!("session: {}", self.session_id), Level::Output);
        // Issue #79: fresh attach (reused daemon, no explicit resume) starts a
        // brand-new session on the first message; surface that instead of the
        // stale previous-session id.
        if self.pending_fresh_attach {
            self.feed.push_plain_untimed(
                "新 session: 将在首条消息时创建 (不再加载上一会话)",
                Level::System,
            );
        }
        let tools = if self.latest.sidebar.tools.names.is_empty() {
            "(none)".to_string()
        } else {
            self.latest.sidebar.tools.names.join(", ")
        };
        self.feed
            .push_plain_untimed(format!("tools:   {tools}"), Level::Output);
        self.feed.push_plain_untimed(
            "Enter send · Ctrl-V paste text/images · Ctrl-C abort/exit · /help",
            Level::System,
        );
    }

    pub fn system_line(&mut self, text: impl AsRef<str>) {
        self.feed.push_plain(text.as_ref(), Level::System);
    }

    pub(super) fn connection_line(&mut self, text: impl Into<String>) {
        const MAX_CONNECTION_LOG_LINES: usize = 8;
        let text = text.into();
        if self.connection_log.len() == MAX_CONNECTION_LOG_LINES {
            self.connection_log.remove(0);
        }
        self.connection_log.push(text.clone());
        self.system_line(text);
    }

    pub fn error_line(&mut self, text: impl AsRef<str>) {
        self.feed
            .push_plain(format!("error: {}", text.as_ref()), Level::Error);
    }
}
