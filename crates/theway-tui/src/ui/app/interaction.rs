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
            Event::Paste(text) => {
                self.insert_paste_text(text);
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(mouse);
            }
            _ => {}
        }
        Ok(())
    }

    /// Mouse wheel scrolls the feed (issue #4): each notch
    /// moves [`WHEEL_SCROLL_LINES`] lines. Scrolling up detaches follow,
    /// scrolling down re-attaches it once the bottom is reached (clamped by
    /// render()). Any other mouse event is inert.
    pub(super) fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind;
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(Self::WHEEL_SCROLL_LINES),
            MouseEventKind::ScrollDown => self.scroll_down(Self::WHEEL_SCROLL_LINES),
            _ => {}
        }
    }

    /// Wheel scroll step per notch.
    const WHEEL_SCROLL_LINES: usize = 3;

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
    /// chain at 1.0x.
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

    /// Effective composer row count (issue #40): the textarea's soft-wrap
    /// decides, so a single logical line that overflows the input box's
    /// content width wraps into more rows instead of clipping.
    /// `content_width = input_area_width - 5` (chrome pad 2+1 columns + the
    /// 2-column `❯` prefix); a draft taller than [`MAX_INPUT_ROWS`]
    /// re-measures one column narrower to reserve the scrollbar track, then
    /// clamps to `1..=MAX_INPUT_ROWS`.
    fn composer_rows(&self, input_area_width: u16) -> u16 {
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
