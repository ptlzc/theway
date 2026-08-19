impl DemoApp {
    fn new() -> Self {
        let file_search = FileSearch::new();
        let file_count = file_search.all_files.len();
        let mut textarea = TextArea::new();
        textarea.set_clipboard_provider(Box::new(ArboardClipboard));
        Self {
            textarea,
            textarea_state: TextAreaState::default(),
            element_meta: HashMap::new(),
            status: format!(
                "Type text. @ to search {file_count} files. Tab/Enter confirm. Esc quit."
            ),
            file_search,
            fs_active: false,
            line_select: None,
            textarea_area: Rect::default(),
        }
    }

    // ── Event handling ──

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Paste(text) => {
                self.handle_paste(&text);
                self.recompute_file_search();
                EventResult::Redraw
            }
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Resize(_, _) => EventResult::Redraw,
            _ => EventResult::Unchanged,
        }
    }

    fn handle_paste(&mut self, text: &str) {
        let has_newline = text.contains('\n');

        if !has_newline {
            // Single-line paste → insert inline as plain text (single undo step).
            self.textarea.insert_str(text);
            let char_count = text.chars().count();
            self.status = format!("Pasted {char_count} chars inline");
            return;
        }

        // Multi-line paste → create an element with summary display.
        let line_count = text.lines().count();

        let bg = Color::Rgb(40, 40, 50);
        let display = Line::from(vec![
            Span::styled("[", Style::default().fg(Color::DarkGray).bg(bg)),
            Span::styled(
                format!(
                    "Pasted {} line{}",
                    line_count,
                    if line_count != 1 { "s" } else { "" },
                ),
                Style::default().fg(Color::Rgb(150, 150, 170)).bg(bg),
            ),
            Span::styled("]", Style::default().fg(Color::DarkGray).bg(bg)),
        ]);

        let id = self
            .textarea
            .insert_element(text, KIND_PASTE, Some(display));

        self.element_meta.insert(
            id,
            ElementMeta {
                description: format!(
                    "Pasted {} line{}",
                    line_count,
                    if line_count != 1 { "s" } else { "" },
                ),
            },
        );

        self.status = format!(
            "Pasted {} line{} (i to inline)",
            line_count,
            if line_count != 1 { "s" } else { "" },
        );
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> EventResult {
        // Don't forward mouse events during line-select mode.
        if self.line_select.is_some() {
            return EventResult::Unchanged;
        }

        // Middle-click: paste from system clipboard at the click position.
        if matches!(
            mouse.kind,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Middle)
        ) {
            if let Ok(mut clip) = arboard::Clipboard::new()
                && let Ok(text) = clip.get_text()
            {
                // Place cursor at the click position first.
                self.textarea
                    .handle_mouse(mouse, self.textarea_area, self.textarea_state);
                self.handle_paste(&text);
                return EventResult::Redraw;
            }
            return EventResult::Unchanged;
        }

        // If the click lands on the "❯ " prompt char (2 cols left of textarea),
        // remap it to column 0 of the textarea so it places the cursor at the
        // start of that visual line.
        let mut mouse = mouse;
        let ta = self.textarea_area;
        if ta.width > 0
            && mouse.column >= ta.x.saturating_sub(2)
            && mouse.column < ta.x
            && mouse.row >= ta.y
            && mouse.row < ta.y + ta.height
        {
            mouse.column = ta.x;
        }

        let action = self
            .textarea
            .handle_mouse(mouse, self.textarea_area, self.textarea_state);

        // Check for element interactions (click, hover enter/leave).
        let mut had_element_event = false;
        if let Some(elem_event) = self.textarea.poll_element_event() {
            had_element_event = true;
            match elem_event.kind {
                TextElementEventKind::Click => {
                    if let Some(meta) = self.element_meta.get(&elem_event.id) {
                        self.status =
                            format!("Clicked element: {} (: to select lines)", meta.description);
                    } else {
                        self.status = format!("Clicked element {:?}", elem_event.id);
                    }
                }
                TextElementEventKind::HoverEnter => {
                    if let Some(meta) = self.element_meta.get(&elem_event.id) {
                        self.status = format!("Hovering: {}", meta.description);
                    }
                }
                TextElementEventKind::HoverLeave => {
                    if let Some(meta) = self.element_meta.get(&elem_event.id) {
                        self.status = format!("Left element: {}", meta.description);
                    } else {
                        self.status = format!("Left element {:?}", elem_event.id);
                    }
                }
            }
        }

        match action {
            MouseAction::CursorPlaced if !had_element_event => {
                let pos = self.textarea.cursor();
                self.status = format!("Click → cursor at byte {pos}");
                self.recompute_file_search();
                EventResult::Redraw
            }
            MouseAction::CursorPlaced => {
                // Element click already set the status — don't overwrite.
                self.recompute_file_search();
                EventResult::Redraw
            }
            MouseAction::SelectionUpdated => {
                if let Some(text) = self.textarea.selected_text() {
                    let chars = text.chars().count();
                    self.status = format!("Selecting… ({chars} chars)");
                }
                EventResult::Redraw
            }
            MouseAction::SelectionFinished => {
                if let Some(text) = self.textarea.take_clipboard() {
                    let chars = text.chars().count();
                    self.status = format!("Selected {chars} chars (copied to clipboard)");
                }
                EventResult::Redraw
            }
            MouseAction::Nothing if had_element_event => EventResult::Redraw,
            MouseAction::Nothing => EventResult::Unchanged,
            MouseAction::Scrolled => EventResult::Redraw,
        }
    }

    /// Confirm the currently-selected file search result.
    ///
    /// Replaces the `@query` text with an atomic element and inserts a trailing space.
    fn confirm_file_search(&mut self) {
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

        // Group: replace + trailing space = 1 undo step.
        self.textarea.begin_undo_group();

        let id = self.textarea.replace_range_with_element(
            ctx.range,
            &element_text,
            KIND_FILE_REF,
            Some(display),
        );

        // Insert trailing space so the user can keep typing.
        self.textarea.insert_str(" ");

        self.textarea.end_undo_group();

        self.element_meta.insert(
            id,
            ElementMeta {
                description: format!("File: {path}"),
            },
        );

        self.fs_active = false;
        self.file_search.clear();
        self.status = format!("Confirmed: @{path}");
    }

    /// Recompute file search context from current textarea state.
    fn recompute_file_search(&mut self) {
        let ctx = compute_file_search_context(
            self.textarea.text(),
            self.textarea.cursor(),
            self.textarea.elements(),
        );
        match ctx {
            Some(ctx) => {
                self.file_search.update(&ctx.query);
                self.fs_active = true;
                if self.file_search.is_visible() {
                    self.status = format!(
                        "@-search: \"{}\" ({} match{})",
                        ctx.query,
                        self.file_search.results.len(),
                        if self.file_search.results.len() != 1 {
                            "es"
                        } else {
                            ""
                        },
                    );
                }
            }
            None => {
                self.fs_active = false;
                self.file_search.clear();
            }
        }
    }
}
