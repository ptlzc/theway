impl DemoApp {
    fn handle_line_select_key(&mut self, key: KeyEvent) -> EventResult {
        // Post-action to execute after the borrow of self.line_select is released.
        enum Action {
            Noop,
            Cancel,
            Confirm,
            LiveUpdate(Option<RangeInclusive<usize>>),
        }

        let action = {
            let Some(mode) = self.line_select.as_mut() else {
                return EventResult::Redraw;
            };

            match key {
                // Cancel: Esc, q, Ctrl-C
                KeyEvent {
                    code: KeyCode::Esc, ..
                }
                | KeyEvent {
                    code: KeyCode::Char('q'),
                    modifiers: KeyModifiers::NONE,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => Action::Cancel,

                // Confirm: Enter
                KeyEvent {
                    code: KeyCode::Enter,
                    ..
                } => Action::Confirm,

                // v / V (Shift-V): toggle selection
                KeyEvent {
                    code: KeyCode::Char('v' | 'V'),
                    ..
                } => {
                    mode.toggle_selection();
                    // Only auto-clear on lock (second v), not while still selecting.
                    if matches!(mode.selection, SelectionState::Locked(..)) {
                        mode.check_select_all();
                    }
                    Action::LiveUpdate(mode.effective_range())
                }

                // g: first line
                KeyEvent {
                    code: KeyCode::Char('g'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => {
                    mode.goto_buf.clear();
                    mode.goto_line(1);
                    Action::Noop
                }
                // G: last line
                KeyEvent {
                    code: KeyCode::Char('G'),
                    ..
                } => {
                    mode.goto_buf.clear();
                    mode.goto_line(mode.total_lines());
                    Action::Noop
                }

                // j / Down: down 1
                KeyEvent {
                    code: KeyCode::Char('j'),
                    modifiers: KeyModifiers::NONE,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Down,
                    ..
                } => {
                    mode.goto_buf.clear();
                    mode.move_cursor(1);
                    Action::Noop
                }
                // k / Up: up 1
                KeyEvent {
                    code: KeyCode::Char('k'),
                    modifiers: KeyModifiers::NONE,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Up, ..
                } => {
                    mode.goto_buf.clear();
                    mode.move_cursor(-1);
                    Action::Noop
                }
                // Ctrl-D: half page down
                KeyEvent {
                    code: KeyCode::Char('d'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    mode.goto_buf.clear();
                    let half = (mode.viewport_height / 2).max(1) as isize;
                    mode.move_cursor(half);
                    Action::Noop
                }
                // Ctrl-U: half page up
                KeyEvent {
                    code: KeyCode::Char('u'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    mode.goto_buf.clear();
                    let half = (mode.viewport_height / 2).max(1) as isize;
                    mode.move_cursor(-half);
                    Action::Noop
                }
                // f: full page down
                KeyEvent {
                    code: KeyCode::Char('f'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => {
                    mode.goto_buf.clear();
                    let page = mode.viewport_height.max(1) as isize;
                    mode.move_cursor(page);
                    Action::Noop
                }
                // b: full page up
                KeyEvent {
                    code: KeyCode::Char('b'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => {
                    mode.goto_buf.clear();
                    let page = mode.viewport_height.max(1) as isize;
                    mode.move_cursor(-page);
                    Action::Noop
                }
                // Digits: accumulate goto-line input and jump immediately
                KeyEvent {
                    code: KeyCode::Char(c),
                    modifiers: KeyModifiers::NONE,
                    ..
                } if c.is_ascii_digit() => {
                    mode.goto_buf.push(c);
                    if let Ok(line) = mode.goto_buf.parse::<usize>() {
                        mode.goto_line(line);
                    }
                    Action::Noop
                }
                // Any other key clears goto buffer
                _ => {
                    mode.goto_buf.clear();
                    Action::Noop
                }
            }
        };

        // Execute action (line_select borrow is released).
        match action {
            Action::Cancel => {
                self.cancel_line_select();
                return EventResult::Redraw;
            }
            Action::Confirm => {
                self.confirm_line_select();
                return EventResult::Redraw;
            }
            Action::LiveUpdate(range) => {
                self.update_line_select_element(range.as_ref());
            }
            Action::Noop => {
                // Live-update element while actively selecting (range follows cursor).
                let selecting_range = self.line_select.as_ref().and_then(|m| {
                    if matches!(m.selection, SelectionState::Selecting(_)) {
                        m.effective_range()
                    } else {
                        None
                    }
                });
                if let Some(range) = selecting_range {
                    self.update_line_select_element(Some(&range));
                }
            }
        }

        // Update status.
        self.update_line_select_status();
        EventResult::Redraw
    }

    fn cancel_line_select(&mut self) {
        let Some(_mode) = self.line_select.take() else {
            return;
        };

        // Cancel the undo group — restores textarea to pre-line-select state.
        // No manual element revert needed.
        self.textarea.cancel_undo_group();

        self.status = "Line select cancelled.".into();
    }

    fn confirm_line_select(&mut self) {
        let Some(mode) = self.line_select.take() else {
            return;
        };

        let mut range = mode.effective_range();
        // Selecting all lines = entire file = no range needed.
        if let Some(ref r) = range
            && *r.start() == 1
            && *r.end() == mode.total_lines()
        {
            range = None;
        }

        let new_text = build_file_ref_text(&mode.file_path, range.as_ref());
        let new_display = build_file_ref_display(&mode.file_path, range.as_ref());

        // Find the element's current range in the buffer.
        let elem_range = self
            .textarea
            .elements()
            .iter()
            .find(|e| e.id == mode.element_id)
            .map(|e| e.range.clone());

        if let Some(elem_range) = elem_range {
            let new_id = self.textarea.replace_range_with_element(
                elem_range,
                &new_text,
                KIND_FILE_REF,
                Some(new_display),
            );

            let desc = match &range {
                Some(r) if r.start() == r.end() => {
                    format!("File: {}:{}", mode.file_path, r.start())
                }
                Some(r) => format!("File: {}:{}-{}", mode.file_path, r.start(), r.end()),
                None => format!("File: {}", mode.file_path),
            };
            self.element_meta.insert(
                new_id,
                ElementMeta {
                    description: desc.clone(),
                },
            );
            self.status = format!("Confirmed: {desc}");
        }

        // Close the undo group — all line-select mutations become 1 undo step.
        self.textarea.end_undo_group();
    }

    /// Live-update the element text/display to reflect a line range change.
    fn update_line_select_element(&mut self, range: Option<&RangeInclusive<usize>>) {
        // Extract data before mutating.
        let (file_path, old_element_id) = {
            let mode = self.line_select.as_ref().unwrap();
            (mode.file_path.clone(), mode.element_id)
        };

        let elem_range = self
            .textarea
            .elements()
            .iter()
            .find(|e| e.id == old_element_id)
            .map(|e| e.range.clone());
        let Some(elem_range) = elem_range else {
            return;
        };

        let new_text = build_file_ref_text(&file_path, range);
        let new_display = build_file_ref_display(&file_path, range);
        let new_id = self.textarea.replace_range_with_element(
            elem_range,
            &new_text,
            KIND_FILE_REF,
            Some(new_display),
        );

        let desc = match range {
            Some(r) if r.start() == r.end() => format!("File: {}:{}", file_path, r.start()),
            Some(r) => format!("File: {}:{}-{}", file_path, r.start(), r.end()),
            None => format!("File: {}", file_path),
        };
        self.element_meta
            .insert(new_id, ElementMeta { description: desc });

        // Update element_id in line_select so subsequent ops find the right element.
        if let Some(mode) = self.line_select.as_mut() {
            mode.element_id = new_id;
        }
    }

    fn update_line_select_status(&mut self) {
        if let Some(mode) = &self.line_select {
            let line_info = format!("L{}/{}", mode.cursor_line + 1, mode.total_lines());
            let sel_info = match mode.selection {
                SelectionState::None => String::new(),
                SelectionState::Selecting(anchor) => {
                    let (s, e) = sorted(anchor, mode.cursor_line);
                    format!(" | selecting {}‑{}", s + 1, e + 1)
                }
                SelectionState::Locked(s, e) => format!(" | locked {}‑{}", s + 1, e + 1),
            };
            let goto_info = if mode.goto_buf.is_empty() {
                String::new()
            } else {
                format!(" | :{}", mode.goto_buf)
            };
            self.status = format!("{} {line_info}{sel_info}{goto_info}", mode.file_path);
        }
    }

    // ── Rendering ──
}
