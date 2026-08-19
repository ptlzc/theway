impl TextArea {
    // ── Undo/Redo ──

    /// Create a snapshot of the current textarea state.
    fn snapshot(&self) -> UndoEntry {
        UndoEntry {
            text: self.text().to_owned(),
            cursor: self.cursor(),
            elements: self.elements.clone(),
        }
    }

    /// Restore the textarea state from a snapshot.
    fn restore(&mut self, entry: UndoEntry) {
        self.text = EditBuffer::from_parts(entry.text, entry.cursor);
        self.elements = entry.elements;
        self.wrap_cache.replace(None);
        self.preferred_col = None;
        // Note: next_element_id is intentionally NOT restored — it only increases.
        // Note: kill_buffer is intentionally NOT restored — yank is separate from undo.
    }

    /// Called before a mutation to decide whether to push a new undo checkpoint.
    ///
    /// Batching rules:
    /// - Inside an undo group (`group_depth > 0`) → skip entirely.
    /// - First mutation ever → always checkpoint.
    /// - Kind changed from last → checkpoint.
    /// - Cursor moved since last mutation (arrows, clicks) → checkpoint.
    /// - Kill / Element / Replace → always checkpoint (discrete actions).
    /// - Same Insert or Delete with consecutive cursor → extend batch (no checkpoint).
    /// - Word boundary (ws↔non-ws transition) → checkpoint (handled by callers
    ///   resetting `last_kind` before calling this method).
    fn pre_mutate(&mut self, kind: MutationKind) {
        // Inside an undo group — the group handles its own checkpoint.
        if self.undo.group_depth > 0 {
            return;
        }

        let should_push = match self.undo.last_kind {
            None => true,
            Some(prev) => {
                prev != kind
                    || self.cursor() != self.undo.last_cursor
                    || matches!(
                        kind,
                        MutationKind::Kill | MutationKind::Element | MutationKind::Replace
                    )
            }
        };

        if should_push {
            let entry = self.snapshot();
            self.undo.stack.push(entry);
            if self.undo.stack.len() > self.undo.max_depth {
                self.undo.stack.remove(0);
            }
        }
        self.undo.redo.clear();
        self.undo.last_kind = Some(kind);
    }

    /// Update `last_cursor` after a mutation completes so the next `pre_mutate`
    /// can detect cursor jumps.
    fn post_mutate(&mut self) {
        self.undo.last_cursor = self.cursor();
    }

    /// Clear the undo/redo history, leaving the current text and cursor
    /// untouched.
    ///
    /// Use this when a buffer is reset to represent a *new logical
    /// context* — e.g. a shared input widget that is reused for a
    /// different target — so that a later `undo` can't resurrect text
    /// that belonged to the previous context. `set_text` deliberately
    /// records a checkpoint (so an accidental replace is undoable), so
    /// callers that want a hard reset must follow it with this.
    pub fn clear_history(&mut self) {
        self.undo.stack.clear();
        self.undo.redo.clear();
        self.undo.last_kind = None;
        self.undo.last_cursor = self.cursor();
    }

    /// Undo the last mutation. Returns `true` if there was something to undo.
    pub fn undo(&mut self) -> bool {
        if let Some(entry) = self.undo.stack.pop() {
            self.scroll_override = None;
            let current = self.snapshot();
            self.undo.redo.push(current);
            self.restore(entry);
            // Reset batching — next mutation starts a fresh group.
            self.undo.last_kind = None;
            self.undo.last_cursor = self.cursor();
            true
        } else {
            false
        }
    }

    /// Redo the last undone mutation. Returns `true` if there was something to redo.
    pub fn redo(&mut self) -> bool {
        if let Some(entry) = self.undo.redo.pop() {
            self.scroll_override = None;
            let current = self.snapshot();
            self.undo.stack.push(current);
            self.restore(entry);
            // Reset batching — next mutation starts a fresh group.
            self.undo.last_kind = None;
            self.undo.last_cursor = self.cursor();
            true
        } else {
            false
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undo.redo.is_empty()
    }

    /// Begin an undo group. All mutations between `begin_undo_group()` and
    /// `end_undo_group()` are collapsed into a single undo step.
    ///
    /// Groups can be nested: only the outermost `end_undo_group()` pushes
    /// the checkpoint. Inner begin/end pairs are reference-counted.
    ///
    /// Use cases:
    /// - Autocomplete: `replace_range_with_element` + `insert_str(" ")` = 1 undo step
    /// - Line-select: enter → N live-updates → confirm = 1 undo step
    pub fn begin_undo_group(&mut self) {
        if self.undo.group_depth == 0 {
            // Outermost group — take the snapshot.
            self.undo.group_checkpoint = Some(self.snapshot());
        }
        self.undo.group_depth += 1;
    }

    /// End an undo group. If this closes the outermost group and the state
    /// actually changed, a single undo entry is pushed.
    pub fn end_undo_group(&mut self) {
        if self.undo.group_depth == 0 {
            return; // Unbalanced call — ignore.
        }
        self.undo.group_depth -= 1;
        if self.undo.group_depth == 0 {
            if let Some(checkpoint) = self.undo.group_checkpoint.take() {
                // Only push if state actually changed.
                let changed = checkpoint.text.as_str() != self.text()
                    || checkpoint.cursor != self.cursor()
                    || checkpoint.elements.len() != self.elements.len();
                if changed {
                    self.undo.stack.push(checkpoint);
                    if self.undo.stack.len() > self.undo.max_depth {
                        self.undo.stack.remove(0);
                    }
                    self.undo.redo.clear();
                }
            }
            // Reset batching state so the next mutation starts fresh.
            self.undo.last_kind = None;
            self.undo.last_cursor = self.cursor();
        }
    }

    /// Cancel an undo group. Restores the textarea to the state it was in
    /// when `begin_undo_group()` was called — no undo entry is created.
    ///
    /// Use case: line-select cancel → revert all live-updates, leave no trace.
    pub fn cancel_undo_group(&mut self) {
        if self.undo.group_depth == 0 {
            return; // Unbalanced call — ignore.
        }
        // Always restore to the outermost checkpoint, regardless of nesting.
        self.undo.group_depth = 0;
        if let Some(checkpoint) = self.undo.group_checkpoint.take() {
            self.restore(checkpoint);
        }
        // Reset batching state.
        self.undo.last_kind = None;
        self.undo.last_cursor = self.cursor();
    }

    // ####### Input Functions #######
    pub fn delete_backward(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        if n == 1 {
            self.apply_edit_command(
                EditCommand::DeleteGraphemeBackward,
                Some(MutationKind::Delete),
            );
            return;
        }
        self.begin_undo_group();
        for _ in 0..n {
            if matches!(
                self.apply_edit_command(
                    EditCommand::DeleteGraphemeBackward,
                    Some(MutationKind::Delete),
                ),
                EditOutcome::Unchanged
            ) {
                break;
            }
        }
        self.end_undo_group();
    }

    pub fn delete_forward(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        if n == 1 {
            self.apply_edit_command(
                EditCommand::DeleteGraphemeForward,
                Some(MutationKind::Delete),
            );
            return;
        }
        self.begin_undo_group();
        for _ in 0..n {
            if matches!(
                self.apply_edit_command(
                    EditCommand::DeleteGraphemeForward,
                    Some(MutationKind::Delete),
                ),
                EditOutcome::Unchanged
            ) {
                break;
            }
        }
        self.end_undo_group();
    }

    pub fn delete_backward_word(&mut self) {
        self.apply_edit_command(
            EditCommand::DeleteWordBackward(WordStyle::Small),
            Some(MutationKind::Kill),
        );
    }

    /// readline `unix-word-rubout` (whitespace-delimited), vs
    /// [`Self::delete_backward_word`]'s punctuation-chunked M-DEL semantics.
    pub fn delete_backward_unix_word(&mut self) {
        self.apply_edit_command(
            EditCommand::DeleteWordBackward(WordStyle::WhitespaceDelimited),
            Some(MutationKind::Kill),
        );
    }

    /// Delete text to the right of the cursor using readline-style word semantics.
    ///
    /// Deletes from the current cursor position through the end of the next word as determined
    /// by `end_of_next_word()`. Any delimiters between the cursor and that word
    /// (whitespace, punctuation, newlines) are included in the deletion.
    pub fn delete_forward_word(&mut self) {
        self.apply_edit_command(
            EditCommand::DeleteWordForward(WordStyle::Small),
            Some(MutationKind::Kill),
        );
    }

    pub fn kill_to_end_of_line(&mut self) {
        self.apply_edit_command(EditCommand::DeleteToLineEnd, Some(MutationKind::Kill));
    }

    pub fn kill_to_beginning_of_line(&mut self) {
        self.apply_edit_command(EditCommand::DeleteToLineStart, Some(MutationKind::Kill));
    }

    /// Kill the entire current line (BOL to EOL), regardless of cursor position.
    /// If the line is already empty, consumes the preceding newline to join lines.
    pub fn kill_current_line(&mut self) {
        let bol = self.beginning_of_current_line();
        let eol = self.end_of_current_line();

        let range = if bol == eol {
            if bol > 0 { Some(bol - 1..bol) } else { None }
        } else {
            Some(bol..eol)
        };

        if let Some(range) = range {
            self.apply_edit_replacement(range, "", Some(MutationKind::Kill));
        }
    }

    pub fn yank(&mut self) {
        if self.kill_buffer.is_empty() {
            return;
        }
        let text = self.kill_buffer.clone();
        self.apply_edit_replacement(
            self.cursor()..self.cursor(),
            &text,
            Some(MutationKind::Insert),
        );
        if let Some(last) = text.chars().last() {
            self.undo.last_insert_ws = last.is_whitespace();
        }
    }

    /// Move the cursor left by a single grapheme cluster.
    pub fn move_cursor_left(&mut self) {
        self.apply_edit_command(EditCommand::MoveGraphemeLeft, None);
    }

    /// Move the cursor right by a single grapheme cluster.
    pub fn move_cursor_right(&mut self) {
        self.apply_edit_command(EditCommand::MoveGraphemeRight, None);
    }

    pub fn move_cursor_up(&mut self) {
        self.scroll_override = None;
        // If we have a wrapping cache, prefer navigating across wrapped (visual) lines.
        if let Some((target_col, maybe_line)) = {
            let cache_ref = self.wrap_cache.borrow();
            if let Some(cache) = cache_ref.as_ref() {
                let lines = &cache.lines;
                if let Some(idx) = Self::wrapped_line_index_by_start(lines, self.cursor()) {
                    let cur_range = &lines[idx];
                    let target_col = self.preferred_col.unwrap_or_else(|| {
                        self.display_width_of_range(cur_range.start, self.cursor())
                    });
                    if idx > 0 {
                        let prev = &lines[idx - 1];
                        let line_start = prev.start;
                        let line_end = prev.end;
                        Some((target_col, Some((line_start, line_end))))
                    } else {
                        Some((target_col, None))
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } {
            // We had wrapping info. Apply movement accordingly.
            match maybe_line {
                Some((line_start, line_end)) => {
                    if self.preferred_col.is_none() {
                        self.preferred_col = Some(target_col);
                    }
                    self.move_to_display_col_on_line(line_start, line_end, target_col);
                    return;
                }
                None => {
                    // Already at first visual line -> move to start
                    self.set_cursor_inner(0);
                    self.preferred_col = None;
                    return;
                }
            }
        }

        // Fallback to logical line navigation if we don't have wrapping info yet.
        if let Some(prev_nl) = self.text[..self.cursor()].rfind('\n') {
            let target_col = match self.preferred_col {
                Some(c) => c,
                None => {
                    let c = self.current_display_col();
                    self.preferred_col = Some(c);
                    c
                }
            };
            let prev_line_start = self.text[..prev_nl].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let prev_line_end = prev_nl;
            self.move_to_display_col_on_line(prev_line_start, prev_line_end, target_col);
        } else {
            self.set_cursor_inner(0);
            self.preferred_col = None;
        }
    }

    pub fn move_cursor_down(&mut self) {
        self.scroll_override = None;
        // If we have a wrapping cache, prefer navigating across wrapped (visual) lines.
        if let Some((target_col, move_to_last)) = {
            let cache_ref = self.wrap_cache.borrow();
            if let Some(cache) = cache_ref.as_ref() {
                let lines = &cache.lines;
                if let Some(idx) = Self::wrapped_line_index_by_start(lines, self.cursor()) {
                    let cur_range = &lines[idx];
                    let target_col = self.preferred_col.unwrap_or_else(|| {
                        self.display_width_of_range(cur_range.start, self.cursor())
                    });
                    if idx + 1 < lines.len() {
                        let next = &lines[idx + 1];
                        let line_start = next.start;
                        let line_end = next.end;
                        Some((target_col, Some((line_start, line_end))))
                    } else {
                        Some((target_col, None))
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } {
            match move_to_last {
                Some((line_start, line_end)) => {
                    if self.preferred_col.is_none() {
                        self.preferred_col = Some(target_col);
                    }
                    self.move_to_display_col_on_line(line_start, line_end, target_col);
                    return;
                }
                None => {
                    // Already on last visual line -> move to end
                    self.set_cursor_inner(self.text.len());
                    self.preferred_col = None;
                    return;
                }
            }
        }

        // Fallback to logical line navigation if we don't have wrapping info yet.
        let target_col = match self.preferred_col {
            Some(c) => c,
            None => {
                let c = self.current_display_col();
                self.preferred_col = Some(c);
                c
            }
        };
        if let Some(next_nl) = self.text[self.cursor()..]
            .find('\n')
            .map(|i| i + self.cursor())
        {
            let next_line_start = next_nl + 1;
            let next_line_end = self.text[next_line_start..]
                .find('\n')
                .map(|i| i + next_line_start)
                .unwrap_or(self.text.len());
            self.move_to_display_col_on_line(next_line_start, next_line_end, target_col);
        } else {
            self.set_cursor_inner(self.text.len());
            self.preferred_col = None;
        }
    }

    /// Home / Super+Left when `move_up_at_bol` is false (visual row if wrapped);
    /// Ctrl+A when true (logical line; already-at-BOL chains to previous line).
    pub fn move_cursor_to_beginning_of_line(&mut self, move_up_at_bol: bool) {
        if move_up_at_bol {
            self.apply_edit_command(EditCommand::MoveLogicalLineStart, None);
            return;
        }
        if let Some(bol) = self.beginning_of_current_visual_line() {
            self.set_cursor(bol);
            return;
        }

        let bol = self.beginning_of_current_line();
        self.set_cursor(bol);
    }

    /// End / Super+Right when `move_down_at_eol` is false (visual row if wrapped);
    /// Ctrl+E when true (logical line; already-at-EOL chains to next line).
    pub fn move_cursor_to_end_of_line(&mut self, move_down_at_eol: bool) {
        if move_down_at_eol {
            self.apply_edit_command(EditCommand::MoveLogicalLineEnd, None);
            return;
        }
        if let Some(eol) = self.end_of_current_visual_line() {
            self.set_cursor(eol);
            return;
        }

        let eol = self.end_of_current_line();
        self.set_cursor(eol);
    }

    fn beginning_of_current_visual_line(&self) -> Option<usize> {
        let cache = self.wrap_cache.borrow();
        let cache = cache.as_ref()?;
        let idx = Self::wrapped_line_index_by_start(&cache.lines, self.cursor())?;
        Some(cache.lines[idx].start)
    }

    /// Soft-continued visual rows land on the last char (exclusive end is the
    /// next row's start). Final segment of a logical line uses exclusive end.
    fn end_of_current_visual_line(&self) -> Option<usize> {
        let cache = self.wrap_cache.borrow();
        let cache = cache.as_ref()?;
        let idx = Self::wrapped_line_index_by_start(&cache.lines, self.cursor())?;
        let line = &cache.lines[idx];
        let end = line.end.min(self.text.len());
        let soft_continued = cache
            .lines
            .get(idx + 1)
            .is_some_and(|next| next.start == end);
        if soft_continued && end > line.start {
            Some(self.clamp_to_line(end, line.start, end))
        } else {
            Some(end)
        }
    }
}
