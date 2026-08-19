impl TextArea {
    // ===== Text elements support =====

    /// Insert an atomic text element at the current cursor position.
    ///
    /// The `text` is inserted into the buffer and registered as an element.
    /// The `kind` tag is opaque to the textarea (host-defined).
    /// The `display` optionally overrides how the element is rendered.
    ///
    /// Returns the assigned [`ElementId`] so the host can store associated metadata.
    pub fn insert_element(
        &mut self,
        text: &str,
        kind: ElementKind,
        display: Option<Line<'static>>,
    ) -> ElementId {
        let plan = self.plan_edit_replacement(self.cursor()..self.cursor(), text);
        self.apply_element_transaction(plan, kind, display)
    }

    /// Replace a range of buffer text with an atomic element.
    ///
    /// This is the "confirm autocomplete" operation: the trigger text (e.g. `@foo`)
    /// is deleted and replaced with element text (e.g. `@src/foo.rs`) in a single
    /// atomic operation. The cursor is placed at the end of the new element.
    ///
    /// Returns the assigned [`ElementId`].
    pub fn replace_range_with_element(
        &mut self,
        range: Range<usize>,
        text: &str,
        kind: ElementKind,
        display: Option<Line<'static>>,
    ) -> ElementId {
        let plan = self.plan_edit_replacement(range, text);
        self.apply_element_transaction(plan, kind, display)
    }

    fn apply_element_transaction(
        &mut self,
        plan: EditPlan,
        kind: ElementKind,
        display: Option<Line<'static>>,
    ) -> ElementId {
        let start = plan.replaced_byte_range().start;
        let inserted_len = plan.replacement().len();
        self.assert_valid_edit_plan(&plan);
        self.pre_mutate(MutationKind::Element);
        self.apply_validated_edit_plan(plan, Some(MutationKind::Element));
        let end = start + inserted_len;
        let id = self.add_element(start..end, kind, display);
        self.set_cursor(end);
        self.post_mutate();
        id
    }

    fn add_element(
        &mut self,
        range: Range<usize>,
        kind: ElementKind,
        display: Option<Line<'static>>,
    ) -> ElementId {
        let id = ElementId(self.next_element_id);
        self.next_element_id += 1;
        let elem = TextElement {
            id,
            range,
            kind,
            display,
        };
        self.elements.push(elem);
        self.elements.sort_by_key(|e| e.range.start);
        self.wrap_cache.replace(None);
        id
    }

    /// Returns the element at the current cursor position, if any.
    ///
    /// If the cursor is at an element's start boundary, that element is returned.
    /// If the cursor is strictly inside an element (shouldn't happen in normal
    /// operation), the containing element is returned.
    pub fn element_at_cursor(&self) -> Option<&TextElement> {
        self.elements
            .iter()
            .find(|e| self.cursor() >= e.range.start && self.cursor() < e.range.end)
    }

    /// Returns the underlying buffer text for the element with the given id.
    pub fn element_text(&self, id: ElementId) -> Option<&str> {
        self.elements
            .iter()
            .find(|e| e.id == id)
            .map(|e| &self.text[e.range.clone()])
    }

    /// Update the display for an existing element. Invalidates the wrap cache.
    pub fn set_element_display(&mut self, id: ElementId, display: Option<Line<'static>>) {
        if let Some(e) = self.elements.iter_mut().find(|e| e.id == id) {
            e.display = display;
            self.wrap_cache.replace(None);
        }
    }

    /// Returns a slice of all elements, sorted by buffer position.
    pub fn elements(&self) -> &[TextElement] {
        &self.elements
    }

    /// Re-register elements after a [`set_text`] call that placed their
    /// buffer text back verbatim. Each `(range, kind, display)` tuple
    /// describes one element whose text already occupies `range` in the
    /// buffer. No text is inserted — this only recreates the element
    /// metadata so the textarea renders chips instead of raw text.
    pub fn restore_elements(
        &mut self,
        elems: impl IntoIterator<Item = (Range<usize>, ElementKind, Option<Line<'static>>)>,
    ) {
        for (range, kind, display) in elems {
            self.add_element(range, kind, display);
        }
        self.wrap_cache.replace(None);
    }

    /// Inline an element: remove it from the element list so its buffer text
    /// becomes plain editable characters. The text content is unchanged.
    ///
    /// The cursor is placed at the end of the inlined region.
    /// This operation is a single undoable step.
    ///
    /// Returns `true` if the element was found and inlined, `false` otherwise.
    pub fn inline_element(&mut self, id: ElementId) -> bool {
        let Some(idx) = self.elements.iter().position(|e| e.id == id) else {
            return false;
        };
        let end = self.elements[idx].range.end;

        // Snapshot for undo before removing the element.
        self.pre_mutate(MutationKind::Element);

        self.elements.remove(idx);
        self.set_cursor_inner(end);
        self.preferred_col = None;
        self.wrap_cache.replace(None);
        self.undo.last_kind = None; // always discrete

        true
    }

    /// Get the contiguous non-whitespace "word" that the cursor is inside or at the start of.
    ///
    /// Returns `(byte_range, text)` where `byte_range` is the range in the buffer.
    /// Returns `None` if the cursor is on whitespace or the buffer is empty.
    ///
    /// This is useful for trigger-character detection (e.g. finding `@foo` under the cursor
    /// for autocomplete). The host can then check `text.starts_with('@')` etc.
    pub fn word_at_cursor(&self) -> Option<(Range<usize>, &str)> {
        if self.text.is_empty() {
            return None;
        }
        let pos = self.cursor().min(self.text.len());

        // Find word start: scan backward from cursor to find whitespace boundary
        let start = self.text[..pos]
            .rfind(|c: char| c.is_whitespace())
            .map(|i| {
                i + self.text[i..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1)
            })
            .unwrap_or(0);

        // Find word end: scan forward from cursor to find whitespace boundary
        let end = self.text[pos..]
            .find(|c: char| c.is_whitespace())
            .map(|i| i + pos)
            .unwrap_or(self.text.len());

        // Also extend backward from start in case cursor is at word boundary
        // Actually, we also need to handle cursor being between words.
        // If cursor is at whitespace, return None.
        if start >= end {
            return None;
        }

        // If cursor is beyond the word end (cursor at whitespace after word), return None
        // Unless cursor is exactly at start position of the word
        let word = &self.text[start..end];
        if word.chars().all(|c| c.is_whitespace()) {
            return None;
        }

        Some((start..end, word))
    }

    fn find_element_containing(&self, pos: usize) -> Option<usize> {
        self.elements
            .iter()
            .position(|e| pos > e.range.start && pos < e.range.end)
    }

    fn clamp_pos_to_nearest_boundary(&self, mut pos: usize) -> usize {
        if pos > self.text.len() {
            pos = self.text.len();
        }
        if let Some(idx) = self.find_element_containing(pos) {
            let e = &self.elements[idx];
            let dist_start = pos.saturating_sub(e.range.start);
            let dist_end = e.range.end.saturating_sub(pos);
            if dist_start <= dist_end {
                e.range.start
            } else {
                e.range.end
            }
        } else {
            pos
        }
    }

    fn expand_range_to_element_boundaries(&self, mut range: Range<usize>) -> Range<usize> {
        // Expand to include any intersecting elements fully
        loop {
            let mut changed = false;
            for e in &self.elements {
                if e.range.start < range.end && e.range.end > range.start {
                    let new_start = range.start.min(e.range.start);
                    let new_end = range.end.max(e.range.end);
                    if new_start != range.start || new_end != range.end {
                        range.start = new_start;
                        range.end = new_end;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        range
    }

    fn shift_elements(&mut self, at: usize, removed: usize, inserted: usize) {
        // Generic shift: for pure insert, removed = 0; for delete, inserted = 0.
        let end = at + removed;
        let diff = inserted as isize - removed as isize;
        // Remove elements fully deleted by the operation and shift the rest
        self.elements
            .retain(|e| !(e.range.start >= at && e.range.end <= end));
        for e in &mut self.elements {
            if e.range.end <= at {
                // before edit
            } else if e.range.start >= end {
                // after edit
                e.range.start = ((e.range.start as isize) + diff) as usize;
                e.range.end = ((e.range.end as isize) + diff) as usize;
            } else {
                // Overlap with element but not fully contained (shouldn't happen when using
                // element-aware replace, but degrade gracefully by snapping element to new bounds)
                let new_start = at.min(e.range.start);
                let new_end = at + inserted.max(e.range.end.saturating_sub(end));
                e.range.start = new_start;
                e.range.end = new_end;
            }
        }
    }

    fn update_elements_after_replace(&mut self, start: usize, end: usize, inserted_len: usize) {
        self.shift_elements(start, end.saturating_sub(start), inserted_len);
    }

    /// Move to the beginning of the previous navigable chunk.
    ///
    /// Word characters are alphanumeric plus `_`. Punctuation runs (such as
    /// `-`) are their own chunk, so moving left across `aa-bb` stops at the
    /// right side of `-`, then the left side of `-`, then the start of `aa`.
    /// Whitespace is skipped over. Elements remain atomic units.
    pub fn beginning_of_previous_word(&self) -> usize {
        let ranges = self.element_ranges();
        self.text
            .plan_command(EditCommand::MoveWordLeft(WordStyle::Small), &ranges)
            .cursor_byte()
    }

    /// Start of the previous whitespace-delimited WORD; elements count as
    /// non-whitespace.
    pub fn beginning_of_previous_unix_word(&self) -> usize {
        let ranges = self.element_ranges();
        self.text
            .plan_command(
                EditCommand::MoveWordLeft(WordStyle::WhitespaceDelimited),
                &ranges,
            )
            .cursor_byte()
    }

    /// Move to the end of the next navigable chunk.
    ///
    /// Word characters are alphanumeric plus `_`. Punctuation runs (such as
    /// `-`) are their own chunk, so moving right across `aa-bb` stops at the
    /// left side of `-`, then the right side of `-`, then the end of `bb`.
    /// Whitespace is skipped over. Elements remain atomic units.
    pub fn end_of_next_word(&self) -> usize {
        let ranges = self.element_ranges();
        self.text
            .plan_command(EditCommand::MoveWordRight(WordStyle::Small), &ranges)
            .cursor_byte()
    }

    fn adjust_pos_out_of_elements(&self, pos: usize, prefer_start: bool) -> usize {
        if let Some(idx) = self.find_element_containing(pos) {
            let e = &self.elements[idx];
            if prefer_start {
                e.range.start
            } else {
                e.range.end
            }
        } else {
            pos
        }
    }

    #[expect(clippy::unwrap_used)]
    fn wrapped_lines(&self, width: u16) -> Ref<'_, Vec<Range<usize>>> {
        // A zero-width terminal must not reach textwrap — it can produce
        // borrowed empty slices that don't point into the input buffer,
        // causing out-of-bounds panics in wrap_ranges pointer arithmetic.
        let width = width.max(1);
        // Ensure cache is ready (potentially mutably borrow, then drop)
        {
            let mut cache = self.wrap_cache.borrow_mut();
            let needs_recalc = match cache.as_ref() {
                Some(c) => c.width != width,
                None => true,
            };
            if needs_recalc {
                let lines = if self.elements.iter().any(|e| e.display.is_some()) {
                    self.element_aware_wrap_ranges(width as usize)
                } else {
                    crate::wrapping::wrap_ranges(
                        &self.text,
                        Options::new(width as usize)
                            .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
                    )
                };
                *cache = Some(WrapCache { width, lines });
            }
        }

        let cache = self.wrap_cache.borrow();
        Ref::map(cache, |c| &c.as_ref().unwrap().lines)
    }

    /// Element-display-aware greedy wrapping.
    ///
    /// Produces wrap ranges where each range is `start..end` with `end` being
    /// the exclusive byte position of the content (including any trailing
    /// spaces that belong to this visual line).
    ///
    /// Elements are treated as atomic units for wrapping: if an element's
    /// display width doesn't fit on the current line, the element is moved
    /// to a new line (like word-wrap). If it doesn't fit on *any* line
    /// (wider than terminal), it gets its own line and rendering truncates it.
    fn element_aware_wrap_ranges(&self, width: usize) -> Vec<Range<usize>> {
        let width = width.max(1);
        let mut result = Vec::new();

        // Process each logical line (split by \n), but skip \n inside elements.
        // Newlines inside elements are internal to the element and must not create
        // visual line breaks — the element's display is a single-line chip.
        let mut seg_start = 0;
        loop {
            let seg_end = self.next_logical_newline(seg_start);

            self.greedy_wrap_segment(seg_start, seg_end, width, &mut result);

            if seg_end >= self.text.len() {
                break;
            }
            seg_start = seg_end + 1; // skip \n
        }

        if result.is_empty() {
            result.push(0..0);
        }

        result
    }

    /// Find the next `\n` at or after `from` that is NOT inside an element.
    ///
    /// Returns `self.text.len()` if no such newline exists.
    fn next_logical_newline(&self, from: usize) -> usize {
        let mut pos = from;
        while pos < self.text.len() {
            // If pos is inside an element, skip past the entire element.
            if let Some(elem) = self
                .elements
                .iter()
                .find(|e| pos >= e.range.start && pos < e.range.end)
            {
                pos = elem.range.end;
                continue;
            }
            if self.text.as_bytes()[pos] == b'\n' {
                return pos;
            }
            pos += 1;
        }
        self.text.len()
    }

    /// Greedy-wrap a single logical line (no \n inside `start..end`).
    fn greedy_wrap_segment(
        &self,
        start: usize,
        end: usize,
        width: usize,
        result: &mut Vec<Range<usize>>,
    ) {
        if start >= end {
            // Empty logical line
            result.push(start..end);
            return;
        }

        let mut line_start = start;
        let mut pos = start;
        let mut display_w: usize = 0;
        // Position right after the last break opportunity (start of the next word/element).
        let mut last_break_pos: Option<usize> = None;

        while pos < end {
            // Check if pos is at the start of an element
            if let Some(elem) = self
                .elements
                .iter()
                .find(|e| pos == e.range.start && e.range.start < e.range.end)
            {
                let elem_end = elem.range.end.min(end);
                let elem_dw: usize = if let Some(display) = &elem.display {
                    display
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref().width())
                        .sum()
                } else {
                    self.plain_display_width(&self.text[elem.range.start..elem_end])
                };

                if display_w > 0 && display_w + elem_dw > width {
                    // Element doesn't fit on current line — break before it.
                    let break_at = last_break_pos.unwrap_or(pos);
                    result.push(line_start..break_at);
                    line_start = break_at;
                    // Skip leading spaces/tabs on the new line
                    while line_start < end
                        && line_start < pos
                        && matches!(self.text.as_bytes().get(line_start), Some(b' ' | b'\t'))
                    {
                        line_start += 1;
                    }
                    pos = line_start;
                    display_w = 0;
                    last_break_pos = None;
                    continue;
                }

                display_w += elem_dw;
                pos = elem_end;
                // After element is a break opportunity
                last_break_pos = Some(pos);
                continue;
            }

            // Plain text grapheme cluster
            let slice = &self.text[pos..end];
            let Some(grapheme) = slice.graphemes(true).next() else {
                break;
            };
            let grapheme_width = self.grapheme_display_width(grapheme);

            if display_w + grapheme_width > width && display_w > 0 {
                // Need to wrap
                let break_at = last_break_pos.unwrap_or(pos);
                result.push(line_start..break_at);
                line_start = break_at;
                // Skip leading spaces/tabs on the new line
                while line_start < end
                    && line_start < pos
                    && matches!(self.text.as_bytes().get(line_start), Some(b' ' | b'\t'))
                {
                    line_start += 1;
                }
                display_w = self.display_width_of_range(line_start, pos);
                last_break_pos = None;
                if line_start == pos {
                    // No break opportunity found; break at current position (break_words).
                    display_w = grapheme_width;
                    pos += grapheme.len();
                }
                continue;
            }

            if grapheme == " " || grapheme == "\t" {
                // Space/tab is a break opportunity; break point is after it.
                last_break_pos = Some(pos + grapheme.len());
            }

            display_w += grapheme_width;
            pos += grapheme.len();
        }

        // Final visual line of this logical line
        result.push(line_start..end);
    }

    /// Calculate the scroll offset that should be used to satisfy the
    /// invariants given the current area size and wrapped lines.
    ///
    /// - Cursor is always on screen.
    /// - No scrolling if content fits in the area.
    fn effective_scroll(
        &self,
        area_height: u16,
        lines: &[Range<usize>],
        current_scroll: u16,
    ) -> u16 {
        let total_lines = lines.len() as u16;
        if area_height >= total_lines {
            return 0;
        }

        let max_scroll = total_lines.saturating_sub(area_height);

        // If we have an internal scroll override (from mousewheel), use it
        // — but still clamp to valid range.
        if let Some(ovr) = self.scroll_override {
            return ovr.min(max_scroll);
        }

        // Where is the cursor within wrapped lines? Prefer assigning boundary positions
        // (where pos equals the start of a wrapped line) to that later line.
        let cursor_line_idx =
            Self::wrapped_line_index_by_start(lines, self.cursor()).unwrap_or(0) as u16;

        let mut scroll = current_scroll.min(max_scroll);

        // Ensure cursor is visible within [scroll, scroll + area_height)
        if cursor_line_idx < scroll {
            scroll = cursor_line_idx;
        } else if cursor_line_idx >= scroll + area_height {
            scroll = cursor_line_idx + 1 - area_height;
        }
        scroll
    }

    /// Compute the effective content width for text wrapping, accounting for
    /// the scrollbar column.  Uses a 2-shot approach:
    ///
    /// 1. Wrap at full `area_width` to get line count.
    /// 2. If scrollbar needed (lines > height) and `show_scrollbar`, reduce
    ///    width by 1 for the scrollbar track.
    ///
    /// Returns `(content_width, needs_scrollbar)`.
    fn content_width(&self, area_width: u16, area_height: u16) -> (u16, bool) {
        if !self.show_scrollbar || area_width <= 1 {
            return (area_width, false);
        }
        // First shot — wrap at full width to check if content overflows.
        let lines = self.wrapped_lines(area_width);
        let needs = lines.len() as u16 > area_height;
        if needs {
            // 1 for scrollbar track + padding gap
            let reserved = 1 + self.scrollbar_padding;
            (area_width.saturating_sub(reserved), true)
        } else {
            (area_width, false)
        }
    }

    /// Convenience: content width for wrapping (area width minus scrollbar if needed).
    fn text_width(&self, area: Rect) -> u16 {
        self.content_width(area.width, area.height).0
    }
}
