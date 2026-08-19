impl App {
    async fn handle_event<B: ratatui::backend::Backend>(
        &mut self,
        event: Event,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.handle_key(key, terminal).await?;
            }
            Event::Key(key) if key.kind == KeyEventKind::Release => {
                // Releasing a key ends the keyboard scroll acceleration
                // chain (issue #38).
                self.reset_scroll_repeat();
            }
            Event::Mouse(m) => self.handle_mouse_event(m).await,
            Event::Paste(text) => {
                self.insert_paste_text(text);
            }
            _ => {}
        }
        Ok(())
    }

    fn scroll_up(&mut self, n: usize) {
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
        // render() clamps and re-enables follow when we reach the bottom.
    }

    /// Keyboard scroll acceleration multiplier (issue #38): +0.1x per
    /// consecutive same-direction key event, capped at 1.5x. The first
    /// press is always 1.0x.
    fn scroll_key_mult(repeat: u32) -> f64 {
        (1.0 + f64::from(repeat) * 0.1).min(1.5)
    }

    /// Record a keyboard scroll key event (Press/Repeat) and return the
    /// accelerated step: `base * mult`. Same-direction consecutive events
    /// increment [`Self::scroll_repeat`]; a direction change restarts the
    /// chain at 1.0x. Mouse-wheel scrolling never calls this — it keeps the
    /// fixed [`SCROLL_STEP`].
    fn scroll_key_step(&mut self, up: bool, base: usize) -> usize {
        if self.scroll_repeat_up == Some(up) {
            self.scroll_repeat = self.scroll_repeat.saturating_add(1);
        } else {
            self.scroll_repeat = 0;
            self.scroll_repeat_up = Some(up);
        }
        let mult = Self::scroll_key_mult(self.scroll_repeat);
        usize::max(1, ((base as f64) * mult).round() as usize)
    }

    /// Reset the keyboard scroll acceleration chain (any key Release).
    fn reset_scroll_repeat(&mut self) {
        self.scroll_repeat = 0;
        self.scroll_repeat_up = None;
    }

    // ── mouse (issue #37: drag-select feed, drag-resize composer) ──────────────────────────

    async fn handle_mouse_event(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(m);
            }
            MouseEventKind::Down(MouseButton::Left) => self.handle_mouse_down(m),
            MouseEventKind::Drag(MouseButton::Left) => self.handle_mouse_drag(m.column, m.row),
            MouseEventKind::Up(MouseButton::Left) => self.handle_mouse_up().await,
            _ => {}
        }
    }

    fn overlays_open(&self) -> bool {
        self.model_picker.is_some() || self.control_plane_prompt.is_some()
    }

    fn handle_mouse_down(&mut self, m: MouseEvent) {
        if self.overlays_open() {
            return;
        }
        // Composer resize handle: the status rule right above the input box,
        // or the box's own top border row.
        let on_rule = self
            .last_status_area
            .is_some_and(|a| m.row == a.y && m.column >= a.x && m.column < a.right());
        let on_top_border = self
            .last_input_area
            .is_some_and(|a| m.row == a.y && m.column >= a.x && m.column < a.right());
        if on_rule || on_top_border {
            let input_width = self.last_input_area.map(|a| a.width).unwrap_or(0);
            self.resize_drag = Some(ComposerDrag {
                start_row: m.row,
                start_rows: self.composer_rows(input_width),
            });
            return;
        }
        // Composer text area: forward to the textarea (cursor placement,
        // chip clicks, its own drag selection).
        if self
            .last_text_area
            .is_some_and(|a| self.rect_contains(a, m.column, m.row))
        {
            self.input
                .handle_mouse(m, self.last_text_area.unwrap(), self.input_state);
            return;
        }
        // Side-panel left-edge resize handle (issue #54): the border column
        // over the panel's full height starts a width drag. Checked BEFORE
        // the feed drag-select branch so the grab strip never starts a text
        // selection. Grabbing exits Auto: the panel becomes user-controlled
        // at its current width.
        if let Some(area) = self.last_panel_area {
            if m.column == area.x && m.row >= area.y && m.row < area.y.saturating_add(area.height) {
                self.panel_drag = Some(PanelDrag {
                    start_col: m.column,
                    start_width: area.width,
                });
                self.side_panel_mode = SidePanelMode::Shown(area.width);
                return;
            }
        }
        // Feed: begin a drag selection anchored at the clicked cell (row,
        // display column; past the row end clamps to the row end).
        if self.mouse_in_feed(m.column, m.row) {
            let line = self.feed_line_at(m.row);
            let col = self.feed_column_at(m.column, line);
            self.feed_selection = Some(FeedSelection {
                anchor: (line, col),
                head: (line, col),
            });
            self.mouse_selecting = true;
        }
    }

    fn handle_mouse_drag(&mut self, column: u16, row: u16) {
        if let Some(drag) = self.resize_drag {
            let grown = drag.start_row.saturating_sub(row);
            let rows = drag
                .start_rows
                .saturating_add(grown)
                .clamp(1, DRAG_MAX_INPUT_ROWS);
            self.manual_composer_rows = Some(rows);
            return;
        }
        if let Some(drag) = self.panel_drag {
            // The panel's right edge stays anchored while the left edge
            // follows the pointer: width = start_width + (start_col - col)
            // (signed — dragging right of the grab column shrinks the
            // panel), clamped to [SIDE_PANEL_MIN_WIDTH, SIDE_PANEL_MAX_WIDTH].
            // Dragging to or past the panel's right edge (its last column),
            // or squeezing the width below the floor, hides the panel
            // (issue #54).
            let right = drag
                .start_col
                .saturating_add(drag.start_width)
                .saturating_sub(1);
            let width = i64::from(drag.start_width) + i64::from(drag.start_col) - i64::from(column);
            self.side_panel_mode = if column >= right || width < i64::from(SIDE_PANEL_MIN_WIDTH) {
                SidePanelMode::Hidden
            } else {
                SidePanelMode::Shown(width.clamp(
                    i64::from(SIDE_PANEL_MIN_WIDTH),
                    i64::from(SIDE_PANEL_MAX_WIDTH),
                ) as u16)
            };
            return;
        }
        if self.mouse_selecting {
            let line = self.feed_line_at(row);
            let col = self.feed_column_at(column, line);
            if let Some(sel) = self.feed_selection.as_mut() {
                sel.head = (line, col);
            }
        }
    }

    /// Mouse release ends any drag; a feed drag copies the selection
    /// (primary-selection semantics, issue #53).
    async fn handle_mouse_up(&mut self) {
        self.resize_drag = None;
        self.panel_drag = None;
        let was_selecting = self.mouse_selecting;
        self.mouse_selecting = false;
        if was_selecting && self.feed_selection.is_some() {
            self.copy_selection().await;
        }
    }

    fn handle_mouse_scroll(&mut self, m: MouseEvent) {
        // Composer text area: forward the wheel event to the textarea so
        // multi-line / wrapped drafts can be browsed (issue #38); content
        // that fits the view is a no-op there.
        if self
            .last_text_area
            .is_some_and(|a| self.rect_contains(a, m.column, m.row))
        {
            self.input
                .handle_mouse(m, self.last_text_area.unwrap(), self.input_state);
            return;
        }
        if !self.mouse_in_feed(m.column, m.row) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll_up(SCROLL_STEP),
            MouseEventKind::ScrollDown => self.scroll_down(SCROLL_STEP),
            _ => {}
        }
    }

    fn mouse_in_feed(&self, column: u16, row: u16) -> bool {
        let Some(area) = self.last_feed_area else {
            return false;
        };
        self.rect_contains(area, column, row)
    }

    fn rect_contains(&self, area: Rect, column: u16, row: u16) -> bool {
        column >= area.x
            && column < area.x.saturating_add(area.width)
            && row >= area.y
            && row < area.y.saturating_add(area.height)
    }

    /// Uncapped rendered-line index under terminal `row` (feed coordinates).
    fn feed_line_at(&self, row: u16) -> usize {
        let rel = self
            .last_feed_area
            .map(|a| row.saturating_sub(a.y) as usize)
            .unwrap_or(0);
        let line = self.selection_view.top.saturating_add(rel);
        line.min(self.selection_view.total.saturating_sub(1))
    }

    /// Display column under terminal `column` in the feed row `line_idx`
    /// (uncapped): relative to the feed area, clamped to the row's text
    /// width — terminal semantics, past the row end lands on the row end
    /// (issue #53).
    fn feed_column_at(&self, column: u16, line_idx: usize) -> usize {
        let rel = self
            .last_feed_area
            .map(|a| column.saturating_sub(a.x) as usize)
            .unwrap_or(0);
        let trimmed = self.feed_cache.trimmed();
        let width = self
            .feed_cache
            .lines()
            .get(line_idx.saturating_sub(trimmed))
            .map(|l| selection::line_text_width(l))
            .unwrap_or(0);
        rel.min(width)
    }

    /// Effective composer row count (issue #40): the mouse-dragged override
    /// wins; otherwise the textarea's own soft-wrap decides — a single
    /// logical line that overflows the input box's content width wraps into
    /// more rows instead of clipping. `content_width = input_area_width - 5`
    /// (chrome pad 2+1 columns + the 2-column `❯` prefix); a draft taller
    /// than [`MAX_INPUT_ROWS`] re-measures one column narrower to reserve
    /// the scrollbar track, then clamps to `1..=MAX_INPUT_ROWS`.
    fn composer_rows(&self, input_area_width: u16) -> u16 {
        if let Some(rows) = self.manual_composer_rows {
            return rows;
        }
        let content_width = input_area_width.saturating_sub(5);
        let rows = self.input.desired_height(content_width);
        let rows = if rows > MAX_INPUT_ROWS as u16 {
            self.input.desired_height(content_width.saturating_sub(1))
        } else {
            rows
        };
        rows.clamp(1, MAX_INPUT_ROWS as u16)
    }

    /// Number of visual lines the draft occupies, counting element chips as
    /// single lines (paste objects can hold newlines invisibly, issue #37).
    fn input_display_lines(&self) -> usize {
        let text = self.input.text();
        let mut lines = 0;
        let mut pos = 0;
        for chip in self.input.elements().iter().filter(|e| e.display.is_some()) {
            let plain = &text[pos..chip.range.start];
            if !plain.is_empty() {
                lines += plain.matches('\n').count() + 1;
            }
            lines += 1; // the chip itself renders as one visual line
            pos = chip.range.end;
        }
        let tail = &text[pos..];
        if !tail.is_empty() {
            lines += tail.matches('\n').count() + 1;
        }
        lines
    }

    // ── rendering ───────────────────────────────────────────────────────────────────────
}
