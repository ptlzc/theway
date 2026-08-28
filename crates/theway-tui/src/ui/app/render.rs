impl App {
    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let input_rows = self.composer_rows(area.width);
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1), // status rule / single-cell Braille indicator
            Constraint::Length(input_rows + 2), // input box with border
            Constraint::Length(1), // hint line
        ])
        .split(area);
        let content_area = chunks[0];
        let status_area = chunks[1];
        let input_area = chunks[2];
        let hint_area = chunks[3];
        self.last_status_area = Some(status_area);
        self.last_input_area = Some(input_area);
        // DAG status band (issue #38): while DAG runs are live the band
        // squeezes the feed's bottom rows, between the feed and the busy
        // band.
        let (content_area, dag_band_area) = if self.latest.dags.is_empty() {
            (content_area, None)
        } else {
            let rows = dag_band::band_rows(&self.latest.dags, content_area.width)
                .min(content_area.height.saturating_sub(1));
            if rows == 0 {
                (content_area, None)
            } else {
                let split = Layout::vertical([Constraint::Min(1), Constraint::Length(rows)])
                    .split(content_area);
                (split[0], Some(split[1]))
            }
        };
        let (feed_area, trigger_area) = match self.side_panel_width(content_area.width) {
            Some(width) => {
                let cols = Layout::horizontal([Constraint::Min(40), Constraint::Length(width)])
                    .split(content_area);
                (cols[0], Some(cols[1]))
            }
            None => (content_area, None),
        };
        self.last_feed_area = Some(feed_area);
        // Issue #54: record the rendered panel rect for left-edge drag
        // hit-testing; cleared whenever the panel is not rendered so a stale
        // rect never matches a grab.
        self.last_panel_area = trigger_area;

        // Feed: block-render cache + visible-window draw (issue #34). The
        // cache re-renders only dirty blocks; the window draw is O(viewport).
        // Scrollback cap (issue #27): N = the daemon-pushed `[tui]
        // max_feed_lines` config value, falling back to DEFAULT_MAX_FEED_LINES.
        // `self.scroll` lives in *uncapped* coordinates (it only grows as the
        // feed grows), so the cache's head trim cannot drift a scrolled-up
        // view; the display scroll is the uncapped offset shifted down by the
        // trimmed count.
        let max_feed_lines = self
            .latest
            .tui_max_feed_lines
            .map(|n| n as usize)
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_FEED_LINES);
        let opts = crate::feed_render::FeedRenderOptions {
            thinking_mode: self.thinking_mode,
            tools_expanded: self.tools_expanded,
            color_level: self.color_level,
            // Live throughput + the recent turn's token counts (issue #44):
            // the snapshot usage carries the most recent round, and the wire
            // resets it to 0 between turns, so a 0 naturally renders as 0.
            thinking_cps: self.cps_meter.cps(),
            thinking_input_tokens: self.latest.usage.input_tokens,
            thinking_output_tokens: self.latest.usage.output_tokens,
            // Theme colors + block layout (issues #43 + #49): structural —
            // a change invalidates the feed cache via `PartialEq`.
            theme: self.theme,
            ..Default::default()
        };
        self.feed_cache
            .update(&self.feed, feed_area.width as usize, &opts, max_feed_lines);
        let lines = self.feed_cache.lines();
        let trimmed = self.feed_cache.trimmed();
        let total = lines.len();
        let uncapped_total = total + trimmed;
        let viewport = feed_area.height as usize;
        self.last_viewport_h = viewport;
        let max_scroll = total.saturating_sub(viewport);
        let display_scroll = if self.follow {
            // Bottom anchor in uncapped coordinates: display bottom is
            // max_scroll (= capped_total - viewport), which maps back to
            // (capped_total - viewport) + trimmed = uncapped_total - viewport.
            // Anchoring one viewport above the end keeps a single PageUp a
            // real step (it must not land back on the follow threshold).
            self.scroll = uncapped_total.saturating_sub(viewport);
            max_scroll
        } else {
            let capped = self.scroll.saturating_sub(trimmed).min(max_scroll);
            if capped >= max_scroll {
                self.follow = true;
                self.scroll = uncapped_total.saturating_sub(viewport);
            }
            capped
        };
        crate::feed_render::render_lines_window(
            frame.buffer_mut(),
            feed_area,
            lines,
            display_scroll,
        );
        // Feed scrollbar (theway-pager-render primitive): right edge of the
        // feed pane, subtle while following, brighter when scrolled up.
        if max_scroll > 0 {
            let sb_area = Rect {
                x: feed_area.right().saturating_sub(1),
                y: feed_area.y,
                width: 1,
                height: feed_area.height,
            };
            theway_pager_render::scrollbar::render_scrollbar(
                frame.buffer_mut(),
                Some(sb_area),
                total as u16,
                viewport as u16,
                display_scroll as u16,
                self.follow,
            );
        }
        if let Some(area) = trigger_area {
            self.render_trigger_panel(frame, area);
        }
        if let Some(band_area) = dag_band_area {
            dag_band::render_dag_band(
                frame.buffer_mut(),
                band_area,
                &self.latest.dags,
                &self.dag_meters,
                self.dag_tick,
                &self.theme.dag_band,
            );
        }

        // Status rule: plain ready/offline rule when idle; a single-cell
        // rainbow Braille spinner while busy.
        if self.busy {
            self.render_busy_status(frame, status_area);
        } else {
            frame.render_widget(
                self.status_line(status_area.width as usize, max_scroll),
                status_area,
            );
        }

        // Input box: grok-style chrome (rounded border, ❯ prefix, info line),
        // ported from xai-grok-pager's prompt widget (issue #28).
        let focused = self.model_picker.is_none()
            && self.control_plane_prompt.is_none()
            && !self.extension_view;
        // The info line shows the full `provider:model-id` label (issue #37).
        let model_name = self.latest.model.clone();
        let mut flags: Vec<prompt_chrome::PromptFlag<'_>> = Vec::new();
        // Active thinking level flag (the persisted last-pick default): only
        // renders when reasoning is enabled — "off" stays invisible.
        let thinking_flag: Option<String> =
            (!self.latest.thinking_level.is_empty() && self.latest.thinking_level != "off")
                .then(|| format!("think {}", self.latest.thinking_level));
        if let Some(ref level) = thinking_flag {
            flags.push(prompt_chrome::PromptFlag {
                text: level,
                color: prompt_chrome::GRAY,
                bold: false,
            });
        }
        // Busy state renders in the pixel-loader status band above the box
        // (issue #37), not as an info-line flag.
        let queued_flag: Option<String> =
            (self.latest.queued_count > 0).then(|| format!("{} queued", self.latest.queued_count));
        if let Some(ref q) = queued_flag {
            flags.push(prompt_chrome::PromptFlag {
                text: q,
                color: prompt_chrome::GRAY,
                bold: false,
            });
        }
        // Context-usage label: the wire usage carries the recent turn's token
        // counts (daemon `wire_snapshot`, issue #38), so total ÷ window
        // tracks the live context fill instead of pegging at 100% on
        // session-cumulative totals.
        let usage_label = {
            let usage = &self.latest.usage;
            if usage.context_window > 0 && usage.total_tokens > 0 {
                let pct = ((usage.total_tokens as f64 * 100.0 / usage.context_window as f64)
                    .round())
                .clamp(0.0, 100.0) as u64;
                format!("{pct}% ctx")
            } else if usage.total_tokens > 0 {
                render_utils::human_tokens(usage.total_tokens)
            } else {
                String::new()
            }
        };
        let features = feature_labels(&self.latest.dags);
        let working_dir = self.cwd.to_string_lossy();
        let chrome = prompt_chrome::PromptChrome {
            focused,
            working_dir: Some(working_dir.as_ref()),
            model_name: &model_name,
            flags: &flags,
            features: &features,
            usage: (!usage_label.is_empty()).then_some(usage_label.as_str()),
            input_empty: self.input_text().is_empty(),
            ..prompt_chrome::PromptChrome::default()
        };
        let text_area = prompt_chrome::render_prompt_chrome(
            frame.buffer_mut(),
            input_area,
            &chrome,
            &self.theme.composer,
        );
        let mut cursor_pos = None;
        if text_area.width > 0 && text_area.height > 0 {
            let input = &self.input;
            let input_state = &mut self.input_state;
            frame.render_stateful_widget_ref(input, text_area, input_state);
            // The textarea renders no cursor of its own: draw it at the
            // computed position (state is fresh — the widget just synced the
            // viewport scroll into `input_state`).
            if focused {
                cursor_pos = self
                    .input
                    .cursor_pos_with_state(text_area, self.input_state);
            }
        }
        if let Some((x, y)) = cursor_pos {
            frame.set_cursor_position(ratatui::layout::Position::new(x, y));
        }

        // Hint line.
        let hint = if self.busy {
            "Enter queue next · Ctrl-O thinking · Ctrl-T tools · Ctrl-V paste · Ctrl-C abort"
        } else {
            "Enter send · Ctrl-O thinking · Ctrl-T tools · Ctrl-V paste · ↑↓ history · PgUp/PgDn scroll · Ctrl-C abort"
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                theway_transport::feed::truncate_chars(hint, hint_area.width as usize),
                Style::default().fg(self.theme.composer.hint),
            )),
            hint_area,
        );

        // Completion popup, drawn above the input over the feed.
        self.render_completions(frame, status_area);
        self.render_model_picker(frame);
        self.render_control_plane_prompt(frame);
        self.render_status_panel_menu(frame);
        self.render_fork_picker(frame);
        self.render_resume_picker(frame);
        self.render_extension_view(frame);
    }

    fn render_model_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.model_picker.as_ref() else {
            return;
        };
        let area = frame.area();
        let width = area.width.clamp(40, 64);
        let height = area.height.clamp(8, 18);
        let rect = centered_rect(area, width, height);
        // borders (2) + title line + blank + footer = 5 rows of chrome
        let visible = rect.height.saturating_sub(5).max(1) as usize;
        let (title, rows) = picker.view(visible);
        let picker_theme = self.theme.picker;
        let mut text = vec![
            Line::styled(title, Style::default().fg(picker_theme.title)),
            Line::raw(""),
        ];
        for (label, selected) in rows {
            if selected {
                text.push(Line::styled(
                    format!("❯ {label}"),
                    Style::default().fg(picker_theme.fg),
                ));
            } else {
                text.push(Line::raw(format!("  {label}")));
            }
        }
        text.push(Line::styled(
            "↑↓/jk navigate · Enter select · Esc back",
            Style::default().fg(picker_theme.dim),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Select model ")
            .title_style(Style::default().fg(picker_theme.title))
            .border_style(Style::default().fg(picker_theme.fg));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    fn render_control_plane_prompt(&self, frame: &mut ratatui::Frame) {
        let Some(prompt) = self.control_plane_prompt.as_ref() else {
            return;
        };
        let area = frame.area();
        let width = area.width.clamp(40, 78);
        let height = area.height.clamp(8, 14);
        let rect = centered_rect(area, width, height);
        let text = vec![
            Line::styled(
                "Control-plane approval required",
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(format!(
                "Action: {}",
                safe_control_prompt_label(&prompt.label)
            )),
            Line::raw(format!(
                "Tool: {}",
                safe_control_prompt_text(&prompt.tool_name, 80)
            )),
            Line::raw(format!(
                "Reason: {}",
                safe_control_prompt_text(&prompt.reason, CONTROL_PROMPT_TEXT_WIDTH)
            )),
            Line::raw(format!(
                "Args hash: {}",
                prompt.args_hash.chars().take(12).collect::<String>()
            )),
            Line::raw(format!(
                "Preview: {}",
                theway_transport::feed::truncate_chars(&prompt.payload, CONTROL_PROMPT_TEXT_WIDTH)
            )),
            Line::raw(""),
            Line::styled(
                "Enter/Y approve · N/D/Esc/Ctrl-C deny",
                Style::default().fg(Color::Cyan),
            ),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Confirm ")
            .border_style(Style::default().fg(Color::Yellow));
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(text).block(block).wrap(Wrap { trim: true }),
            rect,
        );
    }

    /// Second-level `/status-panel` menu (issue #54): a centered popup with
    /// the three mode options (`show` / `hide` / `auto`); the highlighted
    /// option renders with the popup's cyan background. Keys are handled in
    /// `app_input::handle_status_panel_menu_key`.
    fn render_status_panel_menu(&self, frame: &mut ratatui::Frame) {
        let Some(selected) = self.status_panel_menu else {
            return;
        };
        let area = frame.area();
        let width = area.width.clamp(20, 34);
        let height = SIDE_PANEL_MENU_ITEMS.len() as u16 + 3; // items + hint + borders
        let rect = centered_rect(area, width, height);
        let picker_theme = self.theme.picker;
        let mut text = Vec::with_capacity(SIDE_PANEL_MENU_ITEMS.len() + 1);
        for (i, label) in SIDE_PANEL_MENU_ITEMS.iter().enumerate() {
            let style = if i == selected {
                Style::default()
                    .fg(picker_theme.highlight_fg)
                    .bg(picker_theme.highlight_bg)
            } else {
                Style::default().fg(picker_theme.fg)
            };
            text.push(Line::styled(format!(" {label} "), style));
        }
        text.push(Line::styled(
            "↑↓ move · Enter apply · Esc cancel",
            Style::default().fg(picker_theme.dim),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" status panel ")
            .title_style(Style::default().fg(picker_theme.title))
            .border_style(Style::default().fg(picker_theme.fg));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    /// Interactive `/fork` picker (issue #55): a centered popup listing the
    /// current session's User messages newest-first (numbers match the
    /// daemon's `/fork <n>` numbering), reusing the completion popup style —
    /// cyan rows, black-on-cyan highlight, a fixed [`FORK_POPUP_MAX`]-row
    /// window that slides with the selection. Enter in
    /// `app_input::handle_fork_picker_key` forwards `/fork <number>`.
    fn render_fork_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.fork_picker.as_ref() else {
            return;
        };
        if picker.entries.is_empty() {
            return;
        }
        let area = frame.area();
        let width = area.width.clamp(24, 80);
        let scroll = picker.scroll.min(picker.entries.len().saturating_sub(1));
        let shown = (picker.entries.len() - scroll).min(FORK_POPUP_MAX);
        let height = shown as u16 + 3; // item rows + hint + borders
        let rect = centered_rect(area, width, height);
        let picker_theme = self.theme.picker;
        let mut text = Vec::with_capacity(shown + 1);
        for (i, entry) in picker.entries.iter().skip(scroll).take(shown).enumerate() {
            let style = if scroll + i == picker.selected {
                Style::default()
                    .fg(picker_theme.highlight_fg)
                    .bg(picker_theme.highlight_bg)
            } else {
                Style::default().fg(picker_theme.fg)
            };
            text.push(Line::styled(
                format!(" {}) {}", entry.number, entry.preview),
                style,
            ));
        }
        text.push(Line::styled(
            "↑↓ move · Enter fork · Esc cancel",
            Style::default().fg(picker_theme.dim),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" fork ")
            .title_style(Style::default().fg(picker_theme.title))
            .border_style(Style::default().fg(picker_theme.fg));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }

    /// Interactive `/resume` picker (issue #56): a centered popup listing
    /// the daemon's sessions in tree order (oldest → newest), reusing the
    /// completion popup style — cyan rows, black-on-cyan highlight, a fixed
    /// [`RESUME_POPUP_MAX`]-row window that slides with the selection.
    /// Rows render short id + name + busy/graph marks via
    /// [`resume_picker_label`]; the daemon's current session is annotated.
    /// Enter in `app_input::handle_resume_picker_key` switches session.
    fn render_resume_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.resume_picker.as_ref() else {
            return;
        };
        if picker.entries.is_empty() {
            return;
        }
        let area = frame.area();
        let width = area.width.clamp(24, 90);
        let scroll = picker.scroll.min(picker.entries.len().saturating_sub(1));
        let shown = (picker.entries.len() - scroll).min(RESUME_POPUP_MAX);
        let height = shown as u16 + 3; // item rows + hint + borders
        let rect = centered_rect(area, width, height);
        let picker_theme = self.theme.picker;
        let mut text = Vec::with_capacity(shown + 1);
        for (i, entry) in picker.entries.iter().skip(scroll).take(shown).enumerate() {
            let style = if scroll + i == picker.selected {
                Style::default()
                    .fg(picker_theme.highlight_fg)
                    .bg(picker_theme.highlight_bg)
            } else {
                Style::default().fg(picker_theme.fg)
            };
            text.push(Line::styled(
                format!(" {}", resume_picker_label(entry)),
                style,
            ));
        }
        text.push(Line::styled(
            "↑↓ move · Enter resume · Esc cancel",
            Style::default().fg(picker_theme.dim),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" resume ")
            .title_style(Style::default().fg(picker_theme.title))
            .border_style(Style::default().fg(picker_theme.fg));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(text).block(block), rect);
    }
}
