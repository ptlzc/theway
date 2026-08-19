    #[test]
    fn ctrl_a_skips_newline_inside_element() {
        let mut t = TextArea::new();
        t.insert_str("foo ");
        t.insert_element("line1\nline2", ElementKind(1), None);
        t.insert_str(" bar");
        // buffer = "foo line1\nline2 bar" (19 bytes)

        // Set cursor to end
        t.set_cursor(t.text().len());
        t.move_cursor_to_beginning_of_line(false);
        assert_eq!(t.cursor(), 0); // should reach beginning, not stop inside element
    }

    #[test]
    fn ctrl_e_from_element_boundary_skips_to_real_eol() {
        let mut t = TextArea::new();
        t.insert_str("foo ");
        t.insert_element("a\nb\nc", ElementKind(1), None);
        t.insert_str(" bar");
        // element at 4..9

        // Place cursor at element start boundary
        t.set_cursor(4);
        t.move_cursor_to_end_of_line(false);
        assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn ctrl_a_from_after_element_skips_to_real_bol() {
        let mut t = TextArea::new();
        t.insert_str("foo ");
        t.insert_element("a\nb\nc", ElementKind(1), None);
        t.insert_str(" bar");
        // element at 4..9, " bar" at 9..13

        // Place cursor on " bar"
        t.set_cursor(10);
        t.move_cursor_to_beginning_of_line(false);
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn kill_to_eol_with_multiline_element() {
        let mut t = TextArea::new();
        t.insert_str("foo ");
        t.insert_element("x\ny\nz", ElementKind(1), None);
        t.insert_str(" bar");
        // buffer = "foo x\ny\nz bar", element at 4..9

        t.set_cursor(0);
        t.kill_to_end_of_line();
        // Should kill everything on the line: "foo " + element + " bar"
        assert_eq!(t.text(), "");
    }

    #[test]
    fn kill_to_bol_with_multiline_element() {
        let mut t = TextArea::new();
        t.insert_str("foo ");
        t.insert_element("x\ny\nz", ElementKind(1), None);
        t.insert_str(" bar");

        t.set_cursor(t.text().len()); // end of " bar"
        t.kill_to_beginning_of_line();
        assert_eq!(t.text(), "");
    }

    #[test]
    fn bol_eol_with_real_newline_and_element() {
        // "hello\nfoo <element:a\nb> bar"
        // Two real lines. Element is on the second line.
        let mut t = TextArea::new();
        t.insert_str("hello\nfoo ");
        t.insert_element("a\nb", ElementKind(1), None);
        t.insert_str(" bar");
        // buffer = "hello\nfoo a\nb bar"
        // Real newline at 5. Element at 10..13. Element's \n at 11 should be skipped.

        // From start of second line (pos 6), Ctrl-E should reach end
        t.set_cursor(6);
        t.move_cursor_to_end_of_line(false);
        assert_eq!(t.cursor(), t.text().len());

        // From end, Ctrl-A should go back to pos 6
        t.move_cursor_to_beginning_of_line(false);
        assert_eq!(t.cursor(), 6);
    }

    #[test]
    fn bol_eol_no_element_unchanged() {
        // Verify the fix doesn't break normal (no-element) behavior.
        let mut t = TextArea::new();
        t.insert_str("line1\nline2\nline3");

        t.set_cursor(6); // start of "line2"
        t.move_cursor_to_end_of_line(false);
        assert_eq!(t.cursor(), 11); // end of "line2" (before \n)

        t.move_cursor_to_beginning_of_line(false);
        assert_eq!(t.cursor(), 6);
    }

    // ===== Element-display-aware wrapping =====

    #[test]
    fn wrapping_uses_element_display_width() {
        // Scenario: "foo bar " + element("Clean build", display 28 cols) at width 20.
        // Buffer text: "foo bar Clean build" = 19 buffer cols → textwrap says it fits on one line.
        // Display text: "foo bar [📎 Pasted 1 line, 11 chars]" = 8 + 28 = 36 display cols → should wrap.
        let mut t = TextArea::new();
        t.insert_str("foo bar ");
        let display = Line::from("[📎 Pasted 1 line, 11 chars]"); // 28 display cols
        t.insert_element("Clean build", ElementKind(0), Some(display));
        // buffer = "foo bar Clean build" (19 bytes)

        let lines = t.wrapped_lines(20);
        // The element display (28 cols) doesn't fit after "foo bar " (8 cols) on a 20-col line.
        // But it DOES fit on a fresh 20-col line (28 > 20, so it overflows but gets its own line).
        // Expected: line 1 = "foo bar ", line 2 = element.
        assert!(
            lines.len() >= 2,
            "Expected wrapping to produce at least 2 lines, got {} lines. \
             Line ranges: {:?}",
            lines.len(),
            *lines,
        );
    }

    #[test]
    fn wrapping_element_fits_on_next_line() {
        // Element display (10 cols) doesn't fit after "hello " (6 cols) on 12-col line,
        // but fits on a fresh line.
        let mut t = TextArea::new();
        t.insert_str("hello ");
        let display = Line::from("[Pasted!]"); // 9 display cols
        t.insert_element("xy", ElementKind(0), Some(display));
        t.insert_str(" z");
        // buffer: "hello xy z" (10 bytes)
        // display: "hello [Pasted!] z" = 6 + 9 + 2 = 17 display cols

        let lines = t.wrapped_lines(12);
        // Line 1: "hello " (6 cols, fits)
        // Line 2: "[Pasted!] z" (9 + 2 = 11 cols, fits in 12)
        assert_eq!(
            lines.len(),
            2,
            "Expected 2 wrapped lines, got {}. Ranges: {:?}",
            lines.len(),
            *lines,
        );
    }

    #[test]
    fn wrapping_element_without_display_uses_buffer_width() {
        // Element without display override: wrapping should use buffer text width (unchanged behavior).
        let mut t = TextArea::new();
        t.insert_str("hello ");
        t.insert_element("xy", ElementKind(0), None);
        t.insert_str(" z");
        // buffer: "hello xy z" (10 bytes), no display override
        // display = buffer = "hello xy z" = 10 cols

        let lines = t.wrapped_lines(12);
        // 10 cols fits on 12-col line → 1 line
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn wrapping_element_display_renders_on_correct_lines() {
        // End-to-end: wrapping + rendering with display element.
        // "abc " (4) + element("xy", display="[ELEM]" = 6 cols) + " d" (2)
        // At width 8: "abc " (4) + "[ELEM]" (6) = 10 > 8 → wrap before element
        // Line 1: "abc " (4 cols), Line 2: "[ELEM] d" (8 cols)
        let mut t = TextArea::new();
        t.insert_str("abc ");
        let display = Line::from("[ELEM]");
        t.insert_element("xy", ElementKind(0), Some(display));
        t.insert_str(" d");
        // buffer: "abc xy d" (8 bytes)

        // Check wrapping (drop the Ref before rendering)
        {
            let lines = t.wrapped_lines(8);
            assert_eq!(lines.len(), 2, "Should wrap into 2 lines, got {:?}", *lines);
        }

        // Render and verify
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        // Line 1 (y=0): "abc " padded to 8
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "a");
        assert_eq!(buf.cell((1, 0)).unwrap().symbol(), "b");
        assert_eq!(buf.cell((2, 0)).unwrap().symbol(), "c");
        assert_eq!(buf.cell((3, 0)).unwrap().symbol(), " ");

        // Line 2 (y=1): "[ELEM] d"
        assert_eq!(buf.cell((0, 1)).unwrap().symbol(), "[");
        assert_eq!(buf.cell((1, 1)).unwrap().symbol(), "E");
        assert_eq!(buf.cell((5, 1)).unwrap().symbol(), "]");
        assert_eq!(buf.cell((6, 1)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((7, 1)).unwrap().symbol(), "d");
    }

    #[test]
    fn wrapping_element_with_newlines_stays_single_line() {
        // When an element's buffer text contains \n, wrapping must NOT split at those
        // newlines. The element's display is a single-line chip; the \n is internal.
        // Scenario: "hello " + element("line1\nline2\nline3", display="[paste]") + " world"
        // Buffer: "hello line1\nline2\nline3 world"  (contains \n inside element)
        // Display: "hello [paste] world" = 6 + 7 + 6 = 19 cols
        // At width 40: should be 1 visual line.
        let mut t = TextArea::new();
        t.insert_str("hello ");
        let display = Line::from("[paste]"); // 7 display cols
        t.insert_element("line1\nline2\nline3", ElementKind(0), Some(display));
        t.insert_str(" world");
        // buffer: "hello line1\nline2\nline3 world"

        let lines = t.wrapped_lines(40);
        assert_eq!(
            lines.len(),
            1,
            "Element with internal \\n should NOT create extra visual lines. \
             Got {} lines: {:?}",
            lines.len(),
            *lines,
        );
    }

    #[test]
    fn cursor_pos_after_multiline_element() {
        // After inserting text after a multiline element, the cursor should be on the
        // same visual line as the element chip, not bumped down by internal newlines.
        let mut t = TextArea::new();
        t.insert_str("hello ");
        let display = Line::from("[paste]"); // 7 display cols
        t.insert_element("line1\nline2", ElementKind(0), Some(display));
        t.insert_str(" world");

        let area = Rect::new(0, 0, 80, 10);
        let pos = t.cursor_pos(area);
        assert_eq!(
            pos,
            Some((19, 0)), // 6 + 7 + 6 = 19, row 0
            "Cursor should be at col 19, row 0 after multiline element. \
             Got {:?}. Buffer: {:?}, cursor byte: {}",
            pos,
            t.text(),
            t.cursor(),
        );
    }

    #[test]
    fn yank_restores_last_kill() {
        let mut t = ta_with("hello");
        t.set_cursor(0);
        t.kill_to_end_of_line();
        assert_eq!(t.text(), "");
        assert_eq!(t.cursor(), 0);

        t.yank();
        assert_eq!(t.text(), "hello");
        assert_eq!(t.cursor(), 5);

        let mut t = ta_with("hello world");
        t.set_cursor(t.text().len());
        t.delete_backward_word();
        assert_eq!(t.text(), "hello ");
        assert_eq!(t.cursor(), 6);

        t.yank();
        assert_eq!(t.text(), "hello world");
        assert_eq!(t.cursor(), 11);

        let mut t = ta_with("hello");
        t.set_cursor(5);
        t.kill_to_beginning_of_line();
        assert_eq!(t.text(), "");
        assert_eq!(t.cursor(), 0);

        t.yank();
        assert_eq!(t.text(), "hello");
        assert_eq!(t.cursor(), 5);
    }

    #[test]
    fn no_op_kill_preserves_the_kill_buffer() {
        let mut textarea = ta_with("hello");
        textarea.set_cursor(0);
        textarea.kill_to_end_of_line();
        assert_eq!(textarea.kill_buffer, "hello");

        textarea.set_text("world");
        textarea.set_cursor(textarea.text().len());
        textarea.kill_to_end_of_line();
        textarea.yank();
        assert_eq!(textarea.text(), "worldhello");
    }

    #[test]
    fn kill_buffer_survives_set_text() {
        // A cut must outlive the buffer reset that send does via set_text("").
        let mut t = ta_with("hello");
        t.set_cursor(0);
        t.kill_to_end_of_line();
        assert_eq!(t.text(), "");

        t.set_text(""); // send resets the prompt
        assert_eq!(t.text(), "");

        t.yank();
        assert_eq!(t.text(), "hello");
        assert_eq!(t.cursor(), 5);
    }

    #[test]
    fn cursor_left_and_right_handle_graphemes() {
        let mut t = ta_with("a👍b");
        t.set_cursor(t.text().len());

        t.move_cursor_left(); // before 'b'
        let after_first_left = t.cursor();
        t.move_cursor_left(); // before '👍'
        let after_second_left = t.cursor();
        t.move_cursor_left(); // before 'a'
        let after_third_left = t.cursor();

        assert!(after_first_left < t.text().len());
        assert!(after_second_left < after_first_left);
        assert!(after_third_left < after_second_left);

        // Move right back to end safely
        t.move_cursor_right();
        t.move_cursor_right();
        t.move_cursor_right();
        assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn control_b_and_f_move_cursor() {
        let mut t = ta_with("abcd");
        t.set_cursor(1);

        t.input(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(t.cursor(), 2);

        t.input(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(t.cursor(), 1);
    }

    #[test]
    fn control_b_f_fallback_control_chars_move_cursor() {
        let mut t = ta_with("abcd");
        t.set_cursor(2);

        // Simulate terminals that send C0 control chars without CONTROL modifier.
        // ^B (U+0002) should move left
        t.input(KeyEvent::new(KeyCode::Char('\u{0002}'), KeyModifiers::NONE));
        assert_eq!(t.cursor(), 1);

        // ^F (U+0006) should move right
        t.input(KeyEvent::new(KeyCode::Char('\u{0006}'), KeyModifiers::NONE));
        assert_eq!(t.cursor(), 2);
    }

    /// Regression (user report): Ctrl+W must rubout to whitespace, not stop
    /// at punctuation.
    #[test]
    fn ctrl_w_unix_word_rubout_deletes_to_whitespace() {
        let mut t = ta_with("git commit -m hello-world");
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(t.text(), "git commit -m ");
        t.input(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(t.text(), "git commit ");
    }

    #[test]
    fn unix_word_rubout_whitespace_runs_paths_and_edges() {
        let mut t = ta_with("cat path/to/file.rs   ");
        t.set_cursor(t.text().len());
        t.delete_backward_unix_word();
        assert_eq!(t.text(), "cat ");
        assert_eq!(t.cursor(), 4);

        let mut t = ta_with("foo bar-baz");
        t.set_cursor(7); // foo bar|-baz
        t.delete_backward_unix_word();
        assert_eq!(t.text(), "foo -baz");
        assert_eq!(t.cursor(), 4);

        // Newlines are whitespace: rubout crosses line boundaries.
        let mut t = ta_with("line1\nword  ");
        t.set_cursor(t.text().len());
        t.delete_backward_unix_word();
        assert_eq!(t.text(), "line1\n");

        let mut t = ta_with("");
        t.delete_backward_unix_word();
        assert_eq!(t.text(), "");
        let mut t = ta_with("   ");
        t.set_cursor(3);
        t.delete_backward_unix_word();
        assert_eq!(t.text(), "");
    }

    /// Readline parity: only C-w is whitespace-delimited; M-DEL/C-Backspace
    /// stay chunked.
    #[test]
    fn alt_backspace_keeps_word_chunk_semantics() {
        let mut t = ta_with("hello-world");
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(t.text(), "hello-");

        let mut t = ta_with("hello-world");
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(t.text(), "hello-");
    }

    #[test]
    fn delete_backward_word_alt_keys() {
        // Test the custom Alt+Ctrl+h binding
        let mut t = ta_with("hello world");
        t.set_cursor(t.text().len()); // cursor at the end
        t.input(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(t.text(), "hello ");
        assert_eq!(t.cursor(), 6);

        // Test the standard Alt+Backspace binding
        let mut t = ta_with("hello world");
        t.set_cursor(t.text().len()); // cursor at the end
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(t.text(), "hello ");
        assert_eq!(t.cursor(), 6);
    }

    #[test]
    fn ctrl_backspace_deletes_backward_word() {
        let mut t = ta_with("hello world");
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(t.text(), "hello ");
        assert_eq!(t.cursor(), 6);

        // From end of middle word: deletes "bar", leaves surrounding spaces
        let mut t = ta_with("foo bar baz");
        t.set_cursor(7); // after "bar"
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL));
        assert_eq!(t.text(), "foo  baz");
        assert_eq!(t.cursor(), 4);
    }

    #[test]
    fn ctrl_delete_deletes_forward_word() {
        // Mirror of ctrl_backspace_deletes_backward_word.
        let mut t = ta_with("hello world");
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
        assert_eq!(t.text(), " world");
        assert_eq!(t.cursor(), 0);

        // From start of middle word: deletes "bar", leaves surrounding spaces
        let mut t = ta_with("foo bar baz");
        t.set_cursor(4); // before "bar"
        t.input(KeyEvent::new(KeyCode::Delete, KeyModifiers::CONTROL));
        assert_eq!(t.text(), "foo  baz");
        assert_eq!(t.cursor(), 4);
    }

    #[test]
    fn delete_backward_word_handles_narrow_no_break_space() {
        let mut t = ta_with("32\u{202F}AM");
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        pretty_assertions::assert_eq!(t.text(), "32\u{202F}");
        pretty_assertions::assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn delete_forward_word_with_without_alt_modifier() {
        let mut t = ta_with("hello world");
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Delete, KeyModifiers::ALT));
        assert_eq!(t.text(), " world");
        assert_eq!(t.cursor(), 0);

        let mut t = ta_with("hello");
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(t.text(), "ello");
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn alt_d_deletes_forward_word() {
        // Alt+D (Meta-d, Emacs) → delete forward word
        let mut t = ta_with("hello world foo");
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));
        assert_eq!(t.text(), " world foo");
        assert_eq!(t.cursor(), 0);

        // Alt+D at a word boundary
        let mut t = ta_with("hello world");
        t.set_cursor(5); // cursor right after "hello"
        t.input(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));
        assert_eq!(t.text(), "hello");
        assert_eq!(t.cursor(), 5);

        // Super+D (Cmd+D on macOS with Kitty protocol) also works
        let mut t = ta_with("hello world");
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::SUPER));
        assert_eq!(t.text(), " world");
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn ctrl_p_moves_cursor_up() {
        let mut t = ta_with("first\nsecond\nthird");
        let second_line_start = 6; // after "first\n"
        t.set_cursor(second_line_start + 2); // middle of "second"
        t.input(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        // Should be on first line now
        assert!(t.cursor() < second_line_start);
    }

    #[test]
    fn ctrl_n_moves_cursor_down() {
        let mut t = ta_with("first\nsecond\nthird");
        t.set_cursor(2); // middle of "first"
        t.input(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        let second_line_start = 6;
        // Should be on second line now
        assert!(t.cursor() >= second_line_start);
    }

    #[test]
    fn control_h_backspace() {
        // Test Ctrl+H as backspace
        let mut t = ta_with("12345");
        t.set_cursor(3); // cursor after '3'
        t.input(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(t.text(), "1245");
        assert_eq!(t.cursor(), 2);

        // Test Ctrl+H at beginning (should be no-op)
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(t.text(), "1245");
        assert_eq!(t.cursor(), 0);

        // Test Ctrl+H at end
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(t.text(), "124");
        assert_eq!(t.cursor(), 3);
    }
    #[test]
    fn char_bs_backspace() {
        // Test Char('\x08') (BS) as backspace
        let mut t = ta_with("12345");
        t.set_cursor(3); // cursor after '3'
        t.input(KeyEvent::new(KeyCode::Char('\x08'), KeyModifiers::NONE));
        assert_eq!(t.text(), "1245");
        assert_eq!(t.cursor(), 2);
    }

    #[test]
    fn char_del_deletes_backward() {
        // Char('\x7f') (DEL) should delete backward — on Unix terminals,
        // Backspace sends 0x7F in legacy mode (no Kitty protocol).
        let mut t = ta_with("12345");
        t.set_cursor(2); // cursor after '2'
        t.input(KeyEvent::new(KeyCode::Char('\x7f'), KeyModifiers::NONE));
        assert_eq!(t.text(), "1345");
        assert_eq!(t.cursor(), 1);
    }

    #[test]
    fn raw_delete_chars_ignore_stray_modifiers() {
        for raw in ['\u{0008}', '\u{007f}'] {
            for modifiers in [
                KeyModifiers::ALT,
                KeyModifiers::CONTROL,
                KeyModifiers::SUPER,
                KeyModifiers::ALT | KeyModifiers::CONTROL,
            ] {
                let mut t = ta_with("alpha beta");
                t.input(KeyEvent::new(KeyCode::Char(raw), modifiers));
                assert_eq!(
                    t.text(),
                    "alpha bet",
                    "raw {raw:?} with {modifiers:?} must delete one grapheme",
                );
            }
        }
    }

    #[test]
    fn del_char_treated_as_backspace() {
        // When Kitty keyboard protocol gets silently popped, Backspace can
        // arrive as raw DEL (0x7F) instead of KeyCode::Backspace. Ensure it
        // deletes backward instead of inserting an invisible character.
        let mut t = ta_with("hello");
        t.set_cursor(3);
        t.input(KeyEvent::new(KeyCode::Char('\u{007f}'), KeyModifiers::NONE));
        assert_eq!(t.text(), "helo");
        assert_eq!(t.cursor(), 2);

        // At beginning: no-op
        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Char('\u{007f}'), KeyModifiers::NONE));
        assert_eq!(t.text(), "helo");
        assert_eq!(t.cursor(), 0);

        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(KeyCode::Char('\u{007f}'), KeyModifiers::ALT));
        assert_eq!(t.text(), "hel");
        assert_eq!(t.cursor(), 3);
    }

    #[test]
    fn bs_char_treated_as_backspace() {
        // BS (0x08) arriving as Char without CONTROL modifier should also
        // delete backward (Ctrl-H without the modifier flag).
        let mut t = ta_with("abcde");
        t.set_cursor(4);
        t.input(KeyEvent::new(KeyCode::Char('\u{0008}'), KeyModifiers::NONE));
        assert_eq!(t.text(), "abce");
        assert_eq!(t.cursor(), 3);
    }

    #[test]
    fn del_char_with_selection_deletes_selection() {
        // DEL (0x7F) arriving as Char with an active selection should delete
        // the selection cleanly, not insert an invisible control character.
        let mut t = ta_with("hello world");
        t.set_selection(0, 5);
        t.input(KeyEvent::new(KeyCode::Char('\u{007f}'), KeyModifiers::NONE));
        assert_eq!(t.text(), " world");
        assert_eq!(t.cursor(), 0);
        assert!(t.selection_range().is_none());
    }

    #[test]
    fn cursor_vertical_movement_across_lines_and_bounds() {
        let mut t = ta_with("short\nloooooooooong\nmid");
        // Place cursor on second line, column 5
        let second_line_start = 6; // after first '\n'
        t.set_cursor(second_line_start + 5);

        // Move up: target column preserved, clamped by line length
        t.move_cursor_up();
        assert_eq!(t.cursor(), 5); // first line has len 5

        // Move up again goes to start of text
        t.move_cursor_up();
        assert_eq!(t.cursor(), 0);

        // Move down: from start to target col tracked
        t.move_cursor_down();
        // On first move down, we should land on second line, at col 0 (target col remembered as 0)
        let pos_after_down = t.cursor();
        assert!(pos_after_down >= second_line_start);

        // Move down again to third line; clamp to its length
        t.move_cursor_down();
        let third_line_start = t.text().find("mid").unwrap();
        let third_line_end = third_line_start + 3;
        assert!(t.cursor() >= third_line_start && t.cursor() <= third_line_end);

        // Moving down at last line jumps to end
        t.move_cursor_down();
        assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn home_end_and_emacs_style_home_end() {
        let mut t = ta_with("one\ntwo\nthree");
        // Position at middle of second line
        let second_line_start = t.text().find("two").unwrap();
        t.set_cursor(second_line_start + 1);

        t.move_cursor_to_beginning_of_line(false);
        assert_eq!(t.cursor(), second_line_start);

        // Ctrl-A behavior: if at BOL, go to beginning of previous line
        t.move_cursor_to_beginning_of_line(true);
        assert_eq!(t.cursor(), 0); // beginning of first line

        // Move to EOL of first line
        t.move_cursor_to_end_of_line(false);
        assert_eq!(t.cursor(), 3);

        // Ctrl-E: if at EOL, go to end of next line
        t.move_cursor_to_end_of_line(true);
        // end of second line ("two") is right before its '\n'
        let end_second_nl = t.text().find("\nthree").unwrap();
        assert_eq!(t.cursor(), end_second_nl);
    }
