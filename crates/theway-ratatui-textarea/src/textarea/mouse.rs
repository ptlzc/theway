impl TextArea {
    /// Process a crossterm `MouseEvent` and return what happened.
    ///
    /// The host app is expected to call this from its event loop for
    /// every `Event::Mouse(mouse)` and pass the textarea's render `area`
    /// plus the current `TextAreaState` (for scroll info).
    pub fn handle_mouse(
        &mut self,
        event: MouseEvent,
        area: Rect,
        state: TextAreaState,
    ) -> MouseAction {
        // ── Scrollbar interaction ──
        // When scrollbar is shown, clicks/drags on the rightmost column
        // control the scroll position instead of placing the cursor.
        let tw = self.text_width(area);
        let has_scrollbar = self.show_scrollbar && tw < area.width;
        let on_scrollbar = has_scrollbar && event.column == area.x + area.width - 1;

        // Handle scrollbar drag continuation (even if pointer moved off the column).
        if self.scrollbar_dragging {
            match event.kind {
                MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::Down(MouseButton::Left) => {
                    return self.handle_scrollbar_click(event.row, area, tw);
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.scrollbar_dragging = false;
                    return MouseAction::Scrolled;
                }
                _ => {}
            }
        }

        if on_scrollbar && let MouseEventKind::Down(MouseButton::Left) = event.kind {
            self.scrollbar_dragging = true;
            // If the click is on the thumb, don't jump — just start the drag
            // from the current position.  Only jump when clicking the track.
            if self.is_scrollbar_thumb_at(event.row, area, tw) {
                return MouseAction::Scrolled;
            }
            return self.handle_scrollbar_click(event.row, area, tw);
        }

        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Some terminals re-emit Down(Left) after a scroll event
                // even though the button was held the whole time.  When a
                // drag is already active, treat this as a drag continuation
                // so the selection anchor is preserved.
                if self.drag_active {
                    return self.handle_mouse(
                        MouseEvent {
                            kind: MouseEventKind::Drag(MouseButton::Left),
                            ..event
                        },
                        area,
                        state,
                    );
                }

                let col = event.column;
                let row = event.row;

                // Track multi-click (double/triple).
                let click_count = self.click_tracker.register(col, row);

                // Record the mouse-down position (for drag detection).
                self.mouse_down_pos = Some((col, row));
                self.drag_active = false;
                self.last_drag_scroll = None;
                self.drag_scroll_steps = 0;
                self.pending_drag_scroll = None;

                // Clear any existing selection.
                self.clear_selection();

                // Map screen coordinates to buffer position.
                // IMPORTANT: this must happen BEFORE clearing scroll_override
                // so that effective_scroll uses the current viewport, not the
                // cursor-following fallback.
                let Some((pos, hit_element)) = self.buffer_pos_at_screen_ex(col, row, area, state)
                else {
                    self.scroll_override = None;
                    self.drag_anchor = None;
                    return MouseAction::Nothing;
                };

                // Now that we have the correct buffer position, clear the
                // scroll override so the viewport follows the cursor again.
                self.scroll_override = None;

                match click_count {
                    2 => {
                        // Double-click on an element display: snap like a
                        // single click (cursor to element start + Click
                        // event). Word-selecting would select and copy the
                        // element's hidden buffer text to the clipboard;
                        // the host decides what a chip double-click means.
                        // Triple-click line-select below intentionally keeps
                        // buffer-text semantics, element content included —
                        // a copy gesture, like drag-select across a chip.
                        if let Some(action) = self.element_click_snap(pos, hit_element) {
                            return action;
                        }
                        // Double-click: select word under cursor.
                        // Whitespace clicks just place the cursor (no selection).
                        let is_ws = pos < self.text.len()
                            && self.text[pos..]
                                .chars()
                                .next()
                                .is_none_or(|ch| ch.is_whitespace());
                        let start = self.word_start_at(pos);
                        let end = self.word_end_at(pos);
                        if !is_ws && start < end {
                            self.selection = Some(Selection {
                                anchor: start,
                                head: end,
                            });
                            // Place cursor on the last character of the
                            // selection (neovim style), not one past the end.
                            let cursor = self.text[start..end]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| start + i)
                                .unwrap_or(start);
                            self.set_cursor_inner(cursor);
                            self.preferred_col = None;
                            if let Some(text) = self.selected_text() {
                                self.set_clipboard_text(text);
                            }
                            return MouseAction::SelectionFinished;
                        }
                        // Clicked on whitespace — just place cursor.
                        self.set_cursor_inner(pos);
                        self.preferred_col = None;
                        MouseAction::CursorPlaced
                    }
                    3 => {
                        // Triple-click: select entire source line (\n-delimited).
                        let line_start = self.beginning_of_line(pos);
                        // Include the trailing \n if present.
                        let line_end_excl = self.end_of_line(pos);
                        let line_end = if line_end_excl < self.text.len() {
                            line_end_excl + 1 // include \n
                        } else {
                            line_end_excl
                        };
                        self.selection = Some(Selection {
                            anchor: line_start,
                            head: line_end,
                        });
                        // Keep cursor at the click position (like neovim),
                        // not at the end of the selection.
                        self.set_cursor_inner(pos);
                        self.preferred_col = None;
                        if let Some(text) = self.selected_text() {
                            self.set_clipboard_text(text);
                        }
                        MouseAction::SelectionFinished
                    }
                    _ => {
                        // Single click: place cursor.
                        //
                        // If click landed on an element display, snap cursor
                        // to elem start. `hit_element` is reliable because
                        // display_col_to_buffer_pos sets it when the column
                        // falls within an element's visual width.
                        if let Some(action) = self.element_click_snap(pos, hit_element) {
                            return action;
                        }

                        self.drag_anchor = Some(pos);
                        self.set_cursor_inner(pos);
                        self.preferred_col = None;

                        MouseAction::CursorPlaced
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(anchor) = self.drag_anchor else {
                    return MouseAction::Nothing;
                };

                // Compute the buffer position for the drag endpoint.
                // We need to scope the `lines` borrow so it's dropped before
                // we mutate self.

                // Throttle drag-scroll (above/below area) to avoid
                // lightning-fast scrolling at mouse-report rate.
                // Acceleration: first step waits 80ms, then 60ms, then 40ms.
                let outside_area = event.row < area.y || event.row >= area.y + area.height;
                if outside_area {
                    // Store event for continuous drag-scroll re-triggering.
                    self.pending_drag_scroll = Some(event);

                    let now = Instant::now();
                    let interval = Self::drag_scroll_interval(self.drag_scroll_steps);
                    if let Some(last) = self.last_drag_scroll
                        && now.duration_since(last).as_millis() < interval
                    {
                        return MouseAction::Nothing;
                    }
                    self.last_drag_scroll = Some(now);
                    self.drag_scroll_steps = self.drag_scroll_steps.saturating_add(1);
                } else {
                    // Back inside area — cancel continuous drag-scroll.
                    self.pending_drag_scroll = None;
                }

                let (head, new_scroll) = {
                    let tw = self.text_width(area);
                    let lines = self.wrapped_lines(tw);
                    let scroll = self.effective_scroll(area.height, &lines, state.scroll) as usize;
                    let visible_end = scroll + area.height as usize;

                    if event.row < area.y {
                        // ── Dragging above the area → scroll up ──
                        let dist = area.y - event.row;
                        let n = Self::drag_scroll_lines_for_distance(dist);
                        let target_line = scroll.saturating_sub(n);
                        let pos = if target_line < lines.len() {
                            let col = event.column.saturating_sub(area.x) as usize;
                            let line = &lines[target_line];
                            let line_end = line.end.min(self.text.len());
                            let p = self.display_col_to_buffer_pos(line.start, line_end, col).0;
                            self.clamp_to_line(p, line.start, line_end)
                        } else {
                            0
                        };
                        (pos, Some(target_line as u16))
                    } else if event.row >= area.y + area.height {
                        // ── Dragging below the area → scroll down ──
                        let dist = event.row - (area.y + area.height) + 1;
                        let n = Self::drag_scroll_lines_for_distance(dist);
                        let target_line = (visible_end + n - 1).min(lines.len().saturating_sub(1));
                        let max_scroll = lines.len().saturating_sub(area.height as usize);
                        let new_scroll = (target_line + 1)
                            .saturating_sub(area.height as usize)
                            .min(max_scroll);
                        let pos = if target_line < lines.len() {
                            let col = event.column.saturating_sub(area.x) as usize;
                            let line = &lines[target_line];
                            let line_end = line.end.min(self.text.len());
                            let pos = self.display_col_to_buffer_pos(line.start, line_end, col).0;
                            self.clamp_to_line(pos, line.start, line_end)
                        } else {
                            self.text.len()
                        };
                        (pos, Some(new_scroll as u16))
                    } else {
                        // ── Within the area → normal drag ──
                        let col = event.column.clamp(area.x, area.x + tw.saturating_sub(1));
                        let row = event.row;
                        drop(lines); // release borrow for buffer_pos_at_screen
                        match self.buffer_pos_at_screen(col, row, area, state) {
                            Some(pos) => (pos, None),
                            None => return MouseAction::Nothing,
                        }
                    }
                };

                if let Some(s) = new_scroll {
                    self.scroll_override = Some(s);
                }
                if head == anchor {
                    self.drag_active = false;
                    self.selection = None;
                } else {
                    self.drag_active = true;
                    self.selection = Some(Selection { anchor, head });
                }
                self.set_cursor_inner(head);
                self.preferred_col = None;

                if self.selection.is_some() {
                    MouseAction::SelectionUpdated
                } else {
                    MouseAction::CursorPlaced
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.mouse_down_pos = None;
                let was_drag = self.drag_active;
                self.drag_active = false;
                self.scrollbar_dragging = false;
                self.pending_drag_scroll = None;
                self.drag_anchor = None;

                if was_drag {
                    // Discard zero-width selections (anchor == head) that arise
                    // from mouse jitter — they look like an active selection to
                    // the keyboard handler and silently swallow Backspace/Delete.
                    if self.selection_range().is_none() {
                        self.selection = None;
                        MouseAction::CursorPlaced
                    } else {
                        // Finalize selection: copy to clipboard.
                        if let Some(text) = self.selected_text()
                            && !text.is_empty()
                        {
                            self.set_clipboard_text(text);
                        }

                        if !self.keep_selection_after_mouseup {
                            self.selection = None;
                        }

                        MouseAction::SelectionFinished
                    }
                } else {
                    MouseAction::Nothing
                }
            }
            MouseEventKind::ScrollDown => {
                let tw = self.text_width(area);
                let lines = self.wrapped_lines(tw);
                let total = lines.len();
                if total <= area.height as usize {
                    return MouseAction::Nothing;
                }
                let max_scroll = total.saturating_sub(area.height as usize) as u16;
                let current = self
                    .scroll_override
                    .unwrap_or_else(|| self.effective_scroll(area.height, &lines, state.scroll));
                let scroll_lines = Self::scroll_lines_for_height(area.height);
                let new_scroll = (current + scroll_lines).min(max_scroll);
                if new_scroll == current {
                    return MouseAction::Nothing;
                }
                // If dragging, extend the selection head to follow the scroll.
                let drag_new_pos = if self.drag_active {
                    let target_line =
                        (new_scroll as usize + area.height as usize - 1).min(lines.len() - 1);
                    Some(lines[target_line].start)
                } else {
                    None
                };
                drop(lines);
                self.scroll_override = Some(new_scroll);
                if let Some(new_pos) = drag_new_pos {
                    if let Some(sel) = &mut self.selection {
                        sel.head = new_pos;
                    }
                    self.set_cursor_inner(new_pos);
                }
                MouseAction::Scrolled
            }
            MouseEventKind::ScrollUp => {
                let tw = self.text_width(area);
                let lines = self.wrapped_lines(tw);
                let total = lines.len();
                if total <= area.height as usize {
                    return MouseAction::Nothing;
                }
                let current = self
                    .scroll_override
                    .unwrap_or_else(|| self.effective_scroll(area.height, &lines, state.scroll));
                let scroll_lines = Self::scroll_lines_for_height(area.height);
                let new_scroll = current.saturating_sub(scroll_lines);
                if new_scroll == current {
                    return MouseAction::Nothing;
                }
                // If dragging, extend the selection head to follow the scroll.
                let drag_new_pos = if self.drag_active {
                    let target_line = new_scroll as usize;
                    Some(if target_line < lines.len() {
                        lines[target_line].start
                    } else {
                        0
                    })
                } else {
                    None
                };
                drop(lines);
                self.scroll_override = Some(new_scroll);
                if let Some(new_pos) = drag_new_pos {
                    if let Some(sel) = &mut self.selection {
                        sel.head = new_pos;
                    }
                    self.set_cursor_inner(new_pos);
                }
                MouseAction::Scrolled
            }
            MouseEventKind::Moved => {
                // Hover detection: hit-test elements under the cursor.
                let hovered_id = self
                    .element_at_screen(event.column, event.row, area, state)
                    .map(|e| e.id);

                let prev = self.hovered_element;
                if hovered_id != prev {
                    // Emit leave for the old element first, then enter for the new one.
                    // We only store the last event; if both happen, prefer enter
                    // (the caller already knows about the old element from a prior enter).
                    if let Some(old_id) = prev {
                        self.pending_element_event = Some(TextElementEvent {
                            id: old_id,
                            kind: TextElementEventKind::HoverLeave,
                        });
                    }
                    if let Some(new_id) = hovered_id {
                        self.pending_element_event = Some(TextElementEvent {
                            id: new_id,
                            kind: TextElementEventKind::HoverEnter,
                        });
                    }
                    self.hovered_element = hovered_id;
                }
                MouseAction::Nothing
            }
            _ => MouseAction::Nothing,
        }
    }

    /// Handle a click or drag on the scrollbar track.
    ///
    /// Maps the row position proportionally to a scroll offset:
    /// clicking at the top of the track scrolls to the start, at the
    /// bottom scrolls to the end.
    fn handle_scrollbar_click(&mut self, row: u16, area: Rect, tw: u16) -> MouseAction {
        if area.height == 0 {
            return MouseAction::Nothing;
        }
        let total = {
            let lines = self.wrapped_lines(tw);
            lines.len()
        };
        if total <= area.height as usize {
            return MouseAction::Nothing;
        }
        let max_scroll = total.saturating_sub(area.height as usize) as u16;
        let rel_row = row.saturating_sub(area.y);
        // Map relative row to a scroll offset proportionally.
        let scroll = if area.height <= 1 {
            0
        } else {
            ((rel_row as u32 * max_scroll as u32) / (area.height.saturating_sub(1)) as u32) as u16
        };
        self.scroll_override = Some(scroll.min(max_scroll));
        MouseAction::Scrolled
    }

    /// Check whether the given screen row falls on the scrollbar thumb.
    ///
    /// Renders the scrollbar into a scratch buffer and checks whether the
    /// cell at `row` is a non-space character (thumb glyph) or a space (track).
    fn is_scrollbar_thumb_at(&self, row: u16, area: Rect, tw: u16) -> bool {
        if area.height == 0 {
            return false;
        }
        let total = {
            let lines = self.wrapped_lines(tw);
            lines.len()
        };
        if total <= area.height as usize {
            return false;
        }
        let current_scroll = self.scroll_override.unwrap_or(0);

        let lengths = ScrollLengths {
            content_len: total,
            viewport_len: area.height as usize,
        };
        let scrollbar = ScrollBar::vertical(lengths).offset(current_scroll as usize);
        let sb_x = area.right().saturating_sub(1);
        let core_area = CoreRect {
            x: sb_x,
            y: area.y,
            width: 1,
            height: area.height,
        };
        let mut scratch = CoreBuffer::empty(core_area);
        (&scrollbar).render(core_area, &mut scratch);

        if row < area.y || row >= area.y + area.height {
            return false;
        }
        scratch[(sb_x, row)].symbol() != " "
    }
}
