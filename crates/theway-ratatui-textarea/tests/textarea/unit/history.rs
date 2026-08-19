    #[test]
    fn backspace_works_with_zero_width_selection() {
        // Regression: a zero-width selection (anchor == head, from mouse
        // jitter) caused Backspace/Delete to be silently swallowed because
        // delete_selection() returned false but input() still returned early.
        let mut ta = ta_with("hello");
        ta.set_cursor(5);
        // Simulate a zero-width selection (anchor == head at cursor).
        ta.set_selection(5, 5);
        assert!(ta.selection_range().is_none()); // zero-width → no range

        // Backspace must still delete the char before cursor.
        ta.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(ta.text(), "hell");
        assert_eq!(ta.cursor(), 4);
        // Selection should be cleared.
        assert!(ta.selection.is_none());
    }

    #[test]
    fn delete_forward_works_with_zero_width_selection() {
        let mut ta = ta_with("hello");
        ta.set_cursor(2);
        ta.set_selection(2, 2);

        ta.input(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(ta.text(), "helo");
        assert_eq!(ta.cursor(), 2);
        assert!(ta.selection.is_none());
    }

    #[test]
    fn ctrl_x_with_zero_width_selection_falls_through() {
        let mut ta = ta_with("hello");
        ta.set_cursor(5);
        ta.set_selection(5, 5);

        // Ctrl-X on zero-width selection shouldn't eat the key.
        // It should clear selection and fall through to normal handling
        // (which for Ctrl-X without selection is a no-op, but the selection
        // must be cleared).
        ta.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert!(ta.selection.is_none());
    }

    #[test]
    fn mouse_up_discards_zero_width_drag_selection() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click at col 3 then drag to same position (zero distance).
        ta.handle_mouse(mouse_down(3, 0), area, state);
        let action = ta.handle_mouse(mouse_drag(3, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        let action = ta.handle_mouse(mouse_up(3, 0), area, state);
        assert_eq!(action, MouseAction::Nothing);

        // Zero-width drag should not leave a selection behind.
        assert!(ta.selection.is_none());
    }

    #[test]
    fn drag_backward_selects_correctly() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click at col 8, drag back to col 3 (backward selection)
        ta.handle_mouse(mouse_down(8, 0), area, state);
        ta.handle_mouse(mouse_drag(3, 0), area, state);
        // selection_range() normalizes anchor/head
        assert_eq!(ta.selection_range(), Some(3..8));
        assert_eq!(ta.selected_text(), Some("lo wo".to_string()));
    }

    #[test]
    fn click_after_drag_clears_selection() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Drag to select
        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(5, 0), area, state);
        ta.handle_mouse(mouse_up(5, 0), area, state);
        assert!(ta.selection_range().is_some());

        // New click clears the old selection
        ta.handle_mouse(mouse_down(8, 0), area, state);
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn set_text_clears_selection_and_drag_state() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(5, 0), area, state);
        assert!(ta.selection_range().is_some());
        assert!(ta.drag_anchor.is_some());
        assert!(ta.drag_active);
        assert!(ta.mouse_down_pos.is_some());

        ta.set_text("reset");

        assert!(ta.selection.is_none());
        assert!(ta.selection_range().is_none());
        assert!(ta.drag_anchor.is_none());
        assert!(!ta.drag_active);
        assert!(ta.mouse_down_pos.is_none());
        assert!(ta.pending_drag_scroll.is_none());
    }

    #[test]
    fn same_cell_drag_does_not_create_selection() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(3, 0), area, state);
        let action = ta.handle_mouse(mouse_drag(3, 0), area, state);

        assert_eq!(action, MouseAction::CursorPlaced);
        assert!(ta.selection.is_none());
        assert!(ta.selection_range().is_none());
        assert!(!ta.drag_active);
    }

    #[test]
    fn typing_with_zero_width_selection_inserts_character() {
        let mut ta = ta_with("hello");
        ta.set_cursor(5);
        ta.set_selection(5, 5);

        ta.input(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::SHIFT));

        assert_eq!(ta.text(), "hello!");
        assert_eq!(ta.cursor(), 6);
        assert!(ta.selection.is_none());
    }

    #[test]
    fn mouse_up_after_drag_clears_drag_anchor() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(5, 0), area, state);
        assert!(ta.drag_anchor.is_some());

        ta.handle_mouse(mouse_up(5, 0), area, state);

        assert!(ta.drag_anchor.is_none());
        assert!(!ta.drag_active);
    }

    // ── M5: Double/triple click tests ──

    #[test]
    fn double_click_selects_word() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "hello" (col 2)
        ta.handle_mouse(mouse_down(2, 0), area, state);
        let action = ta.handle_mouse(mouse_down(2, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        assert_eq!(ta.selection_range(), Some(0..5));
        assert_eq!(ta.selected_text(), Some("hello".to_string()));
        assert_eq!(ta.take_clipboard(), Some("hello".to_string()));
    }

    #[test]
    fn double_click_cursor_on_last_char() {
        // Neovim places cursor on the last character of the selection,
        // not one past the end.
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "hello"
        ta.handle_mouse(mouse_down(2, 0), area, state);
        ta.handle_mouse(mouse_down(2, 0), area, state);

        assert_eq!(ta.selection_range(), Some(0..5));
        // Cursor should be on 'o' (byte 4), not on ' ' (byte 5)
        assert_eq!(
            ta.cursor(),
            4,
            "double-click cursor should be on last char 'o' (byte 4), got {}",
            ta.cursor()
        );
    }

    #[test]
    fn double_click_cursor_on_last_char_unicode() {
        // Test with multi-byte characters — cursor must land on a valid char boundary
        let mut ta = ta_with("café bar");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "café" (col 1 = 'a')
        ta.handle_mouse(mouse_down(1, 0), area, state);
        ta.handle_mouse(mouse_down(1, 0), area, state);

        assert_eq!(ta.selected_text(), Some("café".to_string()));
        // 'é' is 2 bytes (0xC3 0xA9), so "café" = [c(0), a(1), f(2), é(3,4)]
        // Last char 'é' starts at byte 3
        assert_eq!(
            ta.cursor(),
            3,
            "cursor should be on 'é' (byte 3), got {}",
            ta.cursor()
        );
    }

    #[test]
    fn double_click_on_second_word() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "world" (col 8)
        ta.handle_mouse(mouse_down(8, 0), area, state);
        let action = ta.handle_mouse(mouse_down(8, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        assert_eq!(ta.selection_range(), Some(6..11));
        assert_eq!(ta.selected_text(), Some("world".to_string()));
    }

    #[test]
    fn double_click_stops_at_punctuation() {
        // "hello, world," — double-click on 'h' should select "hello", not "hello,"
        let mut ta = ta_with("hello, world,");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "hello" (col 2 = 'l')
        ta.handle_mouse(mouse_down(2, 0), area, state);
        let action = ta.handle_mouse(mouse_down(2, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        assert_eq!(
            ta.selected_text(),
            Some("hello".to_string()),
            "should select only 'hello', not include trailing comma"
        );
        assert_eq!(ta.selection_range(), Some(0..5));
    }

    #[test]
    fn double_click_on_punctuation_selects_punctuation_run() {
        // Double-click on punctuation selects the contiguous punctuation
        let mut ta = ta_with("hello... world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "..." (col 6 = first '.')
        ta.handle_mouse(mouse_down(6, 0), area, state);
        let action = ta.handle_mouse(mouse_down(6, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        assert_eq!(
            ta.selected_text(),
            Some("...".to_string()),
            "double-click on punctuation should select the punctuation run"
        );
    }

    #[test]
    fn double_click_word_with_underscore() {
        // Underscores are part of a word (like vim's iskeyword)
        let mut ta = ta_with("hello_world foo");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on "hello_world" (col 3)
        ta.handle_mouse(mouse_down(3, 0), area, state);
        let action = ta.handle_mouse(mouse_down(3, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        assert_eq!(
            ta.selected_text(),
            Some("hello_world".to_string()),
            "underscore should be part of the word"
        );
    }

    #[test]
    fn double_click_on_element_snaps_like_single_click() {
        // Word-selecting an element would copy its hidden buffer text to
        // the clipboard; a double-click must instead snap to the element
        // start and re-emit Click so the host decides what it means.
        let mut ta = TextArea::new();
        ta.insert_str("hi ");
        let display = Line::from("[chip]");
        let id = ta.insert_element("hidden\ntext", ElementKind(0), Some(display));
        ta.insert_str(" bye");
        // Buffer: "hi hidden\ntext bye", element at 3..14, display "[chip]".

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // First click on the display (col 4) emits its own Click event.
        ta.handle_mouse(mouse_down(4, 0), area, state);
        assert!(ta.poll_element_event().is_some());

        let action = ta.handle_mouse(mouse_down(4, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 3); // element start
        assert!(ta.selection_range().is_none());
        assert_eq!(ta.take_clipboard(), None);
        let ev = ta
            .poll_element_event()
            .expect("double-click re-emits Click");
        assert_eq!(ev.id, id);
        assert_eq!(ev.kind, TextElementEventKind::Click);
    }

    #[test]
    fn triple_click_selects_line() {
        let mut ta = ta_with("hello world\nsecond line\nthird");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Triple-click on first line (col 3)
        ta.handle_mouse(mouse_down(3, 0), area, state);
        ta.handle_mouse(mouse_down(3, 0), area, state);
        let action = ta.handle_mouse(mouse_down(3, 0), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        // Should select "hello world\n" (including the newline)
        assert_eq!(ta.selection_range(), Some(0..12));
        assert_eq!(ta.selected_text(), Some("hello world\n".to_string()));
    }

    #[test]
    fn triple_click_on_last_line_selects_to_end() {
        let mut ta = ta_with("hello\nworld");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Triple-click on "world" (row 1, col 2)
        ta.handle_mouse(mouse_down(2, 1), area, state);
        ta.handle_mouse(mouse_down(2, 1), area, state);
        let action = ta.handle_mouse(mouse_down(2, 1), area, state);
        assert_eq!(action, MouseAction::SelectionFinished);
        // Last line has no trailing \n — selects to text.len()
        assert_eq!(ta.selection_range(), Some(6..11));
        assert_eq!(ta.selected_text(), Some("world".to_string()));
    }

    #[test]
    fn triple_click_cursor_stays_at_click_pos() {
        // Triple-click should select the whole line but keep the cursor
        // at the click position, not at the end of the selection.
        let mut ta = ta_with("hello world\nsecond line\nthird");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Triple-click on first line at col 3 (byte 3 = 'l')
        ta.handle_mouse(mouse_down(3, 0), area, state);
        ta.handle_mouse(mouse_down(3, 0), area, state);
        ta.handle_mouse(mouse_down(3, 0), area, state);

        // Selection covers the full line "hello world\n"
        assert_eq!(ta.selection_range(), Some(0..12));

        // Cursor should be at the click position (byte 3), not at line_end (12)
        assert_eq!(
            ta.cursor(),
            3,
            "triple-click cursor should stay at click pos (3), got {}",
            ta.cursor()
        );
    }

    #[test]
    fn selection_uses_custom_style_override() {
        let mut t = ta_with("hello");
        t.selection_style = Style::default().bg(Color::Blue);
        t.set_selection(1, 4); // select "ell"

        let area = Rect::new(0, 0, 10, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        // Cells 1, 2, 3 should have Blue background (custom selection style)
        for col in 1..4u16 {
            let cell = &buf[(col, 0)];
            assert_eq!(
                cell.bg,
                Color::Blue,
                "cell at col {col} should have Blue bg"
            );
        }
        // Cell 0 ('h') and cell 4 ('o') should NOT have Blue bg
        assert_ne!(buf[(0, 0)].bg, Color::Blue);
        assert_ne!(buf[(4, 0)].bg, Color::Blue);
    }

    #[test]
    fn double_click_on_whitespace_places_cursor() {
        let mut ta = ta_with("hello   world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Double-click on whitespace (col 6)
        ta.handle_mouse(mouse_down(6, 0), area, state);
        let action = ta.handle_mouse(mouse_down(6, 0), area, state);
        // Whitespace has no word → just places cursor
        assert_eq!(action, MouseAction::CursorPlaced);
        assert!(ta.selection_range().is_none());
    }

    #[test]
    fn click_tracker_resets_on_position_change() {
        let mut ta = ta_with("hello world");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click at col 2, then at col 8 → not a double-click
        ta.handle_mouse(mouse_down(2, 0), area, state);
        let action = ta.handle_mouse(mouse_down(8, 0), area, state);
        // Should be a single click, not a double-click
        assert_eq!(action, MouseAction::CursorPlaced);
        assert!(ta.selection_range().is_none());
    }

    // ── Drag-to-scroll tests ──

    #[test]
    fn drag_below_area_scrolls_down_and_extends_selection() {
        // Text with 5 lines, visible area is only 3 rows tall.
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee");
        // Place cursor at start so scroll=0.
        ta.set_cursor(0);
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState::default();

        // Click on first line.
        ta.handle_mouse(mouse_down(0, 0), area, state);
        assert_eq!(ta.cursor(), 0);

        // Drag below the visible area (row 5, past area.height=3).
        let action = ta.handle_mouse(mouse_drag(0, 5), area, state);
        assert_eq!(action, MouseAction::SelectionUpdated);

        // Cursor should have moved past the visible area.
        // With scroll=0 and height=3, visible lines are 0,1,2 (aaa,bbb,ccc).
        // Dragging below → target_line = visible_end = 3 → "ddd" starts at byte 12.
        // At col 0, cursor should be at byte 12 (start of "ddd").
        assert!(ta.cursor() >= 12);

        // Selection should extend from anchor (0) to the new cursor position.
        let range = ta.selection_range().unwrap();
        assert_eq!(range.start, 0);
        assert!(range.end >= 12);
    }

    #[test]
    fn drag_above_area_scrolls_up_and_extends_selection() {
        // Text with 5 lines, start with cursor on the last line.
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee");
        let area = Rect::new(0, 0, 40, 3);

        // Place cursor at the end first (to ensure scroll is at the bottom).
        ta.set_cursor(ta.text().len());
        let state = TextAreaState { scroll: 2 };

        // Click on bottom visible line (row 2).
        ta.handle_mouse(mouse_down(1, 2), area, state);

        // Drag above the visible area (row is before area.y).
        // Since area.y = 0, dragging to row=0 when scroll=2 means the row
        // is at the top edge. We need a row *above* the area. With area.y=0,
        // we can't go negative, but we can use an area with area.y > 0.
        let area2 = Rect::new(0, 5, 40, 3); // area starts at row 5
        ta.handle_mouse(mouse_down(1, 7), area2, state); // click at row 7 (visible)

        // Drag above: row 3 (above area2.y=5)
        let action = ta.handle_mouse(mouse_drag(0, 3), area2, state);
        assert_eq!(action, MouseAction::SelectionUpdated);

        // Cursor should have moved to a line above the visible region.
        // The exact position depends on how many lines we scroll per drag.
        let range = ta.selection_range().unwrap();
        assert!(range.start < range.end);
    }

    #[test]
    fn drag_below_area_moves_cursor_past_last_visible_line() {
        // 10 short lines, area shows only 2.
        let text = "L0\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\nL9";
        let mut ta = ta_with(text);
        // Place cursor at start so scroll=0.
        ta.set_cursor(0);
        let area = Rect::new(0, 0, 40, 2);
        let state = TextAreaState::default();

        // Click at start.
        ta.handle_mouse(mouse_down(0, 0), area, state);
        assert_eq!(ta.cursor(), 0);

        // Drag below area (row 10, way below the 2-row area).
        let action = ta.handle_mouse(mouse_drag(1, 10), area, state);
        assert_eq!(action, MouseAction::SelectionUpdated);

        // With scroll=0 and height=2, visible lines are 0,1 (L0,L1).
        // Dragging below → target_line = 2 → "L2" starts at byte 6.
        // Cursor should be at col 1 of L2 → byte 7.
        assert!(ta.cursor() >= 6, "cursor={} should be >= 6", ta.cursor());
    }

    #[test]
    fn drag_above_wide_column_still_scrolls_up() {
        // Bug: when dragging above the area with a column wider than the
        // target line, display_col_to_buffer_pos returns line_end which
        // equals the *next* line's start.  wrapped_line_index_by_start
        // then resolves to the next line, so effective_scroll sees the
        // cursor as still within the viewport and doesn't scroll.
        //
        // Scenario: 10 short lines ("ab"), area is 3 rows tall with
        // area.y = 2 (so we can drag above).  Scroll starts at line 5.
        // We drag to row 1 (above area.y = 2) at column 50 (way past
        // each 3-byte line).  The cursor must land ON the target line
        // (line 4), not spill over to line 5.
        let text = "ab\nab\nab\nab\nab\nab\nab\nab\nab\nab";
        let mut ta = ta_with(text);
        let area = Rect::new(0, 2, 40, 3); // area starts at row 2
        let state = TextAreaState { scroll: 5 };

        // Place cursor on wrapped line 6 (within viewport at scroll=5).
        // "ab\n" is 3 bytes per line, so line 6 starts at byte 18.
        ta.set_cursor(18);

        // Click inside the area (row 3 = area.y + 1).
        ta.handle_mouse(mouse_down(1, 3), area, state);

        // Drag above the area: row 1 (< area.y=2), column 50 (far right).
        let action = ta.handle_mouse(mouse_drag(50, 1), area, state);
        assert_eq!(action, MouseAction::SelectionUpdated);

        // target_line = scroll(5) - 1 = 4.  Line 4 spans bytes 12..15.
        // The cursor MUST be within line 4's range [12, 14], NOT at 15
        // (which is line 5's start).
        let cursor = ta.cursor();
        assert!(
            (12..15).contains(&cursor),
            "cursor={cursor} should be in [12, 15) (on line 4), \
             not at 15 (line 5 start)"
        );
    }

    #[test]
    fn drag_below_wide_column_still_scrolls_down() {
        // Same bug but for scrolling down: dragging below the area with
        // a wide column should place the cursor on the target line, not
        // spill over to the next line.
        let text = "ab\nab\nab\nab\nab\nab\nab\nab\nab\nab";
        let mut ta = ta_with(text);
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState::default(); // scroll=0

        // Place cursor on first line.
        ta.set_cursor(0);

        // Click inside the area.
        ta.handle_mouse(mouse_down(1, 0), area, state);

        // Drag below the area: row 5 (>= area.y + height=3), column 50.
        let action = ta.handle_mouse(mouse_drag(50, 5), area, state);
        assert_eq!(action, MouseAction::SelectionUpdated);

        // visible_end = 0 + 3 = 3.  dist = 5 - 3 + 1 = 3.
        // n = drag_scroll_lines_for_distance(3) = 2.
        // target_line = (3 + 2 - 1) = 4.  Line 4 spans bytes 12..15.
        // Cursor must be within [12, 14], not at 15.
        let cursor = ta.cursor();
        assert!(
            (12..15).contains(&cursor),
            "cursor={cursor} should be in [12, 15) (on line 4), \
             not at 15 (line 5 start)"
        );
    }

    #[test]
    fn drag_above_with_multibyte_line_end_does_not_panic() {
        // Regression: clamp_to_line used `line_end - 1` which can land inside
        // a multi-byte character (e.g. '│' = 3 bytes).
        let text = "aaa│\nbbb│\nccc│\nddd│\neee│\nfff│\nggg│";
        let mut ta = ta_with(text);
        let area = Rect::new(0, 0, 40, 3);
        // Start scrolled down so we can drag above.
        let state = TextAreaState { scroll: 3 };
        ta.set_cursor(20); // somewhere in the middle

        // Click inside the area.
        ta.handle_mouse(mouse_down(1, 1), area, state);

        // Drag above the area at a wide column (beyond line width).
        let action = ta.handle_mouse(mouse_drag(50, 0), area, state);
        // Should not panic — cursor should be on a valid char boundary.
        assert!(
            matches!(action, MouseAction::SelectionUpdated),
            "drag above should create selection, got {action:?}"
        );
        // Verify cursor is at a valid char boundary by reading from it.
        let cursor = ta.cursor();
        assert!(
            ta.text().is_char_boundary(cursor),
            "cursor at byte {cursor} is not a char boundary"
        );
    }

    #[test]
    fn selection_across_element_with_multibyte_chars_does_not_panic() {
        // Regression: display_col_to_buffer_pos used `line_end + 1` to skip
        // past elements, but `line_end + 1` can land inside a multi-byte
        // character (e.g. '│' = 3 bytes).
        let mut ta = TextArea::new();
        ta.insert_str("before ");
        // Create an element whose backing text contains multi-byte '│' chars
        // across multiple lines — this triggers wrapping mid-element.
        let backing = "│  Ctrl+Shift+Z/Y  redo  │\n│  Ctrl+C  clear  │";
        ta.insert_element(backing, ElementKind(0), None);
        ta.insert_str(" after");

        let area = Rect::new(0, 0, 30, 5); // narrow so wrapping is forced

        // Select across the entire text (from start to end).
        ta.set_selection(0, ta.text().len());

        // Render should not panic.
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&ta), area, &mut buf);
    }

    #[test]
    fn click_on_text_with_multibyte_chars_does_not_panic() {
        // Plain text with '│' — clicking anywhere should not panic.
        let text = "│  Ctrl+Shift+Z/Y  redo  │\n│  Ctrl+C  clear  │";
        let mut ta = ta_with(text);
        let area = Rect::new(0, 0, 30, 5);
        let state = TextAreaState::default();

        // Click at various columns — should not panic.
        for col in 0..25u16 {
            ta.handle_mouse(mouse_down(col, 0), area, state);
        }
        // Double-click should also be safe.
        ta.handle_mouse(mouse_down(5, 0), area, state);
        ta.handle_mouse(mouse_down(5, 0), area, state);
    }

    #[test]
    fn selecting_wrapped_line_ending_with_multibyte_char_does_not_panic() {
        // Regression: when a line wraps and '│' (3-byte char) ends up right
        // at the wrap boundary, the wrapping code (or rendering) can produce
        // a byte position inside the multi-byte character.
        //
        // Reproduce: enough spaces so '│' is pushed to the next wrapped line.
        let text = format!("{}│", " ".repeat(29)); // 29 spaces + '│' = 30 display cols
        let mut ta = ta_with(&text);
        let area = Rect::new(0, 0, 30, 5); // width 30 → '│' wraps to next line
        let _state = TextAreaState::default();

        // Select across the wrap boundary.
        ta.set_selection(0, ta.text().len());

        // Render should not panic.
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&ta), area, &mut buf);
    }

    #[test]
    fn clicking_on_wrapped_multibyte_line_does_not_panic() {
        // Same as above but triggered via click/drag rather than render.
        for extra_spaces in 28..33 {
            let text = format!("{}│end", " ".repeat(extra_spaces));
            let mut ta = ta_with(&text);
            let area = Rect::new(0, 0, 30, 5);
            let state = TextAreaState::default();

            // Click on every column of both rows.
            for row in 0..2u16 {
                for col in 0..30u16 {
                    ta.handle_mouse(mouse_down(col, row), area, state);
                }
            }
        }
    }

    // ── Inline element tests ──

    #[test]
    fn inline_element_replaces_element_with_text() {
        let mut ta = TextArea::new();
        ta.insert_str("before ");
        let id = ta.insert_element("pasted\ncontent\nhere", ElementKind(1), None);
        ta.insert_str(" after");
        // Buffer: "before pasted\ncontent\nhere after"
        // Element at "pasted\ncontent\nhere" (bytes 7..26)

        let inlined = ta.inline_element(id);
        assert!(inlined);

        // Text should remain the same (the element's buffer text is kept).
        assert_eq!(ta.text(), "before pasted\ncontent\nhere after");
        // But the element should be gone.
        assert!(ta.elements().is_empty());
        // Cursor should be at the end of the inlined text.
        assert_eq!(ta.cursor(), 26); // end of "pasted\ncontent\nhere"
    }
