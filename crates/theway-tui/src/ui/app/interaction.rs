/// A mouse position inside the rendered feed: capped line index plus a
/// 0-based display column (relative to the feed pane's left edge).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MousePos {
    /// Capped feed line index.
    pub line: usize,
    /// 0-based display column inside the feed line.
    pub col: usize,
}

/// Mouse character selection over the feed (issue #70): the left button
/// anchors a drag on a capped feed position; the selection spans
/// `anchor..=current` (in either order) and is copied to the clipboard via
/// OSC 52 on release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MouseSelect {
    /// First position of the drag (capped feed line + column).
    pub anchor: MousePos,
    /// Current drag position (capped feed line + column).
    pub current: MousePos,
    /// True while the button is held down.
    pub dragging: bool,
}

impl MouseSelect {
    /// Selected endpoints, normalized so `(start, end)` reads top-to-bottom,
    /// left-to-right.
    pub fn bounds(&self) -> (MousePos, MousePos) {
        if self.anchor.line < self.current.line
            || (self.anchor.line == self.current.line && self.anchor.col <= self.current.col)
        {
            (self.anchor, self.current)
        } else {
            (self.current, self.anchor)
        }
    }
}

/// OSC 52 clipboard-set sequence: `ESC ] 52 ; c ; <base64> BEL`. The `c`
/// (clipboard) selection targets the system clipboard; terminals and tmux
/// (with `set-clipboard on`) forward it to the OS clipboard.
fn osc52_bytes(text: &str) -> Vec<u8> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{b64}\x07").into_bytes()
}

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

    /// Mouse handling: wheel scrolls the feed (issue #4), left-button
    /// press-drag-release selects feed characters and copies them to the
    /// system clipboard via OSC 52 (issue #70). Pressing any other button
    /// clears a live selection; scrolling clears it too (row indices shift).
    pub(super) fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.clear_mouse_select();
                self.scroll_up(Self::WHEEL_SCROLL_LINES);
            }
            MouseEventKind::ScrollDown => {
                self.clear_mouse_select();
                self.scroll_down(Self::WHEEL_SCROLL_LINES);
            }
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down_left(mouse),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag_left(mouse),
            MouseEventKind::Up(MouseButton::Left) => self.mouse_up_left(),
            _ => self.clear_mouse_select(),
        }
    }

    /// Wheel scroll step per notch.
    const WHEEL_SCROLL_LINES: usize = 3;

    /// Left-button press inside the feed pane starts a character selection at
    /// the clicked position; a press outside the pane clears any selection.
    fn mouse_down_left(&mut self, mouse: crossterm::event::MouseEvent) {
        let Some(pos) = self.feed_pos_at(mouse.row, mouse.column) else {
            self.clear_mouse_select();
            return;
        };
        self.mouse_select = Some(MouseSelect {
            anchor: pos,
            current: pos,
            dragging: true,
        });
    }

    /// Left-button drag extends the character selection (clamped to the feed
    /// rows).
    fn mouse_drag_left(&mut self, mouse: crossterm::event::MouseEvent) {
        let Some(sel) = self.mouse_select else { return };
        if !sel.dragging {
            return;
        }
        let Some(pos) = self.feed_pos_at(mouse.row, mouse.column) else {
            return;
        };
        self.mouse_select = Some(MouseSelect { current: pos, ..sel });
    }

    /// Left-button release ends the drag: a drag (anchor moved) copies the
    /// selected characters to the clipboard via OSC 52 and stays highlighted;
    /// a plain click (no drag) just clears.
    fn mouse_up_left(&mut self) {
        let Some(sel) = self.mouse_select else { return };
        if !sel.dragging {
            return;
        }
        if sel.anchor == sel.current {
            // Click without drag: no selection to keep.
            self.clear_mouse_select();
            return;
        }
        self.mouse_select = Some(MouseSelect {
            dragging: false,
            ..sel
        });
        let Some(bytes) = self.selection_bytes() else {
            self.clear_mouse_select();
            return;
        };
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }

    /// Map a crossterm mouse position (1-based) to a capped feed line and
    /// display column inside the last rendered feed pane, `None` outside it.
    fn feed_pos_at(&self, row: u16, column: u16) -> Option<MousePos> {
        let area = self.last_feed_area?;
        let row = row.saturating_sub(1);
        let column = column.saturating_sub(1);
        if row < area.y || row >= area.bottom() || column < area.x || column >= area.right() {
            return None;
        }
        let total = self.feed_cache.lines().len();
        let line = self.last_display_scroll + (row - area.y) as usize;
        Some(MousePos {
            line: line.min(total.saturating_sub(1)),
            col: (column - area.x) as usize,
        })
    }

    /// Text of the current selection: rendered feed characters joined with
    /// `\n`, each row's trailing padding trimmed.
    fn selected_text(&self) -> String {
        let Some(sel) = self.mouse_select else {
            return String::new();
        };
        let (start, end) = sel.bounds();
        crate::feed_render::selection_text(
            self.feed_cache.lines(),
            crate::feed_render::TextSelection {
                start_line: start.line,
                start_col: start.col,
                end_line: end.line,
                end_col: end.col,
            },
        )
    }

    /// OSC 52 clipboard payload for the current selection: `None` when the
    /// selection holds no copyable text.
    fn selection_bytes(&self) -> Option<Vec<u8>> {
        let text = self.selected_text();
        if text.is_empty() {
            return None;
        }
        Some(osc52_bytes(&text))
    }

    /// Clear any active mouse selection.
    fn clear_mouse_select(&mut self) {
        self.mouse_select = None;
    }

    fn scroll_up(&mut self, n: usize) {
        self.clear_mouse_select();
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_down(&mut self, n: usize) {
        self.clear_mouse_select();
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
