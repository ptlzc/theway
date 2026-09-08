impl App {
    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = self.theme.screen.inset(frame.area());
        let input_rows = self.composer_rows(area.width);
        // Inline menu band (model selector issue #72 + `/side-panel` menu
        // issue #54): while a hierarchical menu is open the input area gains
        // a band above the prompt chrome (1 breadcrumb row + the active
        // level's choices, capped at CASCADE_CHOICE_ROWS).
        let menu_band = self.menu_band_data();
        let band_rows = menu_band
            .as_ref()
            .map(|data| menu::band_rows(data.rows.len(), CASCADE_CHOICE_ROWS))
            .unwrap_or(0);
        let chunks = Layout::vertical([
            Constraint::Min(1),
            // Blank spacer between the feed/output and the status bar.
            Constraint::Length(1),
            Constraint::Length(1), // status rule / single-cell Braille indicator
            Constraint::Length(input_rows + 2 + band_rows), // input box + menu band
        ])
        .split(area);
        let content_area = chunks[0];
        let banner_area = chunks[1];
        let status_area = chunks[2];
        let input_area = chunks[3];
        // Transient MCP-failure banner (3s): the spacer row doubles as the
        // banner row; expired banners clear themselves on the next frame
        // (the event loop also wakes at expiry to force that frame).
        if let Some(banner) = self.mcp_error_banner.as_ref() {
            if banner.until <= tokio::time::Instant::now() {
                self.mcp_error_banner = None;
            }
        }
        if let Some(banner) = self.mcp_error_banner.as_ref() {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    theway_transport::feed::truncate_chars(
                        &banner.text,
                        banner_area.width as usize,
                    ),
                    Style::default()
                        .fg(self.theme.sidebar.error)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                )),
                banner_area,
            );
        }
        self.last_status_area = Some(status_area);
        self.last_input_area = Some(input_area);
        // The menu band occupies the top `band_rows` of the input area; the
        // prompt chrome starts below it.
        let (cascade_area, chrome_area) = if band_rows > 0 {
            let split = Layout::vertical([Constraint::Length(band_rows), Constraint::Min(1)])
                .split(input_area);
            (Some(split[0]), split[1])
        } else {
            (None, input_area)
        };
        self.last_cascade_area = cascade_area;
        // Side panel (issue #54): the panel sits on the configured edge of
        // the content area (right by default; the `/side-panel › Position`
        // menu moves it and the feed reclaims the rest).
        let (feed_area, trigger_area) = self.side_panel_layout(content_area);
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
            thinking_input_tokens: self.latest.usage.total_input_tokens,
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
        // DAG status band (issue #38): the band joins the scrollable content
        // as rows appended after the newest feed row, so it scrolls with the
        // feed (up and off-screen) instead of pinning between the feed and
        // the status bar. Hidden mode renders no band rows. Issue #98: once
        // every run reaches a terminal state the band closes by itself.
        let band_rows = if !dag_band::has_live_runs(&self.latest.dags)
            || self.dag_band_mode == crate::ui::DagBandMode::Hidden
            || self.graph_position == crate::ui::GraphPosition::SidePanel
        {
            0
        } else {
            dag_band::band_rows(&self.latest.dags, feed_area.width) as usize
        };
        let feed_total = lines.len();
        let total = feed_total + band_rows;
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
        // Clamp a live FEED mouse selection to the current row count (the feed
        // can shrink between frames, e.g. `/clear`) before it reaches the
        // window renderer (issue #70). Other regions (composer/panel/status)
        // are static per frame and need no clamp.
        if let Some(sel) = self.mouse_select
            && sel.region == SelectRegion::Feed
        {
            let last = feed_total.saturating_sub(1);
            let anchor_line = sel.anchor.line.min(last);
            let current_line = sel.current.line.min(last);
            if anchor_line != sel.anchor.line || current_line != sel.current.line {
                self.mouse_select = Some(MouseSelect {
                    anchor: MousePos {
                        line: anchor_line,
                        col: sel.anchor.col,
                    },
                    current: MousePos {
                        line: current_line,
                        col: sel.current.col,
                    },
                    ..sel
                });
            }
        }
        self.last_display_scroll = display_scroll;
        // The feed occupies the rows above the band's top edge; the band rows
        // live at the very bottom of the scrollable content.
        let feed_visible = (feed_total as i64 - display_scroll as i64)
            .clamp(0, viewport as i64) as u16;
        let feed_render_area = Rect {
            x: feed_area.x,
            y: feed_area.y,
            width: feed_area.width,
            height: feed_visible,
        };
        crate::feed_render::render_lines_window(
            frame.buffer_mut(),
            feed_render_area,
            lines,
            display_scroll,
            self.mouse_select
                .filter(|sel| sel.region == SelectRegion::Feed)
                .map(|sel| {
                    let (start, end) = sel.bounds();
                    crate::feed_render::TextSelection {
                        start_line: start.line,
                        start_col: start.col,
                        end_line: end.line,
                        end_col: end.col,
                    }
                }),
        );
        // DAG band inside the scrollable feed: render the band into a
        // scratch buffer once, then blit the visible rows at the band's
        // scrolled position (clipped against the viewport).
        if band_rows > 0 {
            let band_top_index = feed_total as i64;
            let band_top_y = feed_area.y as i64 + (band_top_index - display_scroll as i64);
            let clip_y = band_top_y.max(feed_area.y as i64) as u16;
            let skip_rows = (display_scroll as i64 - band_top_index).max(0) as usize;
            if clip_y < feed_area.bottom() {
                let mut band_buf = ratatui::buffer::Buffer::empty(Rect::new(
                    0,
                    0,
                    feed_area.width,
                    band_rows as u16,
                ));
                dag_band::render_dag_band(
                    &mut band_buf,
                    Rect::new(0, 0, feed_area.width, band_rows as u16),
                    &self.latest.dags,
                    &self.dag_meters,
                    self.dag_tick,
                    &self.theme.dag_band,
                );
                for y in clip_y..feed_area.bottom() {
                    let src_row = skip_rows + (y - clip_y) as usize;
                    if src_row >= band_rows {
                        break;
                    }
                    // Clear the row first so the band never leaves feed
                    // cells behind its transparent padding.
                    for x in feed_area.x..feed_area.right() {
                        if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
                            cell.reset();
                        }
                    }
                    for x in feed_area.x..feed_area.right() {
                        let Some(src) = band_buf.cell((x - feed_area.x, src_row as u16)) else {
                            continue;
                        };
                        if src.symbol() == " " && src.style() == Style::default() {
                            continue;
                        }
                        if let Some(dst) = frame.buffer_mut().cell_mut((x, y)) {
                            dst.set_symbol(src.symbol());
                            dst.set_style(src.style());
                        }                    }
                }
            }
        }
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
            self.render_trigger_panel(frame, area, self.side_panel_position);
        }

        // Status rule: plain ready/offline rule when idle; a single-cell
        // rainbow Braille spinner while busy.
        if self.busy {
            self.render_busy_status(frame, status_area);
            // Issue #103: snapshot a selectable busy-band line (spinner glyph
            // is replaced by a plain "working" marker for copy purposes).
            self.status_select_lines = vec![Line::raw(" working ".to_string())];
        } else {
            let label = self.status_label();
            self.status_select_lines = vec![Line::raw(label.clone())];
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
        // session-cumulative totals. Rendered as USED tokens over the window
        // (e.g. `60k/1M [60%]`).
        let usage_label = {
            let usage = &self.latest.usage;
            let total_tokens = usage.total_input_tokens.saturating_add(usage.output_tokens);
            if usage.context_window > 0 && total_tokens > 0 {
                render_utils::context_usage_label(total_tokens, usage.context_window)
            } else if total_tokens > 0 {
                render_utils::human_tokens(total_tokens)
            } else {
                String::new()
            }
        };
        let features = feature_labels(&self.latest.dags, &self.latest.subagents);
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
            chrome_area,
            &chrome,
            &self.theme.composer,
        );
        // Issue #103: the chrome's inner text rect (past the border + ❯
        // prefix) is the column-accurate composer selection area.
        self.last_input_text_area = (text_area.width > 0).then_some(text_area);
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

        // Completion popup, drawn above the input over the feed.
        self.render_completions(frame, status_area);
        self.render_menu_band_overlay(frame);
        self.render_control_plane_prompt(frame);
        self.render_fork_picker(frame);
        self.render_resume_picker(frame);
        self.render_extension_view(frame);
    }

    /// Assemble the inline band payload for whichever hierarchical menu is
    /// open (the model selector or the `/side-panel` menu). `None` = no
    /// menu, no band.
    fn menu_band_data(&self) -> Option<MenuBandData> {
        if let Some(picker) = self.model_picker.as_ref() {
            let data = picker.cascade(CASCADE_CHOICE_ROWS);
            return Some(MenuBandData {
                crumbs: vec![
                    MenuCrumb {
                        label: "provider",
                        pinned: data.provider.to_string(),
                    },
                    MenuCrumb {
                        label: "model",
                        pinned: data.model.clone(),
                    },
                    MenuCrumb {
                        label: "thinking",
                        pinned: data.thinking.clone(),
                    },
                ],
                active: match data.active {
                    crate::model_picker::CascadeColumn::Provider => 0,
                    crate::model_picker::CascadeColumn::Model => 1,
                    crate::model_picker::CascadeColumn::Thinking => 2,
                },
                title: data.title.clone(),
                rows: data.rows.clone(),
            });
        }
        if let Some(menu) = self.graph_menu.as_ref() {
            let items = menu.items();
            let rows = items
                .iter()
                .enumerate()
                .map(|(index, label)| (label.to_string(), index == menu.cursor))
                .collect();
            let crumbs = vec![MenuCrumb {
                label: "graph",
                pinned: match menu.level {
                    GraphMenuLevel::Root => String::new(),
                    GraphMenuLevel::Position => "Position".to_string(),
                },
            }];
            return Some(MenuBandData {
                active: 0,
                crumbs,
                title: String::new(),
                rows,
            });
        }
        let menu = self.panel_menu.as_ref()?;
        let items = menu.items();
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, label)| (label.to_string(), index == menu.cursor))
            .collect();
        let crumbs = vec![MenuCrumb {
            label: "side-panel",
            pinned: match menu.level {
                PanelMenuLevel::Root => String::new(),
                PanelMenuLevel::Toggle => "Toggle".to_string(),
                PanelMenuLevel::Position => "Position".to_string(),
            },
        }];
        Some(MenuBandData {
            active: 0,
            crumbs,
            title: String::new(),
            rows,
        })
    }

    /// Split the content area for the side panel (issue #54): the panel
    /// occupies the configured edge (right by default; the `/side-panel ›
    /// Position` menu moves it) and the feed reclaims the remaining space.
    /// Returns `(feed_area, panel_area)`; `None` panel = fully hidden.
    fn side_panel_layout(&self, area: Rect) -> (Rect, Option<Rect>) {
        let Some(width) = self.side_panel_width(area.width) else {
            return (area, None);
        };
        match self.side_panel_position {
            SidePanelPosition::Right => {
                let cols = Layout::horizontal([Constraint::Min(40), Constraint::Length(width)])
                    .split(area);
                (cols[0], Some(cols[1]))
            }
            SidePanelPosition::Left => {
                let cols = Layout::horizontal([Constraint::Length(width), Constraint::Min(40)])
                    .split(area);
                (cols[1], Some(cols[0]))
            }
            SidePanelPosition::Top => {
                let rows = Layout::vertical([
                    Constraint::Length(TRIGGER_PANEL_HEIGHT),
                    Constraint::Min(1),
                ])
                .split(area);
                (rows[1], Some(rows[0]))
            }
            SidePanelPosition::Bottom => {
                let rows = Layout::vertical([
                    Constraint::Min(1),
                    Constraint::Length(TRIGGER_PANEL_HEIGHT),
                ])
                .split(area);
                (rows[0], Some(rows[1]))
            }
        }
    }

    /// Shared inline band renderer: whichever menu is open draws its
    /// breadcrumb + choice list above the composer (issue #72/#54).
    fn render_menu_band_overlay(&self, frame: &mut ratatui::Frame) {
        let Some(data) = self.menu_band_data() else {
            return;
        };
        let Some(cascade_area) = self.last_cascade_area else {
            return;
        };
        render_menu_band(
            frame,
            cascade_area,
            &data,
            &self.theme.picker,
            &self.theme.composer,
        );
    }


    fn render_control_plane_prompt(&self, frame: &mut ratatui::Frame) {
        let Some(prompt) = self.control_plane_prompt.as_ref() else {
            return;
        };
        let area = self.theme.screen.inset(frame.area());
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
        let area = self.theme.screen.inset(frame.area());
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

    /// Interactive `/resume` picker (issue #56): a full-width popup listing
    /// the daemon's sessions by last activity (oldest → newest, newest at
    /// the bottom), reusing the completion popup style — cyan rows,
    /// black-on-cyan highlight, a fixed [`RESUME_POPUP_MAX`]-row window
    /// that slides with the selection. Rows render short id + time + path +
    /// name + busy/graph marks via [`resume_picker_label`]; the daemon's
    /// current session is annotated.
    /// Enter in `app_input::handle_resume_picker_key` switches session.
    fn render_resume_picker(&self, frame: &mut ratatui::Frame) {
        let Some(picker) = self.resume_picker.as_ref() else {
            return;
        };
        if picker.entries.is_empty() {
            return;
        }
        let area = self.theme.screen.inset(frame.area());
        let width = area.width;
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
