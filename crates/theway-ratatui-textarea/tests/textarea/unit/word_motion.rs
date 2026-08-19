    #[test]
    fn buffer_pos_at_screen_plain_text_start() {
        let t = ta_with("hello");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click at column 0 → pos 0
        assert_eq!(t.buffer_pos_at_screen(0, 0, area, state), Some(0));
    }

    #[test]
    fn buffer_pos_at_screen_plain_text_middle() {
        let t = ta_with("hello");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click at column 3 → pos 3
        assert_eq!(t.buffer_pos_at_screen(3, 0, area, state), Some(3));
    }

    #[test]
    fn buffer_pos_at_screen_past_end_of_line() {
        let t = ta_with("hello");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click at column 10, line only has 5 chars → snap to end of text
        assert_eq!(t.buffer_pos_at_screen(10, 0, area, state), Some(5));
    }

    #[test]
    fn buffer_pos_at_screen_below_text() {
        let t = ta_with("hello");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click on row 3, text only occupies row 0 → end of text
        assert_eq!(t.buffer_pos_at_screen(0, 3, area, state), Some(5));
    }

    #[test]
    fn buffer_pos_at_screen_outside_area() {
        let t = ta_with("hello");
        let area = Rect::new(5, 5, 20, 5);
        let state = TextAreaState::default();
        // Click at (0, 0) which is outside area starting at (5, 5)
        assert_eq!(t.buffer_pos_at_screen(0, 0, area, state), None);
        // Click at (4, 5) — just left of area
        assert_eq!(t.buffer_pos_at_screen(4, 5, area, state), None);
        // Click at (5, 4) — just above area
        assert_eq!(t.buffer_pos_at_screen(5, 4, area, state), None);
    }

    #[test]
    fn buffer_pos_at_screen_with_area_offset() {
        let t = ta_with("hello");
        let area = Rect::new(10, 5, 20, 5);
        let state = TextAreaState::default();
        // Click at screen (13, 5) = column 3 within the area → pos 3
        assert_eq!(t.buffer_pos_at_screen(13, 5, area, state), Some(3));
    }

    #[test]
    fn buffer_pos_at_screen_multiline() {
        let t = ta_with("hello\nworld");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click on row 0, col 2 → "hello" pos 2
        assert_eq!(t.buffer_pos_at_screen(2, 0, area, state), Some(2));
        // Click on row 1, col 1 → "world" pos 6+1 = 7
        assert_eq!(t.buffer_pos_at_screen(1, 1, area, state), Some(7));
    }

    #[test]
    fn buffer_pos_at_screen_wrapped_text() {
        // "abcdefghij" at width 5 wraps into "abcde" (0..5) and "fghij" (5..10)
        let t = ta_with("abcdefghij");
        let area = Rect::new(0, 0, 5, 5);
        let state = TextAreaState::default();
        // Click on row 0, col 2 → pos 2
        assert_eq!(t.buffer_pos_at_screen(2, 0, area, state), Some(2));
        // Click on row 1, col 0 → pos 5 (start of second wrapped line)
        assert_eq!(t.buffer_pos_at_screen(0, 1, area, state), Some(5));
        // Click on row 1, col 3 → pos 8
        assert_eq!(t.buffer_pos_at_screen(3, 1, area, state), Some(8));
    }

    #[test]
    fn buffer_pos_at_screen_scrolled() {
        // 3 lines, area height 2 → first line scrolled off when cursor is at end
        let mut t = ta_with("aaa\nbbb\nccc");
        t.set_cursor(t.text().len()); // cursor at end → scroll to show last lines
        let area = Rect::new(0, 0, 20, 2);
        let mut state = TextAreaState::default();
        // Render to compute scroll
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        // state.scroll should be 1 (skipping "aaa")
        assert_eq!(state.scroll, 1);
        // Click row 0 = visual row 0 = wrapped line 1 ("bbb"), col 1 → pos 5
        assert_eq!(t.buffer_pos_at_screen(1, 0, area, state), Some(5));
        // Click row 1 = visual row 1 = wrapped line 2 ("ccc"), col 2 → pos 10
        assert_eq!(t.buffer_pos_at_screen(2, 1, area, state), Some(10));
    }

    #[test]
    fn buffer_pos_at_screen_wide_unicode() {
        // "a🦀b" — 🦀 is 2 columns wide (4 bytes)
        let t = ta_with("a🦀b");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // col 0 → 'a' at pos 0
        assert_eq!(t.buffer_pos_at_screen(0, 0, area, state), Some(0));
        // col 1 → first column of 🦀 → pos 1
        assert_eq!(t.buffer_pos_at_screen(1, 0, area, state), Some(1));
        // col 2 → second column of 🦀 → still pos 1 (within the 2-wide grapheme;
        // display_col_to_buffer_pos snaps to start of grapheme since target_col < width_so_far)
        // Actually: width_so_far after 'a' is 1, then 🦀 adds 2 → width_so_far=3 > target_col=2
        // → returns pos 1 (start of 🦀)
        assert_eq!(t.buffer_pos_at_screen(2, 0, area, state), Some(1));
        // col 3 → 'b' at pos 5 (1 + 4 bytes for 🦀)
        assert_eq!(t.buffer_pos_at_screen(3, 0, area, state), Some(5));
    }

    #[test]
    fn buffer_pos_at_screen_element_with_display() {
        // "ab" + element(buffer="raw_text", display="[X]") + "cd"
        // Display: "ab[X]cd" — element is at display cols 2..5
        let mut t = TextArea::new();
        t.insert_str("ab");
        let display = Line::from("[X]");
        t.insert_element("raw_text", ElementKind(0), Some(display));
        t.insert_str("cd");
        // Buffer: "abraw_textcd", element range 2..10
        assert_eq!(t.text(), "abraw_textcd");

        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();

        // col 0 → 'a' at pos 0
        assert_eq!(t.buffer_pos_at_screen(0, 0, area, state), Some(0));
        // col 1 → 'b' at pos 1
        assert_eq!(t.buffer_pos_at_screen(1, 0, area, state), Some(1));
        // col 2 → start of element display "[X]" → snap to element start (pos 2)
        assert_eq!(t.buffer_pos_at_screen(2, 0, area, state), Some(2));
        // col 3 → middle of element display → snap to nearest boundary
        // display width = 3, dist_start = 1, dist_end = 2 → snap to start (pos 2)
        assert_eq!(t.buffer_pos_at_screen(3, 0, area, state), Some(2));
        // col 4 → near end of element display → snap to end (pos 10)
        // dist_start = 2, dist_end = 1 → snap to end
        assert_eq!(t.buffer_pos_at_screen(4, 0, area, state), Some(10));
        // col 5 → 'c' at pos 10
        assert_eq!(t.buffer_pos_at_screen(5, 0, area, state), Some(10));
        // col 6 → 'd' at pos 11
        assert_eq!(t.buffer_pos_at_screen(6, 0, area, state), Some(11));
    }

    #[test]
    fn element_at_screen_hit_and_miss() {
        let mut t = TextArea::new();
        t.insert_str("ab");
        let display = Line::from("[File]");
        let id = t.insert_element("file.rs", ElementKind(1), Some(display));
        t.insert_str("cd");
        // Display: "ab[File]cd"

        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();

        // Click on 'a' (col 0) → no element
        assert!(t.element_at_screen(0, 0, area, state).is_none());
        // Click on 'b' (col 1) → no element
        assert!(t.element_at_screen(1, 0, area, state).is_none());
        // Click on element display (col 2) → element (snaps to start, pos 2 = element start)
        let elem = t.element_at_screen(2, 0, area, state);
        assert!(elem.is_some());
        assert_eq!(elem.unwrap().id, id);
        // Click on element display (col 3) → still the element
        assert_eq!(t.element_at_screen(3, 0, area, state).unwrap().id, id);
        // Click past element (col 8) → 'c' or 'd', no element
        assert!(t.element_at_screen(8, 0, area, state).is_none());
    }

    #[test]
    fn buffer_pos_at_screen_empty_textarea() {
        let t = TextArea::new();
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click on empty textarea → pos 0
        assert_eq!(t.buffer_pos_at_screen(0, 0, area, state), Some(0));
        // Click at col 5 → still pos 0 (end of empty text)
        assert_eq!(t.buffer_pos_at_screen(5, 0, area, state), Some(0));
    }

    // ── Mouse M2: Selection state + rendering tests ──

    #[test]
    fn selection_range_normalizes_anchor_head() {
        let mut t = ta_with("hello world");
        // anchor > head → range should be normalized to start..end
        t.set_selection(8, 3);
        let range = t.selection_range().unwrap();
        assert_eq!(range, 3..8);
    }

    #[test]
    fn selection_range_anchor_equals_head_is_none() {
        let mut t = ta_with("hello");
        t.set_selection(3, 3);
        assert!(t.selection_range().is_none());
    }

    #[test]
    fn selected_text_returns_buffer_substring() {
        let mut t = ta_with("hello world");
        t.set_selection(6, 11);
        assert_eq!(t.selected_text().unwrap(), "world");
    }

    #[test]
    fn selection_expands_to_element_boundaries() {
        let mut t = TextArea::new();
        t.insert_str("ab");
        t.insert_element("element_text", ElementKind(0), None);
        t.insert_str("cd");
        // Buffer: "abelement_textcd", element range 2..14
        assert_eq!(t.text(), "abelement_textcd");

        // Select only part of the element (bytes 5..10) → should expand to 2..14
        t.set_selection(5, 10);
        let range = t.selection_range().unwrap();
        assert_eq!(range.start, 2); // expanded to element start
        assert_eq!(range.end, 14); // expanded to element end
        assert_eq!(t.selected_text().unwrap(), "element_text");
    }

    #[test]
    fn clear_selection_clears() {
        let mut t = ta_with("hello");
        t.set_selection(0, 3);
        assert!(t.selection_range().is_some());
        t.clear_selection();
        assert!(t.selection_range().is_none());
    }

    #[test]
    fn take_clipboard_returns_and_clears() {
        let mut t = TextArea::new();
        t.set_clipboard_text("copied text".to_string());
        let text = t.take_clipboard();
        assert_eq!(text, Some("copied text".to_string()));
        // Second take returns None
        assert_eq!(t.take_clipboard(), None);
    }

    #[test]
    fn no_selection_returns_none() {
        let t = ta_with("hello");
        assert!(t.selection_range().is_none());
        assert!(t.selected_text().is_none());
    }

    #[test]
    fn selection_rendering_applies_default_selection_style() {
        let mut t = ta_with("hello");
        t.set_selection(1, 4); // select "ell"

        let area = Rect::new(0, 0, 10, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        let default_bg = Color::Rgb(49, 62, 115);
        let default_fg = Color::Rgb(192, 202, 245);
        // Cells 1, 2, 3 should have the default selection bg + fg
        for col in 1..4u16 {
            let cell = &buf[(col, 0)];
            assert_eq!(
                cell.bg, default_bg,
                "cell at col {col} should have default selection bg"
            );
            assert_eq!(
                cell.fg, default_fg,
                "cell at col {col} should have default selection fg"
            );
        }
        // Cell 0 ('h') and cell 4 ('o') should NOT have selection bg
        assert_ne!(buf[(0, 0)].bg, default_bg);
        assert_ne!(buf[(4, 0)].bg, default_bg);
    }

    // ── Phase 1: Undo/Redo plumbing tests ──

    #[test]
    fn undo_insert_chars_one_at_a_time() {
        let mut ta = TextArea::new();
        ta.insert_str("a");
        ta.insert_str("b");
        ta.insert_str("c");
        assert_eq!(ta.text(), "abc");
        assert_eq!(ta.cursor(), 3);

        // Phase 2: consecutive single-char inserts are batched into 1 undo step.
        assert!(ta.undo());
        assert_eq!(ta.text(), "");
        assert_eq!(ta.cursor(), 0);

        // Nothing left to undo.
        assert!(!ta.undo());
    }

    #[test]
    fn redo_after_undo_restores() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.insert_str(" ");
        ta.insert_str("world");
        assert_eq!(ta.text(), "hello world");

        // With word boundary batching: "hello" / " " / "world" = 3 steps.
        ta.undo(); // undo "world"
        assert_eq!(ta.text(), "hello ");
        ta.undo(); // undo " "
        assert_eq!(ta.text(), "hello");
        ta.undo(); // undo "hello"
        assert_eq!(ta.text(), "");

        // Redo walks forward.
        ta.redo();
        assert_eq!(ta.text(), "hello");
        ta.redo();
        assert_eq!(ta.text(), "hello ");
        ta.redo();
        assert_eq!(ta.text(), "hello world");
        assert_eq!(ta.cursor(), 11);
    }

    #[test]
    fn undo_via_super_modifier() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        assert_eq!(ta.text(), "hello");

        // Cmd+Z (SUPER) triggers undo.
        ta.input(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER));
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn redo_via_super_modifier() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.undo();
        assert_eq!(ta.text(), "");

        // Cmd+Shift+Z (SUPER, reported as uppercase Z) triggers redo.
        ta.input(KeyEvent::new(
            KeyCode::Char('Z'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        ));
        assert_eq!(ta.text(), "hello");
    }

    #[test]
    fn redo_cleared_by_new_mutation() {
        let mut ta = TextArea::new();
        ta.insert_str("abc");

        ta.undo(); // undo "abc" → ""
        assert_eq!(ta.text(), "");
        assert!(ta.can_redo());

        ta.insert_str("x"); // new mutation clears redo
        assert!(!ta.can_redo());
        assert_eq!(ta.text(), "x");
    }

    #[test]
    fn undo_delete_backward_restores_char() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.delete_backward(1); // "hell"
        assert_eq!(ta.text(), "hell");

        ta.undo(); // undo delete → "hello"
        assert_eq!(ta.text(), "hello");
        assert_eq!(ta.cursor(), 5);
    }

    #[test]
    fn undo_redo_preserves_cursor() {
        let mut ta = TextArea::new();
        ta.insert_str("abc");
        // cursor is at 3
        ta.set_cursor(1);
        ta.insert_str("X"); // "aXbc", cursor at 2
        assert_eq!(ta.text(), "aXbc");
        assert_eq!(ta.cursor(), 2);

        ta.undo(); // undo insert "X" → "abc", cursor at 1
        assert_eq!(ta.text(), "abc");
        assert_eq!(ta.cursor(), 1);

        ta.redo(); // redo → "aXbc", cursor at 2
        assert_eq!(ta.text(), "aXbc");
        assert_eq!(ta.cursor(), 2);
    }

    #[test]
    fn can_undo_can_redo_reflect_state() {
        let mut ta = TextArea::new();
        assert!(!ta.can_undo());
        assert!(!ta.can_redo());

        ta.insert_str("a");
        assert!(ta.can_undo());
        assert!(!ta.can_redo());

        ta.undo();
        assert!(!ta.can_undo());
        assert!(ta.can_redo());

        ta.redo();
        assert!(ta.can_undo());
        assert!(!ta.can_redo());
    }

    #[test]
    fn undo_stack_depth_capped() {
        let mut ta = TextArea::new();
        // Override max_depth for testing.
        ta.undo.max_depth = 5;

        // Use set_text (Replace — always discrete) to force separate undo steps.
        for i in 0..10 {
            ta.set_text(&format!("v{i}"));
        }
        assert_eq!(ta.text(), "v9");
        // Stack should be capped at 5.
        assert_eq!(ta.undo.stack.len(), 5);

        // We can undo at most 5 times.
        let mut count = 0;
        while ta.undo() {
            count += 1;
        }
        assert_eq!(count, 5);
        // We've undone 5 set_text calls, landing on the 5th oldest state.
        assert_eq!(ta.text(), "v4");
    }

    #[test]
    fn undo_set_text_restores_previous() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.set_text("new");
        assert_eq!(ta.text(), "new");

        ta.undo(); // undo set_text → "hello"
        assert_eq!(ta.text(), "hello");
    }

    #[test]
    fn undo_redo_multiple_round_trips() {
        let mut ta = TextArea::new();
        // Use separate insert kinds so they don't batch together.
        ta.insert_str("hello");
        ta.delete_backward(2); // "hel" — kind changes Insert→Delete, new undo step
        assert_eq!(ta.text(), "hel");

        ta.undo(); // undo delete → "hello"
        assert_eq!(ta.text(), "hello");

        ta.undo(); // undo insert → ""
        assert_eq!(ta.text(), "");

        ta.redo(); // redo insert → "hello"
        assert_eq!(ta.text(), "hello");

        ta.redo(); // redo delete → "hel"
        assert_eq!(ta.text(), "hel");

        // undo one, insert new → redo cleared
        ta.undo(); // undo delete → "hello"
        assert_eq!(ta.text(), "hello");
        ta.insert_str("z"); // "helloz" — new branch
        assert_eq!(ta.text(), "helloz");
        assert!(!ta.can_redo());

        // undo "z" — but wait, "z" extends the "hello" batch (same kind, consecutive cursor)?
        // No: undo reset last_kind=None, so "z" is a fresh group.
        ta.undo();
        assert_eq!(ta.text(), "hello");
    }

    // ── Phase 2: Batching tests ──

    #[test]
    fn batch_consecutive_inserts_into_one_undo_step() {
        // Typing "hello" char by char → batched into 1 undo step.
        let mut ta = TextArea::new();
        ta.insert_str("h");
        ta.insert_str("e");
        ta.insert_str("l");
        ta.insert_str("l");
        ta.insert_str("o");
        assert_eq!(ta.text(), "hello");
        assert_eq!(ta.undo.stack.len(), 1); // single checkpoint

        ta.undo();
        assert_eq!(ta.text(), "");
        assert!(!ta.undo());
    }

    #[test]
    fn multi_grapheme_delete_calls_are_single_undo_steps() {
        for forward in [false, true] {
            let mut ta = ta_with("hello");
            if forward {
                ta.set_cursor(0);
                ta.delete_forward(2);
                assert_eq!(ta.text(), "llo");
            } else {
                ta.delete_backward(2);
                assert_eq!(ta.text(), "hel");
            }
            assert!(ta.undo());
            assert_eq!(ta.text(), "hello");
        }
    }

    #[test]
    fn multi_count_deletes_cross_atomic_element_boundaries() {
        let mut backward = TextArea::new();
        backward.insert_str("a");
        backward.insert_element("TOKEN", ElementKind(1), None);
        backward.insert_str("b");
        backward.delete_backward(2);
        assert_eq!(backward.text(), "a");
        assert!(backward.elements().is_empty());
        assert!(backward.undo());
        assert_eq!(backward.text(), "aTOKENb");

        let mut forward = TextArea::new();
        forward.insert_str("a");
        forward.insert_element("TOKEN", ElementKind(1), None);
        forward.insert_str("b");
        forward.set_cursor(0);
        forward.delete_forward(2);
        assert_eq!(forward.text(), "b");
        assert!(forward.elements().is_empty());
        assert!(forward.undo());
        assert_eq!(forward.text(), "aTOKENb");
    }

    #[test]
    fn batch_consecutive_deletes_into_one_undo_step() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        // 5 backspaces — all Delete kind, consecutive cursor
        ta.delete_backward(1); // o
        ta.delete_backward(1); // l
        ta.delete_backward(1); // l
        ta.delete_backward(1); // e
        ta.delete_backward(1); // h
        assert_eq!(ta.text(), "");

        // 2 undo steps: 1 for insert batch, 1 for delete batch
        ta.undo(); // undo all deletes
        assert_eq!(ta.text(), "hello");

        ta.undo(); // undo insert
        assert_eq!(ta.text(), "");
        assert!(!ta.undo());
    }

    #[test]
    fn kind_change_breaks_batch() {
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        ta.delete_backward(1); // "hell" — kind changes → new step
        assert_eq!(ta.text(), "hell");

        // 2 undo steps
        ta.undo(); // undo delete
        assert_eq!(ta.text(), "hello");
        ta.undo(); // undo insert
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn cursor_jump_breaks_insert_batch() {
        let mut ta = TextArea::new();
        ta.insert_str("he"); // cursor at 2
        ta.set_cursor(0); // move cursor to 0 (no mutation, just movement)
        ta.insert_str("X"); // cursor was at 0, last_cursor was 2 → jump → new step
        assert_eq!(ta.text(), "Xhe");

        // 2 undo steps
        ta.undo(); // undo "X"
        assert_eq!(ta.text(), "he");
        ta.undo(); // undo "he"
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn kill_always_discrete() {
        let mut ta = TextArea::new();
        ta.insert_str("hello world");
        ta.set_cursor(5);
        ta.kill_to_end_of_line(); // kills " world"
        assert_eq!(ta.text(), "hello");
        ta.kill_to_end_of_line(); // kills nothing (already at EOL with no newline... wait)

        // Second kill at EOL does nothing (text.len() == cursor_pos).
        // So only 1 kill undo step.
        ta.undo(); // undo kill
        assert_eq!(ta.text(), "hello world");
    }

    #[test]
    fn kill_consecutive_each_own_step() {
        // Two kill operations back-to-back should be separate undo steps.
        let mut ta = TextArea::new();
        ta.insert_str("aaa bbb ccc");
        ta.set_cursor(7); // after "aaa bbb"
        ta.kill_to_end_of_line(); // kills " ccc" → "aaa bbb"
        assert_eq!(ta.text(), "aaa bbb");
        ta.set_cursor(3);
        ta.kill_to_end_of_line(); // kills " bbb" → "aaa"
        assert_eq!(ta.text(), "aaa");

        ta.undo(); // undo second kill
        assert_eq!(ta.text(), "aaa bbb");
        ta.undo(); // undo first kill
        assert_eq!(ta.text(), "aaa bbb ccc");
    }

    #[test]
    fn insert_str_multi_char_is_one_step() {
        // A single insert_str("hello world") call is 1 undo step.
        let mut ta = TextArea::new();
        ta.insert_str("hello world");
        assert_eq!(ta.undo.stack.len(), 1);

        ta.undo();
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn set_text_always_discrete() {
        let mut ta = TextArea::new();
        ta.set_text("first");
        ta.set_text("second");
        assert_eq!(ta.text(), "second");

        // Each set_text is its own undo step (Replace is always discrete)
        ta.undo();
        assert_eq!(ta.text(), "first");
        ta.undo();
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn insert_then_undo_then_insert_fresh_batch() {
        // After undo, last_kind is reset, so new inserts start a fresh batch.
        let mut ta = TextArea::new();
        ta.insert_str("ab");
        ta.undo(); // → ""
        ta.insert_str("cd");
        ta.insert_str("ef"); // should batch with "cd"
        assert_eq!(ta.text(), "cdef");

        ta.undo(); // undo "cdef" batch
        assert_eq!(ta.text(), "");
    }
