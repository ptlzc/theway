impl TextArea {
    /// Compute the number of lines to scroll per mouse wheel tick based on
    /// the viewport height. Small viewports scroll slowly; large viewports
    /// scroll by up to three lines.
    fn scroll_lines_for_height(height: u16) -> u16 {
        match height {
            0..=5 => 1,
            6..=15 => 2,
            _ => 3,
        }
    }

    /// Drag-scroll throttle intervals (ms): ramps up from slow to fast.
    /// After the last entry, the final value repeats.
    const DRAG_SCROLL_RAMP_MS: &[u128] = &[80, 60, 40];

    /// Compute the drag-scroll interval for the given step count.
    fn drag_scroll_interval(step: u32) -> u128 {
        let ramp = Self::DRAG_SCROLL_RAMP_MS;
        ramp[ramp.len().min(step as usize + 1) - 1]
    }

    /// How many extra lines to scroll based on distance from area edge.
    /// Returns 1 for 1-2 rows outside, 2 for 3-4 rows, 3 for 5-8, etc.
    fn drag_scroll_lines_for_distance(distance: u16) -> usize {
        match distance {
            0..=2 => 1,
            3..=5 => 2,
            6..=10 => 3,
            _ => 5,
        }
    }

    /// Clamp a buffer position so it stays within a wrapped line's range
    /// `[line_start, line_end)`.  Without this, `display_col_to_buffer_pos`
    /// can return `line_end` when the column exceeds the line's display
    /// width — and `line_end` equals the *next* wrapped line's start,
    /// which confuses `effective_scroll` into thinking the cursor hasn't
    /// actually moved to the target line.
    ///
    /// Uses `self.text` to find the last valid char boundary inside the line
    /// so we never land in the middle of a multi-byte character.
    fn clamp_to_line(&self, pos: usize, line_start: usize, line_end: usize) -> usize {
        if line_end > line_start {
            // Find the start of the last character in the line.
            let last_char_start = self.text[line_start..line_end]
                .char_indices()
                .next_back()
                .map(|(i, _)| line_start + i)
                .unwrap_or(line_start);
            pos.min(last_char_start)
        } else {
            line_start
        }
    }

    pub fn new() -> Self {
        Self {
            text: EditBuffer::new(),
            wrap_cache: RefCell::new(None),
            preferred_col: None,
            elements: Vec::new(),
            next_element_id: 0,
            kill_buffer: String::new(),
            undo: UndoState::default(),
            selection: None,
            clipboard_provider: Box::new(InternalClipboard::default()),
            clipboard: None,
            keep_selection_after_mouseup: true,
            selection_style: Style::default()
                .bg(Color::Rgb(49, 62, 115))
                .fg(Color::Rgb(192, 202, 245)),
            mouse_down_pos: None,
            drag_anchor: None,
            drag_active: false,
            last_drag_scroll: None,
            drag_scroll_steps: 0,
            pending_drag_scroll: None,
            click_tracker: ClickTracker::default(),
            scroll_override: None,
            show_scrollbar: true,
            scrollbar_track_style: Style::default().bg(Color::Rgb(32, 35, 53)),
            scrollbar_thumb_style: Style::default()
                .fg(Color::Rgb(42, 46, 65))
                .bg(Color::Rgb(32, 35, 53)),
            scrollbar_padding: 0,
            scrollbar_dragging: false,
            hovered_element: None,
            pending_element_event: None,
            tab_width: 4,
        }
    }

    /// Columns per tab for display width and tab→space expansion (`0` = passthrough).
    pub fn tab_width(&self) -> u8 {
        self.tab_width
    }

    /// Set columns per tab. Also controls expansion on insert/`set_text`/`replace_range`.
    pub fn set_tab_width(&mut self, tab_width: u8) {
        if self.tab_width != tab_width {
            self.tab_width = tab_width;
            self.wrap_cache.replace(None);
        }
    }

    /// Expand `\t` to `tab_width` spaces (scrollback-compatible fixed width).
    /// `tab_width == 0` or no tabs → borrowed input.
    ///
    /// Public because it is the exact transform every insert path applies
    /// (see [`insert_str`](Self::insert_str) /
    /// [`insert_element`](Self::insert_element)), letting hosts canonicalize
    /// external text before comparing it against buffer content.
    pub fn expand_tabs<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        expand_tabs_with_width(text, self.tab_width)
    }

    /// Display width of plain buffer text, treating tabs as `tab_width` columns.
    fn plain_display_width(&self, text: &str) -> usize {
        plain_display_width_with_tab(text, self.tab_width)
    }

    /// Display width of a single grapheme cluster (tab uses `tab_width`).
    fn grapheme_display_width(&self, grapheme: &str) -> usize {
        grapheme_display_width_with_tab(grapheme, self.tab_width)
    }

    fn element_ranges(&self) -> Vec<Range<usize>> {
        self.elements
            .iter()
            .map(|element| element.range.clone())
            .collect()
    }

    fn adjust_position_after_edit(
        position: usize,
        replaced: &Range<usize>,
        inserted_len: usize,
    ) -> usize {
        if position < replaced.start {
            position
        } else if position <= replaced.end {
            replaced.start + inserted_len
        } else {
            position - replaced.len() + inserted_len
        }
    }

    fn is_semantic_edit(plan: &EditPlan) -> bool {
        plan.removed_text() != plan.replacement() || !plan.replaced_byte_range().is_empty()
    }

    fn assert_valid_edit_plan(&self, plan: &EditPlan) {
        if let Err(error) = self.text.validate_plan(plan) {
            panic!("textarea edit invariant failed: {error:?}");
        }
    }

    fn apply_validated_edit_plan(
        &mut self,
        plan: EditPlan,
        mutation_kind: Option<MutationKind>,
    ) -> EditOutcome {
        let semantic_edit = Self::is_semantic_edit(&plan);
        let replaced = plan.replaced_byte_range();
        let inserted_len = plan.replacement().len();
        let outcome = self.text.apply_validated_plan(&plan);
        if semantic_edit {
            self.update_elements_after_replace(replaced.start, replaced.end, inserted_len);
            if let Some(selection) = &mut self.selection {
                selection.anchor =
                    Self::adjust_position_after_edit(selection.anchor, &replaced, inserted_len);
                selection.head =
                    Self::adjust_position_after_edit(selection.head, &replaced, inserted_len);
            }
            if self
                .selection
                .is_some_and(|selection| selection.anchor == selection.head)
            {
                self.selection = None;
            }
            self.wrap_cache.replace(None);
            if mutation_kind == Some(MutationKind::Kill) {
                self.kill_buffer = plan.into_removed_text();
            }
        }
        if semantic_edit || !matches!(outcome, EditOutcome::Unchanged) {
            self.preferred_col = None;
            self.scroll_override = None;
        }
        outcome
    }

    fn try_apply_edit_plan(
        &mut self,
        plan: EditPlan,
        mutation_kind: Option<MutationKind>,
    ) -> Result<EditOutcome, ApplyEditPlanError> {
        self.text.validate_plan(&plan)?;
        let semantic_edit = Self::is_semantic_edit(&plan);
        if semantic_edit && let Some(kind) = mutation_kind {
            self.pre_mutate(kind);
        }
        let outcome = self.apply_validated_edit_plan(plan, mutation_kind);
        if semantic_edit && mutation_kind.is_some() {
            self.post_mutate();
        }
        Ok(outcome)
    }

    fn apply_edit_plan(
        &mut self,
        plan: EditPlan,
        mutation_kind: Option<MutationKind>,
    ) -> EditOutcome {
        match self.try_apply_edit_plan(plan, mutation_kind) {
            Ok(outcome) => outcome,
            Err(error) => panic!("textarea edit invariant failed: {error:?}"),
        }
    }

    fn apply_edit_command(
        &mut self,
        command: EditCommand,
        mutation_kind: Option<MutationKind>,
    ) -> EditOutcome {
        let category = command.category();
        let ranges = self.element_ranges();
        let plan = self.text.plan_command(command, &ranges);
        let outcome = self.apply_edit_plan(plan, mutation_kind);
        if category == EditCommandCategory::Navigation {
            self.preferred_col = None;
            self.scroll_override = None;
        }
        outcome
    }

    fn plan_edit_replacement(&self, range: Range<usize>, replacement: &str) -> EditPlan {
        let replacement = self.expand_tabs(replacement).into_owned();
        let ranges = self.element_ranges();
        self.text
            .plan_replace_byte_range(range, &replacement, &ranges)
    }

    fn apply_edit_replacement(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        mutation_kind: Option<MutationKind>,
    ) {
        let plan = self.plan_edit_replacement(range, replacement);
        self.apply_edit_plan(plan, mutation_kind);
    }

    pub fn set_text(&mut self, text: &str) {
        let cursor = self.cursor();
        let plan = self.plan_edit_replacement(0..self.text.len(), text);
        self.assert_valid_edit_plan(&plan);
        self.pre_mutate(MutationKind::Replace);
        let _ = self.text.apply_validated_plan(&plan);
        self.elements.clear();
        let len = self.text.len();
        self.set_cursor_inner(cursor.min(len));
        self.wrap_cache.replace(None);
        self.preferred_col = None;
        // Kill buffer intentionally survives: yank is independent of buffer
        // content, so a cut can be pasted into a fresh prompt after send.
        self.selection = None;
        self.mouse_down_pos = None;
        self.drag_anchor = None;
        self.drag_active = false;
        self.last_drag_scroll = None;
        self.drag_scroll_steps = 0;
        self.pending_drag_scroll = None;
        self.click_tracker = ClickTracker::default();
        self.scroll_override = None;
        self.scrollbar_dragging = false;
        self.hovered_element = None;
        self.pending_element_event = None;
        self.post_mutate();
    }

    pub fn text(&self) -> &str {
        self.text.text()
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.scroll_override = None;
        // Word boundary: break the insert batch when char class changes (ws↔non-ws).
        if let Some(first) = text.chars().next() {
            let first_ws = first.is_whitespace();
            if self.undo.last_kind == Some(MutationKind::Insert)
                && self.undo.last_insert_ws != first_ws
            {
                // Force pre_mutate to see a "kind change" so it pushes a checkpoint.
                self.undo.last_kind = None;
            }
        }
        self.apply_edit_replacement(
            self.cursor()..self.cursor(),
            text,
            Some(MutationKind::Insert),
        );
        if let Some(last) = text.chars().last() {
            self.undo.last_insert_ws = last.is_whitespace();
        }
    }

    pub fn insert_str_at(&mut self, pos: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        self.apply_edit_replacement(pos..pos, text, Some(MutationKind::Insert));
        if let Some(last) = text.chars().last() {
            self.undo.last_insert_ws = last.is_whitespace();
        }
    }

    pub fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        self.apply_edit_replacement(range, text, Some(MutationKind::Replace));
    }

    pub fn cursor(&self) -> usize {
        self.text.cursor_byte()
    }

    pub fn set_cursor(&mut self, pos: usize) {
        let pos = pos.clamp(0, self.text.len());
        let pos = self.clamp_pos_to_nearest_boundary(pos);
        self.set_cursor_inner(pos);
        self.preferred_col = None;
        self.scroll_override = None;
    }

    fn set_cursor_inner(&mut self, pos: usize) {
        let _ = self.text.set_cursor_byte(pos);
    }

    /// Override the scroll position, bypassing cursor-follow logic.
    ///
    /// When set to `Some(offset)`, `effective_scroll` will use this offset
    /// instead of ensuring the cursor is visible. Useful for forcing a
    /// specific viewport (e.g., scroll-to-top when the textarea is collapsed
    /// and unfocused). Set to `None` to restore normal cursor-following.
    ///
    /// Note: unlike the internal scroll_override set by mousewheel events,
    /// this is NOT cleared by cursor movement — it persists until explicitly
    /// cleared by the caller.
    pub fn set_scroll_override(&mut self, scroll: Option<u16>) {
        self.scroll_override = scroll;
    }

    /// Current scroll override value (if any).
    pub fn scroll_override(&self) -> Option<u16> {
        self.scroll_override
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        self.wrapped_lines(width).len() as u16
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.cursor_pos_with_state(area, TextAreaState::default())
    }

    /// Compute the on-screen cursor position taking scrolling into account.
    ///
    /// Returns `None` if the cursor is not visible in the current viewport
    /// (e.g. the user scrolled the viewport away from the cursor via mousewheel).
    ///
    /// Unlike [`screen_position_of`], this applies a wrap-boundary adjustment:
    /// when the cursor sits at the exact wrap boundary (col == content width),
    /// it is shown at the start of the next visual line instead of on the
    /// invisible right border.
    pub fn cursor_pos_with_state(&self, area: Rect, state: TextAreaState) -> Option<(u16, u16)> {
        let tw = self.text_width(area);
        let lines = self.wrapped_lines(tw);
        let effective_scroll = self.effective_scroll(area.height, &lines, state.scroll);
        let mut i = Self::wrapped_line_index_by_start(&lines, self.cursor())?;
        let ls = &lines[i];
        let mut col = self.display_width_of_range(ls.start, self.cursor()) as u16;

        // If the cursor sits at the exact wrap boundary (col == content width),
        // show it at the start of the next visual line instead of on the
        // invisible right border.  When the cursor is at text.len() and the
        // last line is exactly full, there is no next wrapped line — but we
        // still want the cursor on a new row at column 0.
        if col >= tw {
            i += 1;
            col = 0;
        }

        // If the cursor's visual line is outside the visible viewport, hide it.
        let scroll = effective_scroll as usize;
        if i < scroll || i >= scroll + area.height as usize {
            return None;
        }

        let screen_row = (i - scroll) as u16;
        Some((area.x + col, area.y + screen_row))
    }

    /// Compute the on-screen position of an arbitrary buffer byte offset.
    ///
    /// Returns `None` if the position is outside the visible viewport.
    /// Does not apply cursor-specific wrap-boundary adjustments — see
    /// [`cursor_pos_with_state`] for cursor positioning.
    pub fn screen_position_of(
        &self,
        pos: usize,
        area: Rect,
        state: TextAreaState,
    ) -> Option<(u16, u16)> {
        let tw = self.text_width(area);
        let lines = self.wrapped_lines(tw);
        let effective_scroll = self.effective_scroll(area.height, &lines, state.scroll);
        let i = Self::wrapped_line_index_by_start(&lines, pos)?;
        let ls = &lines[i];
        let col = self.display_width_of_range(ls.start, pos) as u16;

        let scroll = effective_scroll as usize;
        if i < scroll || i >= scroll + area.height as usize {
            return None;
        }

        let screen_row = (i - scroll) as u16;
        Some((area.x + col, area.y + screen_row))
    }

    /// Compute the on-screen cells covered by a buffer byte range.
    ///
    /// A soft-wrapped range can cross visual rows, so unlike
    /// [`screen_position_of`] this returns one height-1 [`Rect`] per visual
    /// row the range intersects, top to bottom, clamped to the content
    /// region (`text_width` columns — excludes any scrollbar column). Rows
    /// scrolled outside the viewport are skipped, so a partially visible
    /// range yields only its visible rows. Bytes belonging to no row (a
    /// `\n`, or whitespace dropped at a wrap boundary) are not covered;
    /// trailing spaces kept on a row are. Ranges that are empty, extend
    /// past the text, or have non-char-boundary endpoints yield no spans.
    pub fn screen_spans_of_range(
        &self,
        range: Range<usize>,
        area: Rect,
        state: TextAreaState,
    ) -> Vec<Rect> {
        let mut spans = Vec::new();
        if range.start >= range.end
            || range.end > self.text.len()
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
        {
            return spans;
        }
        let tw = self.text_width(area);
        let lines = self.wrapped_lines(tw);
        let scroll = self.effective_scroll(area.height, &lines, state.scroll) as usize;
        // Rows before the one containing `range.start` cannot intersect;
        // `None` (start ahead of the first row) falls back to scanning all.
        let first = Self::wrapped_line_index_by_start(&lines, range.start).unwrap_or(0);
        // Rendered content stops at `tw` columns; a row's trailing wrap
        // spaces can measure wider, so clamp to the content edge, not the
        // full area (whose last column may hold the scrollbar).
        let right_edge = area.x.saturating_add(tw);
        for (i, ls) in lines.iter().enumerate().skip(first) {
            if ls.start >= range.end {
                break;
            }
            if i < scroll {
                continue;
            }
            if i >= scroll + area.height as usize {
                break;
            }
            let seg_start = range.start.max(ls.start);
            let seg_end = range.end.min(ls.end);
            if seg_start >= seg_end {
                continue;
            }
            let start_x = area
                .x
                .saturating_add(self.display_width_of_range(ls.start, seg_start) as u16)
                .min(right_edge);
            let end_x = area
                .x
                .saturating_add(self.display_width_of_range(ls.start, seg_end) as u16)
                .min(right_edge);
            if start_x < end_x {
                spans.push(Rect {
                    x: start_x,
                    y: area.y + (i - scroll) as u16,
                    width: end_x - start_x,
                    height: 1,
                });
            }
        }
        spans
    }

    /// Map screen coordinates `(col, row)` to a buffer byte position.
    ///
    /// Returns `None` if `(col, row)` is outside the textarea `area`.
    ///
    /// Edge cases:
    /// - Click past end of a wrapped line → snaps to line end.
    /// - Click below all text → snaps to `text.len()`.
    /// - Click on an element → snaps to nearest element boundary (start or end).
    pub fn buffer_pos_at_screen(
        &self,
        col: u16,
        row: u16,
        area: Rect,
        state: TextAreaState,
    ) -> Option<usize> {
        // Outside the textarea area → None.
        if col < area.x || col >= area.x + area.width || row < area.y || row >= area.y + area.height
        {
            return None;
        }

        let tw = self.text_width(area);
        let lines = self.wrapped_lines(tw);
        let scroll = self.effective_scroll(area.height, &lines, state.scroll);

        let visual_row = (row - area.y) as usize + scroll as usize;

        // Below all text → end of text.
        if visual_row >= lines.len() {
            return Some(self.text.len());
        }

        let line = &lines[visual_row];
        let target_col = (col - area.x) as usize;
        // Clamp line.end to text length (safety measure for edge cases).
        let line_end = line.end.min(self.text.len());
        Some(
            self.display_col_to_buffer_pos(line.start, line_end, target_col)
                .0,
        )
    }

    /// Like `buffer_pos_at_screen` but also indicates whether the column
    /// fell on an element's display region.
    fn buffer_pos_at_screen_ex(
        &self,
        col: u16,
        row: u16,
        area: Rect,
        state: TextAreaState,
    ) -> Option<(usize, bool)> {
        if col < area.x || row < area.y {
            return None;
        }

        let tw = self.text_width(area);
        let lines = self.wrapped_lines(tw);
        let scroll = self.effective_scroll(area.height, &lines, state.scroll);

        let visual_row = (row - area.y) as usize + scroll as usize;

        if visual_row >= lines.len() {
            return Some((self.text.len(), false));
        }

        let line = &lines[visual_row];
        let target_col = (col - area.x) as usize;
        let line_end = line.end.min(self.text.len());
        Some(self.display_col_to_buffer_pos(line.start, line_end, target_col))
    }

    /// Return the element at screen coordinates, if any.
    ///
    /// Uses `buffer_pos_at_screen` to find the buffer position, then checks
    /// whether that position falls inside an element.
    pub fn element_at_screen(
        &self,
        col: u16,
        row: u16,
        area: Rect,
        state: TextAreaState,
    ) -> Option<&TextElement> {
        let (pos, hit_element) = self.buffer_pos_at_screen_ex(col, row, area, state)?;
        if hit_element {
            // hit_element means the column fell on an element's display.
            // pos may be elem start or elem end — match either.
            self.elements
                .iter()
                .find(|e| pos >= e.range.start && pos <= e.range.end && !e.range.is_empty())
        } else {
            self.elements
                .iter()
                .find(|e| pos >= e.range.start && pos < e.range.end)
        }
    }

    // ── Selection API ──

    /// Normalized selection range, expanded to element boundaries.
    ///
    /// Returns `None` if no selection is active or anchor == head (empty).
    pub fn selection_range(&self) -> Option<Range<usize>> {
        let sel = self.selection?;
        if sel.anchor == sel.head {
            return None;
        }
        let start = sel.anchor.min(sel.head);
        let end = sel.anchor.max(sel.head);
        let expanded = self.expand_range_to_element_boundaries(start..end);
        let clamped_start = expanded.start.min(self.text.len());
        let clamped_end = expanded.end.min(self.text.len());
        if clamped_start >= clamped_end {
            None
        } else {
            Some(clamped_start..clamped_end)
        }
    }

    /// Text within the current selection (buffer text, not display text).
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        Some(self.text[range].to_string())
    }

    /// Clear the selection without affecting the clipboard.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Delete the selected range (if any). Returns `true` if text was deleted.
    ///
    /// This is a single undo step. After deletion, the cursor is placed at
    /// the start of the deleted range and the selection is cleared.
    pub fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            return false;
        };
        let start = range.start;
        self.apply_edit_replacement(range, "", Some(MutationKind::Replace));
        self.set_cursor_inner(start.min(self.text.len()));
        self.post_mutate();
        self.selection = None;
        true
    }

    /// Set the selection programmatically.
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        self.selection = Some(Selection { anchor, head });
    }

    /// Take the clipboard contents (returns `None` if empty).
    ///
    /// This is the primary way for the host app to retrieve text
    /// that was selected by mouse drag / double-click / triple-click.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    /// Peek at the current clipboard content without consuming it.
    pub fn clipboard(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    /// Replace the clipboard provider. The default is [`InternalClipboard`]
    /// (in-memory only). Pass an `arboard`-backed implementation to sync
    /// copy/cut/paste with the system clipboard.
    pub fn set_clipboard_provider(&mut self, provider: Box<dyn ClipboardProvider>) {
        self.clipboard_provider = provider;
    }

    // ── Element events ──

    /// Take the pending [`TextElementEvent`], if any.
    ///
    /// Call this after [`handle_mouse`](Self::handle_mouse) to check whether
    /// an element was clicked or hover-entered/left.
    pub fn poll_element_event(&mut self) -> Option<TextElementEvent> {
        self.pending_element_event.take()
    }

    /// Internal: set clipboard text via the provider AND the notification field.
    fn set_clipboard_text(&mut self, text: String) {
        if !text.is_empty() {
            self.clipboard_provider.set(&text);
            self.clipboard = Some(text);
        }
    }

    // ── Timers / tick ──

    /// Recommended poll timeout for the host event loop.
    ///
    /// When the textarea has pending timer-driven work (e.g. continuous
    /// drag-scrolling while the mouse is held outside the area), this
    /// returns `Some(ms)`.  The host should use this as the
    /// `event::poll` timeout.  When the poll times out without an event,
    /// call [`tick`](Self::tick).
    ///
    /// Returns `None` when no timer work is pending — the host can use
    /// its own default timeout.
    pub fn poll_timeout_ms(&self) -> Option<u64> {
        // Drag-scroll is the only timer-driven feature for now.
        self.pending_drag_scroll.as_ref()?;
        let interval = Self::drag_scroll_interval(self.drag_scroll_steps);
        Some(interval as u64)
    }

    /// Advance timer-driven work (called by the host when `poll` times
    /// out).  Returns a `MouseAction` describing what changed (typically
    /// `SelectionUpdated` for drag-scroll, or `Nothing`).
    pub fn tick(&mut self, area: Rect, state: TextAreaState) -> MouseAction {
        // Drag-scroll continuation.
        if let Some(event) = self.pending_drag_scroll {
            return self.handle_mouse(event, area, state);
        }
        MouseAction::Nothing
    }

    // ── Mouse ──

    /// Shared single/double-click treatment of a click that landed on an
    /// element display (`hit_element`): snap the cursor to the element
    /// start, anchor drags there, and emit [`TextElementEventKind::Click`].
    ///
    /// Returns `None` when the click was not on an element.
    fn element_click_snap(&mut self, pos: usize, hit_element: bool) -> Option<MouseAction> {
        if !hit_element {
            return None;
        }
        let elem = self
            .elements
            .iter()
            .find(|e| pos >= e.range.start && pos <= e.range.end && !e.range.is_empty())?;
        let id = elem.id;
        let start = elem.range.start;
        self.set_cursor_inner(start);
        self.preferred_col = None;
        self.drag_anchor = Some(start);
        self.pending_element_event = Some(TextElementEvent {
            id,
            kind: TextElementEventKind::Click,
        });
        Some(MouseAction::CursorPlaced)
    }
}
