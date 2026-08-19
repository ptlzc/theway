    #[test]
    fn delete_forward_batches() {
        let mut ta = TextArea::new();
        ta.insert_str("abcde");
        ta.set_cursor(0);
        ta.delete_forward(1); // "bcde"
        ta.delete_forward(1); // "cde"
        ta.delete_forward(1); // "de"
        assert_eq!(ta.text(), "de");

        // All delete_forward calls batch into 1 step
        ta.undo(); // undo all deletes
        assert_eq!(ta.text(), "abcde");
    }

    #[test]
    fn word_boundary_breaks_insert_batch() {
        // Typing "foo bar" char by char: ws↔non-ws transitions create checkpoints.
        let mut ta = TextArea::new();
        // "foo" — all non-ws, batches into 1 step
        ta.insert_str("f");
        ta.insert_str("o");
        ta.insert_str("o");
        // " " — whitespace, class change → new step
        ta.insert_str(" ");
        // "bar" — non-ws, class change → new step
        ta.insert_str("b");
        ta.insert_str("a");
        ta.insert_str("r");
        assert_eq!(ta.text(), "foo bar");

        ta.undo(); // undo "bar"
        assert_eq!(ta.text(), "foo ");
        ta.undo(); // undo " "
        assert_eq!(ta.text(), "foo");
        ta.undo(); // undo "foo"
        assert_eq!(ta.text(), "");
        assert!(!ta.undo());
    }

    #[test]
    fn word_boundary_whitespace_runs_batch_together() {
        // Multiple consecutive whitespace chars batch into one step.
        let mut ta = TextArea::new();
        ta.insert_str("a");
        ta.insert_str(" ");
        ta.insert_str(" ");
        ta.insert_str(" ");
        ta.insert_str("b");
        assert_eq!(ta.text(), "a   b");

        ta.undo(); // undo "b"
        assert_eq!(ta.text(), "a   ");
        ta.undo(); // undo "   "
        assert_eq!(ta.text(), "a");
        ta.undo(); // undo "a"
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn word_boundary_newlines_are_whitespace() {
        // Newlines are whitespace — they batch with spaces, break from words.
        let mut ta = TextArea::new();
        ta.insert_str("foo");
        ta.insert_str("\n");
        ta.insert_str("\n");
        ta.insert_str(" ");
        ta.insert_str(" ");
        ta.insert_str("bar");
        assert_eq!(ta.text(), "foo\n\n  bar");

        ta.undo(); // undo "bar"
        assert_eq!(ta.text(), "foo\n\n  ");
        ta.undo(); // undo "\n\n  " (all whitespace batched)
        assert_eq!(ta.text(), "foo");
        ta.undo(); // undo "foo"
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn word_boundary_multi_char_insert_str_is_one_step() {
        // A single insert_str("hello world") call is still 1 undo step,
        // even though it contains a space. Boundary check only applies
        // between separate insert_str calls.
        let mut ta = TextArea::new();
        ta.insert_str("hello world");
        assert_eq!(ta.undo.stack.len(), 1);

        ta.undo();
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn word_boundary_after_undo_starts_fresh() {
        // After undo, last_kind is reset — no stale boundary check.
        let mut ta = TextArea::new();
        ta.insert_str("abc");
        ta.insert_str(" ");
        ta.undo(); // undo " "
        assert_eq!(ta.text(), "abc");
        // Now insert non-ws — should start a fresh batch, no boundary check against stale state.
        ta.insert_str("d");
        ta.insert_str("e");
        assert_eq!(ta.text(), "abcde");

        ta.undo(); // undo "de"
        assert_eq!(ta.text(), "abc");
    }

    #[test]
    fn element_insert_always_discrete() {
        let mut ta = TextArea::new();
        ta.insert_str("hi ");
        ta.insert_element("@file.rs", ElementKind(0), None);
        // Element should be its own undo step, not batched with the insert.
        assert_eq!(ta.text(), "hi @file.rs");

        ta.undo(); // undo element
        assert_eq!(ta.text(), "hi ");
        assert!(ta.elements().is_empty());

        ta.undo(); // undo "hi "
        assert_eq!(ta.text(), "");
    }

    // ── Phase 3: Element undo/redo tests ──

    #[test]
    fn undo_insert_element_redo_preserves_element_id() {
        let mut ta = TextArea::new();
        let id = ta.insert_element("@foo", ElementKind(1), None);
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);
        assert_eq!(ta.cursor(), "@foo".len());

        ta.undo(); // remove element
        assert!(ta.elements().is_empty());
        assert_eq!(ta.text(), "");
        assert_eq!(ta.cursor(), 0);

        ta.redo(); // restore element — same ElementId
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);
        assert_eq!(ta.text(), "@foo");
        assert_eq!(ta.cursor(), "@foo".len());
    }

    #[test]
    fn undo_redo_zero_length_element_preserves_metadata_and_cursor() {
        let mut ta = TextArea::new();
        let id = ta.insert_element("", ElementKind(9), None);
        assert_eq!(ta.cursor(), 0);
        assert_eq!(ta.elements()[0].range, 0..0);

        assert!(ta.undo());
        assert!(ta.elements().is_empty());
        assert_eq!(ta.cursor(), 0);

        assert!(ta.redo());
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);
        assert_eq!(ta.elements()[0].range, 0..0);
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn undo_replace_range_with_element_restores_original() {
        let mut ta = TextArea::new();
        ta.insert_str("hello @foo world");
        // Replace "@foo" (6..10) with an element
        let id = ta.replace_range_with_element(6..10, "@bar.rs", ElementKind(2), None);
        assert_eq!(ta.text(), "hello @bar.rs world");
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);

        ta.undo(); // undo replace → original text, no elements
        assert_eq!(ta.text(), "hello @foo world");
        assert!(ta.elements().is_empty());

        ta.redo(); // redo → element back
        assert_eq!(ta.text(), "hello @bar.rs world");
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);
    }

    #[test]
    fn undo_element_display_preserved() {
        let mut ta = TextArea::new();
        let display = Line::from(vec![
            ratatui::text::Span::styled("[", Style::default().fg(Color::Green)),
            ratatui::text::Span::raw("file.rs"),
            ratatui::text::Span::styled("]", Style::default().fg(Color::Green)),
        ]);
        let id = ta.insert_element("@file.rs", ElementKind(0), Some(display));
        assert!(ta.elements()[0].display.is_some());

        ta.undo();
        assert!(ta.elements().is_empty());

        ta.redo();
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);
        // Display should be restored from the snapshot clone
        let restored = ta.elements()[0].display.as_ref().unwrap();
        assert_eq!(restored.spans.len(), 3);
        let text: String = restored.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[file.rs]");
    }

    #[test]
    fn next_element_id_never_decreases_after_undo() {
        let mut ta = TextArea::new();
        let id1 = ta.insert_element("a", ElementKind(0), None);
        let id2 = ta.insert_element("b", ElementKind(0), None);

        ta.undo(); // undo element "b"
        ta.undo(); // undo element "a"
        assert!(ta.elements().is_empty());

        // New element after undo should get a fresh ID, never reuse id1 or id2.
        let id3 = ta.insert_element("c", ElementKind(0), None);
        assert_ne!(id3, id1);
        assert_ne!(id3, id2);
        // IDs are monotonically increasing
        assert!(id3.0 > id2.0);
    }

    #[test]
    fn backspace_on_element_undo_restores_element() {
        let mut ta = TextArea::new();
        ta.insert_str("before ");
        let id = ta.insert_element("[paste]", ElementKind(0), None);
        assert_eq!(ta.text(), "before [paste]");
        assert_eq!(ta.cursor(), 14);

        // Backspace at element end → deletes entire element atomically
        ta.delete_backward(1);
        assert_eq!(ta.text(), "before ");
        assert!(ta.elements().is_empty());

        // Undo → element restored with same ID
        ta.undo();
        assert_eq!(ta.text(), "before [paste]");
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.elements()[0].id, id);
        assert_eq!(ta.elements()[0].range, 7..14);
    }

    // ── Phase 4: Undo group tests ──

    #[test]
    fn undo_group_collapses_multiple_mutations() {
        // Autocomplete scenario: replace trigger + insert trailing space = 1 undo step.
        let mut ta = TextArea::new();
        ta.insert_str("hello @fo");
        assert_eq!(ta.text(), "hello @fo");

        ta.begin_undo_group();
        ta.replace_range_with_element(6..9, "@foo.rs", ElementKind(1), None);
        ta.insert_str(" "); // trailing space after element
        ta.end_undo_group();

        assert_eq!(ta.text(), "hello @foo.rs ");
        assert_eq!(ta.elements().len(), 1);

        // Single undo undoes the entire autocomplete operation.
        ta.undo();
        assert_eq!(ta.text(), "hello @fo");
        assert!(ta.elements().is_empty());
    }

    #[test]
    fn cancel_undo_group_restores_original() {
        // Line-select cancel: enter → N live-updates → cancel = 0 undo entries.
        let mut ta = TextArea::new();
        ta.insert_str("original");
        let stack_before = ta.undo.stack.len();

        ta.begin_undo_group();
        ta.set_text("modified once");
        ta.set_text("modified twice");
        ta.cancel_undo_group();

        // State restored to before the group.
        assert_eq!(ta.text(), "original");
        // No new undo entries created by the group.
        assert_eq!(ta.undo.stack.len(), stack_before);
    }

    #[test]
    fn nested_groups_only_outermost_pushes() {
        let mut ta = TextArea::new();
        ta.insert_str("start");

        ta.begin_undo_group(); // depth 1
        ta.insert_str(" A");
        ta.begin_undo_group(); // depth 2
        ta.insert_str(" B");
        ta.end_undo_group(); // depth 1 (inner end — no push)
        assert_eq!(ta.text(), "start A B");
        ta.insert_str(" C");
        ta.end_undo_group(); // depth 0 (outermost end — push)

        assert_eq!(ta.text(), "start A B C");

        // Single undo undoes everything in the group.
        ta.undo();
        assert_eq!(ta.text(), "start");
    }

    #[test]
    fn group_with_no_mutations_creates_no_entry() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        let stack_len = ta.undo.stack.len();

        ta.begin_undo_group();
        // No mutations inside the group.
        ta.end_undo_group();

        // Stack unchanged — no empty undo entry created.
        assert_eq!(ta.undo.stack.len(), stack_len);
    }

    #[test]
    fn redo_cleared_by_end_undo_group() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.undo(); // undo → ""
        assert!(ta.can_redo());

        ta.begin_undo_group();
        ta.insert_str("world");
        ta.end_undo_group();

        // Redo from the previous undo should be cleared.
        assert!(!ta.can_redo());
        assert_eq!(ta.text(), "world");
    }

    #[test]
    fn cancel_nested_group_restores_outermost() {
        // Even if deeply nested, cancel restores to the outermost group snapshot.
        let mut ta = TextArea::new();
        ta.insert_str("original");

        ta.begin_undo_group();
        ta.insert_str(" X");
        ta.begin_undo_group();
        ta.insert_str(" Y");
        // Cancel from inner level — should still restore to outermost snapshot.
        ta.cancel_undo_group();

        assert_eq!(ta.text(), "original");
        assert_eq!(ta.undo.group_depth, 0);
    }

    #[test]
    fn mutations_after_group_work_normally() {
        // After a group ends, normal batching resumes.
        let mut ta = TextArea::new();

        ta.begin_undo_group();
        ta.insert_str("grouped");
        ta.end_undo_group();

        // Normal insert after group — should be its own batch.
        ta.insert_str("X");
        ta.insert_str("Y"); // batches with X

        ta.undo(); // undo "XY"
        assert_eq!(ta.text(), "grouped");

        ta.undo(); // undo group
        assert_eq!(ta.text(), "");
    }

    // ── M3: Click-to-place cursor tests ──

    /// Helper to create a MouseEvent for testing.
    fn mouse_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_up(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn click_places_cursor_at_correct_position() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click at column 3 → cursor at byte 3
        let action = ta.handle_mouse(mouse_down(3, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 3);

        // Click at column 0 → cursor at byte 0
        let action = ta.handle_mouse(mouse_down(0, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 0);
    }

    #[test]
    fn click_on_element_returns_clicked_element() {
        let mut ta = TextArea::new();
        ta.insert_str("hi ");
        let id = ta.insert_element("elem", ElementKind(0), None);
        ta.insert_str(" bye");

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Element occupies cols 3..7, click at col 4
        let action = ta.handle_mouse(mouse_down(4, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        let ev = ta.poll_element_event().expect("should emit element click");
        assert_eq!(ev.id, id);
        assert_eq!(ev.kind, TextElementEventKind::Click);
    }

    #[test]
    fn click_past_end_of_line_snaps_to_line_end() {
        let mut ta = ta_with("hi");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click far past end of "hi" (col 20)
        let action = ta.handle_mouse(mouse_down(20, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 2); // end of "hi"
    }

    #[test]
    fn click_below_text_snaps_to_text_end() {
        let mut ta = ta_with("hello");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click on row 3 (only 1 row of text)
        let action = ta.handle_mouse(mouse_down(0, 3), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 5); // text.len()
    }

    #[test]
    fn click_clears_existing_selection() {
        let mut ta = ta_with("hello world");
        ta.set_selection(0, 5);
        assert!(ta.selection_range().is_some());

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(8, 0), area, state);
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn click_outside_area_returns_nothing() {
        let mut ta = ta_with("hello");
        let area = Rect::new(5, 5, 20, 3);
        let state = TextAreaState::default();

        // Click outside the area
        let action = ta.handle_mouse(mouse_down(0, 0), area, state);
        assert_eq!(action, MouseAction::Nothing);
    }

    #[test]
    fn mouse_up_clears_down_pos() {
        let mut ta = ta_with("hello");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(2, 0), area, state);
        assert!(ta.mouse_down_pos.is_some());

        ta.handle_mouse(mouse_up(2, 0), area, state);
        assert!(ta.mouse_down_pos.is_none());
    }

    #[test]
    fn click_on_second_line_multiline_text() {
        let mut ta = ta_with("hello\nworld");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click on row 1, col 2 → "world" starts at byte 6, so byte 8 = 'r'
        let action = ta.handle_mouse(mouse_down(2, 1), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 8); // "hello\nwo" = 8 bytes → cursor at 'r'
    }

    // ── M4: Drag selection tests ──

    fn mouse_drag(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn drag_selects_text() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Mouse down at col 0, drag to col 5
        ta.handle_mouse(mouse_down(0, 0), area, state);
        let action = ta.handle_mouse(mouse_drag(5, 0), area, state);
        assert_eq!(action, MouseAction::SelectionUpdated);
        assert_eq!(ta.selection_range(), Some(0..5));
        assert_eq!(ta.selected_text(), Some("hello".to_string()));
    }

    #[test]
    fn drag_across_element_expands_to_element_boundaries() {
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        let mut ta = TextArea::new();
        ta.insert_str("ab");
        ta.insert_element("ELEM", ElementKind(0), None);
        ta.insert_str("cd");
        // buffer: "abELEMcd"
        // element range: 2..6
        // display cols: a(0) b(1) E(2) L(3) E(4) M(5) c(6) d(7)

        // Drag from col 1 ("b") to col 7 ("d") — fully crosses the element.
        // Raw selection: anchor=1, head=7. Element at 2..6 is fully inside.
        ta.handle_mouse(mouse_down(1, 0), area, state);
        ta.handle_mouse(mouse_drag(7, 0), area, state);
        let range = ta.selection_range().unwrap();
        assert_eq!(range, 1..7);

        let mut ta = TextArea::new();
        ta.insert_str("ab");
        ta.insert_element("ELEM", ElementKind(0), None);
        ta.insert_str("cd");

        // Now test partial overlap: drag from col 0 to col 3 (into the element).
        // display_col_to_buffer_pos snaps col 3 to element start (2) since dist
        // to start (1) < dist to end (3). Raw selection 0..2 → but element at
        // 2..6 is NOT overlapped, so no expansion.
        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(3, 0), area, state);
        let range = ta.selection_range().unwrap();
        assert_eq!(range, 0..2);

        let mut ta = TextArea::new();
        ta.insert_str("ab");
        ta.insert_element("ELEM", ElementKind(0), None);
        ta.insert_str("cd");

        // Drag from col 0 to col 5 — past element midpoint, so snaps to end (6).
        // Raw selection 0..6 → element fully covered.
        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(5, 0), area, state);
        let range = ta.selection_range().unwrap();
        assert_eq!(range, 0..6);
    }

    #[test]
    fn mouse_up_after_drag_copies_to_clipboard() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(6, 0), area, state);
        ta.handle_mouse(mouse_drag(11, 0), area, state);
        let action = ta.handle_mouse(mouse_up(11, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);

        // Clipboard should contain "world"
        assert_eq!(ta.take_clipboard(), Some("world".to_string()));
        // take_clipboard clears it
        assert_eq!(ta.take_clipboard(), None);
    }

    #[test]
    fn selection_persists_after_mouseup_by_default() {
        let mut ta = ta_with("hello");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(3, 0), area, state);
        ta.handle_mouse(mouse_up(3, 0), area, state);

        // Default: keep_selection_after_mouseup = true
        assert!(ta.selection_range().is_some());
        assert_eq!(ta.selected_text(), Some("hel".to_string()));
    }

    #[test]
    fn selection_clears_after_mouseup_when_configured() {
        let mut ta = ta_with("hello");
        ta.keep_selection_after_mouseup = false;
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(3, 0), area, state);
        ta.handle_mouse(mouse_up(3, 0), area, state);

        // Clipboard was still set
        assert_eq!(ta.take_clipboard(), Some("hel".to_string()));
        // But selection is cleared
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn backspace_deletes_selection_only() {
        let mut ta = ta_with("hello world");
        ta.set_selection(0, 5);
        assert_eq!(ta.selected_text(), Some("hello".to_string()));

        // Backspace should delete "hello", not an extra char.
        ta.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(ta.text(), " world");
        assert_eq!(ta.cursor(), 0);
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn typing_replaces_selection() {
        let mut ta = ta_with("hello world");
        ta.set_selection(0, 5);

        // Typing 'X' should replace "hello" with "X".
        ta.input(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
        assert_eq!(ta.text(), "X world");
        assert_eq!(ta.cursor(), 1);
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn arrow_clears_selection() {
        let mut ta = ta_with("hello world");
        ta.set_selection(0, 5);
        assert!(ta.selection_range().is_some());

        ta.input(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn undo_after_delete_selection_restores() {
        let mut ta = ta_with("hello world");
        ta.set_cursor(ta.text().len());
        ta.set_selection(0, 5);

        ta.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(ta.text(), " world");
        assert_eq!(ta.cursor(), 0);
        assert_eq!(ta.undo.last_cursor, 0);

        ta.undo();
        assert_eq!(ta.text(), "hello world");
    }

    #[test]
    fn undo_after_type_replace_selection_restores() {
        let mut ta = ta_with("hello world");
        ta.set_selection(0, 5);

        ta.input(KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE));
        assert_eq!(ta.text(), "X world");

        // Single undo should restore to pre-replacement state (undo group).
        ta.undo();
        assert_eq!(ta.text(), "hello world");
    }
