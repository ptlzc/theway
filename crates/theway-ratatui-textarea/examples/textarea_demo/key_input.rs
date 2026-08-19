impl DemoApp {
    fn handle_key(&mut self, key: KeyEvent) -> EventResult {
        // ── Line select mode takes ALL keys ──
        if self.line_select.is_some() {
            return self.handle_line_select_key(key);
        }

        // ── Global quit / clear ──
        match key {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                if self.fs_active {
                    self.fs_active = false;
                    self.file_search.clear();
                    self.status = "File search dismissed.".into();
                    return EventResult::Redraw;
                }
                return EventResult::Quit;
            }
            KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => return EventResult::Quit,
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                if self.textarea.is_empty() {
                    return EventResult::Quit;
                }
                self.textarea.set_text("");
                self.element_meta.clear();
                self.fs_active = false;
                self.file_search.clear();
                self.status = "Cleared.".into();
                return EventResult::Redraw;
            }
            _ => {}
        }

        // ── File search key interception (when dropdown is visible) ──
        if self.fs_active && self.file_search.is_visible() {
            match key {
                // ':' during file search → confirm file + open line select
                KeyEvent {
                    code: KeyCode::Char(':'),
                    modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                    ..
                } => {
                    self.enter_line_select_from_search();
                    return EventResult::Redraw;
                }
                KeyEvent {
                    code: KeyCode::Tab, ..
                }
                | KeyEvent {
                    code: KeyCode::Enter,
                    ..
                } => {
                    self.confirm_file_search();
                    return EventResult::Redraw;
                }
                KeyEvent {
                    code: KeyCode::Up, ..
                }
                | KeyEvent {
                    code: KeyCode::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    self.file_search.move_selection(-1);
                    return EventResult::Redraw;
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
                    self.file_search.move_selection(1);
                    return EventResult::Redraw;
                }
                _ => {} // fall through to textarea
            }
        }

        // ── ':' / Tab / Enter when cursor is on a file-ref element → open line select ──
        if matches!(
            key,
            KeyEvent {
                code: KeyCode::Char(':'),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
                ..
            } | KeyEvent {
                code: KeyCode::Tab | KeyCode::Enter,
                ..
            }
        ) && let Some(elem) = self.textarea.element_at_cursor()
            && elem.kind == KIND_FILE_REF
        {
            self.enter_line_select_from_element();
            return EventResult::Redraw;
        }

        // ── 'i' on any element → inline it; Tab/Enter on paste element → inline ──
        if let Some(elem) = self.textarea.element_at_cursor() {
            let is_i = matches!(
                key,
                KeyEvent {
                    code: KeyCode::Char('i'),
                    modifiers: KeyModifiers::NONE,
                    ..
                }
            );
            let is_tab_enter = matches!(
                key,
                KeyEvent {
                    code: KeyCode::Tab | KeyCode::Enter,
                    ..
                }
            );
            if is_i || (is_tab_enter && elem.kind == KIND_PASTE) {
                let id = elem.id;
                let desc = self
                    .element_meta
                    .remove(&id)
                    .map(|m| m.description)
                    .unwrap_or_else(|| "element".into());
                self.textarea.inline_element(id);
                self.status = format!("Inlined {desc}");
                self.recompute_file_search();
                return EventResult::Redraw;
            }
        }

        // ── Pass key to textarea, then recompute file search ──

        // Undo / Redo — intercept before passing to textarea.input().
        match key {
            KeyEvent {
                code: KeyCode::Char('z'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                if self.textarea.undo() {
                    self.status = "Undo.".into();
                } else {
                    self.status = "Nothing to undo.".into();
                }
                self.recompute_file_search();
                return EventResult::Redraw;
            }
            KeyEvent {
                code: KeyCode::Char('z'),
                modifiers,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL)
                && modifiers.contains(KeyModifiers::SHIFT) =>
            {
                if self.textarea.redo() {
                    self.status = "Redo.".into();
                } else {
                    self.status = "Nothing to redo.".into();
                }
                self.recompute_file_search();
                return EventResult::Redraw;
            }
            KeyEvent {
                code: KeyCode::Char('Z'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                if self.textarea.redo() {
                    self.status = "Redo.".into();
                } else {
                    self.status = "Nothing to redo.".into();
                }
                self.recompute_file_search();
                return EventResult::Redraw;
            }
            KeyEvent {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => {
                if self.textarea.redo() {
                    self.status = "Redo.".into();
                } else {
                    self.status = "Nothing to redo.".into();
                }
                self.recompute_file_search();
                return EventResult::Redraw;
            }
            _ => {}
        }

        self.textarea.input(key);
        self.recompute_file_search();

        // Update status when file search is not active.
        if !self.fs_active {
            if let Some(elem) = self.textarea.element_at_cursor() {
                let id = elem.id;
                if let Some(meta) = self.element_meta.get(&id) {
                    self.status = format!(
                        "Cursor on element: {} (: to select lines)",
                        meta.description
                    );
                }
            } else {
                let elems = self.textarea.elements().len();
                let chars = self.textarea.text().len();
                self.status = format!(
                    "cursor: {} | {} chars | {} element{}",
                    self.textarea.cursor(),
                    chars,
                    elems,
                    if elems != 1 { "s" } else { "" },
                );
            }
        }

        EventResult::Redraw
    }

    // ── Line select entry / handling / confirm ──

    fn enter_line_select_from_search(&mut self) {
        // First, confirm the file search (create element).
        let Some(ctx) = compute_file_search_context(
            self.textarea.text(),
            self.textarea.cursor(),
            self.textarea.elements(),
        ) else {
            return;
        };
        let Some(path) = self.file_search.selected_path() else {
            return;
        };
        let path = path.to_owned();

        let element_text = build_file_ref_text(&path, None);
        let display = build_file_ref_display(&path, None);

        // Begin undo group — stays open until confirm/cancel line select.
        self.textarea.begin_undo_group();

        let id = self.textarea.replace_range_with_element(
            ctx.range,
            &element_text,
            KIND_FILE_REF,
            Some(display),
        );
        self.textarea.insert_str(" ");

        self.element_meta.insert(
            id,
            ElementMeta {
                description: format!("File: {path}"),
            },
        );
        self.fs_active = false;
        self.file_search.clear();

        // Now open line select.
        if let Some(mode) = LineSelectMode::open(path.clone(), id) {
            self.status =
                format!("{path} | j/k ↕ | C-u/C-d ½pg | f/b pg | v sel | Enter ok | Esc cancel");
            self.line_select = Some(mode);
        } else {
            // File unreadable — close the group immediately.
            self.textarea.end_undo_group();
            self.status = format!("Could not read: {path}");
        }
    }

    fn enter_line_select_from_element(&mut self) {
        let Some(elem) = self.textarea.element_at_cursor() else {
            return;
        };
        if elem.kind != KIND_FILE_REF {
            return;
        }
        let id = elem.id;
        let elem_text = self.textarea.element_text(id).unwrap_or("").to_string();
        let (path, existing_range) = parse_file_ref(&elem_text);
        let path = path.to_string();

        let Some(mut mode) = LineSelectMode::open(path.clone(), id) else {
            self.status = format!("Could not read: {path}");
            return;
        };

        // Begin undo group — stays open until confirm/cancel.
        self.textarea.begin_undo_group();

        // If there's an existing line range, scroll to it and show as locked.
        if let Some(range) = existing_range {
            let mid = (range.start() + range.end()) / 2;
            mode.goto_line(mid);
            mode.selection = SelectionState::Locked(
                range.start().saturating_sub(1),
                range.end().saturating_sub(1),
            );
        }

        self.status =
            format!("{path} | j/k ↕ | C-u/C-d ½pg | f/b pg | v sel | Enter ok | Esc cancel");
        self.line_select = Some(mode);
    }
}
