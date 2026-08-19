impl TextArea {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn is_word_char(ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_'
    }

    /// Classify a character into a word-class for double-click selection.
    ///
    /// Three classes (matching vim/neovim `w` word definition):
    /// - `0`: whitespace
    /// - `1`: word chars (alphanumeric + underscore)
    /// - `2`: punctuation / everything else
    fn char_class(ch: char) -> u8 {
        if ch.is_whitespace() {
            0
        } else if Self::is_word_char(ch) {
            1
        } else {
            2
        }
    }

    /// Find the start of the word containing `pos` (for double-click selection).
    ///
    /// Uses vim-style word classes: word chars (alphanumeric + `_`), punctuation,
    /// and whitespace are three distinct groups.  Scans backward until the class
    /// changes.
    ///
    /// If `pos` is inside an element, returns the element start.
    fn word_start_at(&self, pos: usize) -> usize {
        // If inside an element, return element start.
        if let Some(elem) = self
            .elements
            .iter()
            .find(|e| pos >= e.range.start && pos < e.range.end)
        {
            return elem.range.start;
        }

        // Determine the class of the character at `pos` (or just before if at end).
        let target_class = if pos < self.text.len() {
            Self::char_class(self.text[pos..].chars().next().unwrap())
        } else if pos > 0 {
            let ch = self.text[..pos].chars().next_back().unwrap();
            Self::char_class(ch)
        } else {
            return 0;
        };

        let before = &self.text[..pos];
        let word_start = before
            .char_indices()
            .rev()
            .find(|&(_, ch)| Self::char_class(ch) != target_class)
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        self.adjust_pos_out_of_elements(word_start, true)
    }

    /// Find the end of the word containing `pos` (for double-click selection).
    ///
    /// Uses vim-style word classes (see [`Self::char_class`]).
    ///
    /// If `pos` is inside an element, returns the element end.
    fn word_end_at(&self, pos: usize) -> usize {
        // If inside an element, return element end.
        if let Some(elem) = self
            .elements
            .iter()
            .find(|e| pos >= e.range.start && pos < e.range.end)
        {
            return elem.range.end;
        }

        // Determine the class of the character at `pos`.
        let target_class = if pos < self.text.len() {
            Self::char_class(self.text[pos..].chars().next().unwrap())
        } else {
            return self.text.len();
        };

        let after = &self.text[pos..];
        let word_end = after
            .char_indices()
            .find(|&(_, ch)| Self::char_class(ch) != target_class)
            .map(|(rel_idx, _)| pos + rel_idx)
            .unwrap_or(self.text.len());
        self.adjust_pos_out_of_elements(word_end, false)
    }

    fn current_display_col(&self) -> usize {
        let bol = self.beginning_of_current_line();
        self.display_width_of_range(bol, self.cursor())
    }

    /// Compute the display width of the buffer range `[from..to)`.
    ///
    /// Plain runs use tab-aware width (`tab_width` columns per `\t`, or
    /// unicode-width when `tab_width == 0`). Element ranges with a custom
    /// `display` use the element's display width instead of the buffer text
    /// width. This is the core of the display projection system.
    fn display_width_of_range(&self, from: usize, to: usize) -> usize {
        if from >= to {
            return 0;
        }
        let mut width = 0usize;
        let mut pos = from;

        for elem in &self.elements {
            if elem.range.start >= to {
                break; // elements are sorted, no more overlap possible
            }
            if elem.range.end <= pos {
                continue; // element is entirely before our current position
            }

            // Plain text before this element
            if pos < elem.range.start {
                let plain_end = elem.range.start.min(to);
                width += self.plain_display_width(&self.text[pos..plain_end]);
                pos = plain_end;
            }
            if pos >= to {
                break;
            }

            // Element region
            let elem_start_in_range = elem.range.start.max(pos);
            let elem_end_in_range = elem.range.end.min(to);
            if elem_start_in_range < elem_end_in_range {
                if let Some(display) = &elem.display {
                    // If the range covers the entire element (or starts at element start),
                    // use the full display width. If it covers only a partial overlap
                    // (cursor inside element — shouldn't happen normally), fall back to
                    // buffer text width.
                    if elem_start_in_range == elem.range.start {
                        let display_w: usize = display
                            .spans
                            .iter()
                            .map(|s| s.content.as_ref().width())
                            .sum();
                        width += display_w;
                    } else {
                        width += self.plain_display_width(
                            &self.text[elem_start_in_range..elem_end_in_range],
                        );
                    }
                } else {
                    width += self
                        .plain_display_width(&self.text[elem_start_in_range..elem_end_in_range]);
                }
                pos = elem_end_in_range;
            }
        }

        // Remaining plain text after all elements
        if pos < to {
            width += self.plain_display_width(&self.text[pos..to]);
        }

        width
    }

    fn wrapped_line_index_by_start(lines: &[Range<usize>], pos: usize) -> Option<usize> {
        // partition_point returns the index of the first element for which
        // the predicate is false, i.e. the count of elements with start <= pos.
        let idx = lines.partition_point(|r| r.start <= pos);
        if idx == 0 { None } else { Some(idx - 1) }
    }

    /// Map a display column to a buffer byte position on a given wrapped line.
    ///
    /// Pure query — does not mutate any state. Handles elements (snapping to
    /// nearest element boundary) and wide unicode graphemes.
    /// If `target_col` is past the line's display width, returns `line_end`
    /// (clamped to the nearest element boundary).
    ///
    /// Returns `(byte_pos, hit_element)` where `hit_element` is `true` when
    /// the column fell on an element's display region.
    fn display_col_to_buffer_pos(
        &self,
        line_start: usize,
        line_end: usize,
        target_col: usize,
    ) -> (usize, bool) {
        let mut width_so_far = 0usize;
        let mut pos = line_start;

        while pos < line_end {
            // Check if pos is at or inside an element
            if let Some(elem_idx) = self
                .elements
                .iter()
                .position(|e| pos >= e.range.start && pos < e.range.end)
            {
                let elem = &self.elements[elem_idx];
                let elem_start = elem.range.start;
                let elem_buf_end = elem.range.end;
                // The visible portion of the element on this line
                let elem_line_end = elem_buf_end.min(line_end);

                if pos == elem_start {
                    // We're at the start of an element — treat it as a whole unit.
                    let elem_display_w = if let Some(display) = &elem.display {
                        display
                            .spans
                            .iter()
                            .map(|s| s.content.as_ref().width())
                            .sum()
                    } else {
                        self.plain_display_width(&self.text[elem_start..elem_line_end])
                    };

                    if width_so_far + elem_display_w > target_col {
                        // Click landed on this element display — snap to the
                        // nearer boundary (start vs end of the underlying
                        // buffer text) so that drag-selection works naturally.
                        let dist_start = target_col.saturating_sub(width_so_far);
                        let dist_end = elem_display_w.saturating_sub(dist_start);
                        if dist_start <= dist_end {
                            return (elem_start, true);
                        } else {
                            return (elem_buf_end, true);
                        }
                    }
                    width_so_far += elem_display_w;
                    pos = elem_buf_end.min(line_end); // move past element (or to line end)
                } else {
                    // We're in the middle of an element (e.g. a wrapped line starts
                    // mid-element). Skip past the rest of the element on this line.
                    let partial_w = self.plain_display_width(&self.text[pos..elem_line_end]);
                    if width_so_far + partial_w > target_col {
                        // Snap to element's actual end boundary
                        return (elem_buf_end, true);
                    }
                    width_so_far += partial_w;
                    pos = elem_buf_end.min(line_end); // move past element (or to line end)
                }
                continue;
            }

            // Plain text grapheme
            let slice = &self.text[pos..line_end];
            if let Some(grapheme) = slice.graphemes(true).next() {
                let grapheme_width = self.grapheme_display_width(grapheme);
                width_so_far += grapheme_width;
                if width_so_far > target_col {
                    return (self.clamp_pos_to_nearest_boundary(pos), false);
                }
                pos += grapheme.len();
            } else {
                break;
            }
        }

        (self.clamp_pos_to_nearest_boundary(line_end), false)
    }

    fn move_to_display_col_on_line(
        &mut self,
        line_start: usize,
        line_end: usize,
        target_col: usize,
    ) {
        let cursor = self
            .display_col_to_buffer_pos(line_start, line_end, target_col)
            .0;
        self.set_cursor_inner(cursor);
    }

    fn beginning_of_line(&self, pos: usize) -> usize {
        // Scan backward for '\n' that is NOT inside an element.
        // Newlines inside elements (e.g. multi-line paste) are not line boundaries.
        for i in (0..pos).rev() {
            if self.text.as_bytes()[i] == b'\n' && !self.is_inside_element(i) {
                return i + 1;
            }
        }
        0
    }
    fn beginning_of_current_line(&self) -> usize {
        self.beginning_of_line(self.cursor())
    }

    fn end_of_line(&self, pos: usize) -> usize {
        // Scan forward for '\n' that is NOT inside an element.
        for i in pos..self.text.len() {
            if self.text.as_bytes()[i] == b'\n' && !self.is_inside_element(i) {
                return i;
            }
        }
        self.text.len()
    }
    fn end_of_current_line(&self) -> usize {
        self.end_of_line(self.cursor())
    }

    /// Check if a byte position is inside (strictly within) an element.
    fn is_inside_element(&self, pos: usize) -> bool {
        self.elements
            .iter()
            .any(|e| pos >= e.range.start && pos < e.range.end)
    }

    fn apply_classified_command(&mut self, command: EditCommand) {
        if let EditCommand::Insert(character) = command {
            self.insert_str(&character.to_string());
            return;
        }
        let mutation_kind = match command.category() {
            EditCommandCategory::Insert => unreachable!("insert commands return above"),
            EditCommandCategory::Navigation => None,
            EditCommandCategory::Delete => Some(MutationKind::Delete),
            EditCommandCategory::Kill => Some(MutationKind::Kill),
        };
        self.apply_edit_command(command, mutation_kind);
    }

    pub fn input(&mut self, event: KeyEvent) {
        // ── Selection-aware interception ──
        // When a selection is active, certain keys interact with the selected
        // range rather than performing their normal single-char action.
        if self.selection.is_some() {
            if let Some(EditCommand::Insert(character)) = classify_key_event(&event) {
                self.begin_undo_group();
                if !self.delete_selection() {
                    self.clear_selection();
                }
                self.insert_str(&character.to_string());
                self.end_undo_group();
                return;
            }
            match event {
                // Enter / Ctrl-J/M → replace selection with newline.
                KeyEvent {
                    code: KeyCode::Char('j' | 'm'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Enter,
                    ..
                } => {
                    self.begin_undo_group();
                    if !self.delete_selection() {
                        self.clear_selection();
                    }
                    self.insert_str("\n");
                    self.end_undo_group();
                    return;
                }
                // Backspace / Delete → delete the selection only (no extra char).
                // If the selection is zero-width (anchor == head), delete_selection()
                // returns false — clear the stale selection and fall through to the
                // normal single-char delete so Backspace/Delete aren't silently swallowed.
                KeyEvent {
                    code: KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\x08' | '\x7f'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('h'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('d'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    if self.delete_selection() {
                        return;
                    }
                    // Zero-width selection — clear and fall through.
                    self.clear_selection();
                }
                // Ctrl-X → cut selection (copy to clipboard + delete).
                KeyEvent {
                    code: KeyCode::Char('x'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    if let Some(text) = self.selected_text() {
                        self.set_clipboard_text(text);
                    }
                    if self.delete_selection() {
                        return;
                    }
                    // Zero-width selection — clear and fall through.
                    self.clear_selection();
                }
                // All other keys → clear selection, fall through to normal handling.
                _ => {
                    self.clear_selection();
                }
            }
        }

        if let Some(command) = classify_key_event(&event) {
            self.apply_classified_command(command);
            return;
        }

        match event {
            KeyEvent {
                code: KeyCode::Char('j' | 'm'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }
            | KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.insert_str("\n"),
            KeyEvent {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.yank();
            }

            // Undo / Redo (Ctrl or Cmd)
            KeyEvent {
                code: KeyCode::Char('Z'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL)
                || modifiers.contains(KeyModifiers::SUPER) =>
            {
                // Ctrl/Cmd-Shift-Z → redo (terminals that report uppercase Z + Shift)
                self.redo();
            }
            k if is_undo_input(&k) => {
                self.undo();
            }
            KeyEvent {
                code: KeyCode::Char('r'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.redo();
            }

            // Ctrl-V → paste from clipboard provider.
            KeyEvent {
                code: KeyCode::Char('v'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                if let Some(text) = self.clipboard_provider.get() {
                    self.insert_str(&text);
                }
            }

            // Cmd+Left / Cmd+Right (macOS): terminals using the Kitty keyboard
            // protocol (Ghostty, Kitty, WezTerm) send these as Super+Arrow.
            KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::SUPER,
                ..
            } => {
                self.move_cursor_to_beginning_of_line(false);
            }
            KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::SUPER,
                ..
            } => {
                self.move_cursor_to_end_of_line(false);
            }
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_up();
            }
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('n'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                self.move_cursor_down();
            }
            // Home/End → logical line (full left/right even when soft-wrapped).
            // Super+Left/Right stay on the visual wrap row; Ctrl+A/E chain
            // across logical lines when already at BOL/EOL.
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                self.set_cursor(self.beginning_of_current_line());
            }

            KeyEvent {
                code: KeyCode::End, ..
            } => {
                self.set_cursor(self.end_of_current_line());
            }
            // `tracing` was dropped in the theway port (not in the port's
            // dependency set); unhandled keys are intentionally silent.
            _o => {}
        }
    }
}
