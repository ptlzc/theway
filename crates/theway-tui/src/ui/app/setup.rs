impl App {
    pub fn new(config: AppConfig) -> Self {
        let initial = config.initial;
        let initial_runtime_revision = initial.sidebar.runtime_revision;
        let mut feed = Feed::new();
        feed.replace_blocks(&initial.feed_blocks);
        let completer = SlashCompleter::from_commands(collect_slash_commands(
            &config.registry,
            &initial.sidebar.skills.items,
            &initial.sidebar.commands,
            &initial.sidebar.mcp.tool_names,
        ));
        Self {
            client: config.client,
            session_id: initial.session_id.clone(),
            cwd: config.cwd,
            registry: config.registry,
            completer,
            history: config.history,
            history_idx: None,
            draft: String::new(),
            pending_skill: None,
            pending_images: config.pending_images,
            pending_pasted_images: Vec::new(),
            feed,
            panel_status: PanelStatus::from_sidebar(&initial.sidebar),
            model_catalog: initial.model_catalog.clone(),
            model_picker: None,
            control_plane_prompt: initial.control_plane_prompt.clone(),
            latest_goal: initial.goal.clone(),
            latest_trigger_poll: initial.latest_trigger_poll.clone(),
            latest: initial,
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
            feed_selection: None,
            copy_handler: None,
            feed_cache: crate::feed_cache::FeedRenderCache::new(),
            selection_view: SelectionView::default(),
            last_viewport_h: 1,
            last_feed_area: None,
            busy: false,
            spinner_frame: 0,
            busy_started: None,
            cps_meter: stats::CpsMeter::new(),
            spinner: pixel_loader::RainbowSpinner::new(),
            dag_meters: std::collections::HashMap::new(),
            dag_tick: 0,
            manual_composer_rows: None,
            resize_drag: None,
            side_panel_mode: SidePanelMode::Auto,
            status_panel_menu: None,
            panel_drag: None,
            fork_picker: None,
            resume_picker: None,
            mouse_selecting: false,
            last_text_area: None,
            last_status_area: None,
            last_input_area: None,
            last_panel_area: None,
            last_ctrlc: None,
            quit: false,
            connected: true,
            resync_pending: false,
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

    pub fn error_line(&mut self, text: impl AsRef<str>) {
        self.feed
            .push_plain(format!("error: {}", text.as_ref()), Level::Error);
    }
}
