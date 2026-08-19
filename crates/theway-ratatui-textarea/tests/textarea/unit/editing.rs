    #[test]
    fn canonical_adapter_matches_standalone_edit_buffer() {
        let cases = [
            (
                "hello-world",
                "hello-world".len(),
                EditCommand::MoveWordLeft(WordStyle::Small),
            ),
            (
                "hello-world",
                0,
                EditCommand::MoveWordRight(WordStyle::Small),
            ),
            (
                "foo bar",
                "foo bar".len(),
                EditCommand::DeleteWordBackward(WordStyle::Small),
            ),
            (
                "foo bar",
                0,
                EditCommand::DeleteWordForward(WordStyle::Small),
            ),
            ("one\ntwo", 4, EditCommand::MoveLogicalLineStart),
            ("one\ntwo", 3, EditCommand::MoveLogicalLineEnd),
            ("abc", 2, EditCommand::DeleteGraphemeBackward),
            ("abc", 1, EditCommand::DeleteGraphemeForward),
        ];

        for (text, cursor, command) in cases {
            let mut textarea = TextArea::new();
            textarea.set_text(text);
            textarea.clear_history();
            textarea.set_cursor(cursor);
            let mut buffer = EditBuffer::from_parts(text, cursor);

            textarea.apply_classified_command(command);
            let _ = buffer.apply(command);

            assert_eq!(textarea.text(), buffer.text());
            assert_eq!(textarea.cursor(), buffer.cursor_byte());
        }
    }

    #[test]
    fn canonical_adapter_updates_selection_from_applied_delta() {
        let mut textarea = ta_with("abcdef");
        textarea.set_selection(4, 6);
        textarea.replace_range(0..2, "X");
        assert_eq!(textarea.text(), "Xcdef");
        assert_eq!(textarea.selection_range(), Some(3..5));
    }

    #[test]
    fn canonical_adapter_applies_same_byte_metadata_edits_with_history() {
        let mut textarea = TextArea::new();
        let id = textarea.insert_element("TOKEN", ElementKind(1), None);
        textarea.set_selection(0, 5);
        textarea.clear_history();

        textarea.replace_range(0..5, "TOKEN");
        assert_eq!(textarea.text(), "TOKEN");
        assert!(textarea.elements().is_empty());
        assert!(textarea.selection.is_none());
        assert!(textarea.can_undo());

        assert!(textarea.undo());
        assert_eq!(textarea.text(), "TOKEN");
        assert_eq!(textarea.elements().len(), 1);
        assert_eq!(textarea.elements()[0].id, id);
        assert!(textarea.redo());
        assert_eq!(textarea.text(), "TOKEN");
        assert!(textarea.elements().is_empty());
    }

    #[test]
    fn replace_element_forces_cursor_end_and_restores_metadata() {
        let mut before = ta_with("left TOKEN right");
        before.clear_history();
        before.set_cursor(0);
        let id = before.replace_range_with_element(5..10, "NODE", ElementKind(1), None);
        let end = 5 + "NODE".len();
        assert_eq!(before.cursor(), end);
        assert_eq!(before.elements()[0].id, id);

        assert!(before.undo());
        assert_eq!(before.text(), "left TOKEN right");
        assert!(before.elements().is_empty());
        assert_eq!(before.cursor(), 0);
        assert!(before.redo());
        assert_eq!(before.text(), "left NODE right");
        assert_eq!(before.elements()[0].id, id);
        assert_eq!(before.cursor(), end);

        let mut after = ta_with("left TOKEN right");
        after.clear_history();
        after.set_cursor(after.text().len());
        after.replace_range_with_element(5..10, "NODE", ElementKind(1), None);
        assert_eq!(after.cursor(), end);
    }

    #[test]
    fn empty_set_text_invalidates_redo_and_is_undoable() {
        let mut textarea = TextArea::new();
        textarea.insert_str("x");
        assert!(textarea.undo());
        assert!(textarea.can_redo());

        textarea.set_text("");
        assert!(!textarea.can_redo());
        assert!(textarea.can_undo());
        assert_eq!(textarea.cursor(), 0);
    }

    #[test]
    fn set_text_preserves_cursor_clamped_across_grow_and_shrink() {
        let mut grow = ta_with("abcd");
        grow.set_cursor(2);
        grow.set_text("abcdefgh");
        assert_eq!(grow.cursor(), 2);

        let mut shrink = ta_with("abcdefgh");
        shrink.set_cursor(6);
        shrink.set_text("abc");
        assert_eq!(shrink.cursor(), 3);
    }

    #[test]
    fn set_text_restores_zero_length_element_metadata_through_history() {
        let mut textarea = TextArea::new();
        let id = textarea.insert_element("", ElementKind(7), None);
        textarea.clear_history();

        textarea.set_text("");
        assert!(textarea.elements().is_empty());
        assert!(textarea.undo());
        assert_eq!(textarea.text(), "");
        assert_eq!(textarea.elements().len(), 1);
        assert_eq!(textarea.elements()[0].id, id);
        assert_eq!(textarea.elements()[0].range, 0..0);
        assert!(textarea.redo());
        assert!(textarea.elements().is_empty());
    }

    #[test]
    fn rejected_adapter_plan_has_no_side_effects() {
        let mut textarea = TextArea::new();
        let id = textarea.insert_element("TOKEN", ElementKind(1), None);
        textarea.set_selection(0, 5);
        textarea.kill_buffer = "sentinel".to_owned();
        textarea.preferred_col = Some(3);
        textarea.scroll_override = Some(2);
        let _ = textarea.desired_height(20);
        textarea.clear_history();
        let plan = textarea.plan_edit_replacement(0..5, "X");
        let _ = textarea.text.set_cursor_byte(0);

        let result = textarea.try_apply_edit_plan(plan, Some(MutationKind::Replace));
        assert_eq!(result, Err(ApplyEditPlanError::StalePlan));
        assert_eq!(textarea.text(), "TOKEN");
        assert_eq!(textarea.elements().len(), 1);
        assert_eq!(textarea.elements()[0].id, id);
        assert_eq!(textarea.selection_range(), Some(0..5));
        assert_eq!(textarea.kill_buffer, "sentinel");
        assert_eq!(textarea.preferred_col, Some(3));
        assert_eq!(textarea.scroll_override, Some(2));
        assert!(textarea.wrap_cache.borrow().is_some());
        assert!(!textarea.can_undo());
    }

    #[test]
    fn handled_boundary_navigation_clears_vertical_affinity() {
        let mut textarea = ta_with("ab\nwxyz");
        textarea.set_cursor(0);
        textarea.preferred_col = Some(3);
        textarea.scroll_override = Some(2);

        textarea.move_cursor_left();
        assert_eq!(textarea.cursor(), 0);
        assert_eq!(textarea.preferred_col, None);
        assert_eq!(textarea.scroll_override, None);

        textarea.move_cursor_down();
        assert_eq!(textarea.cursor(), 3);
    }

    #[test]
    fn insert_str_at_inside_element_clamps_to_an_atomic_boundary() {
        let mut textarea = TextArea::new();
        textarea.insert_str("a");
        textarea.insert_element("TOKEN", ElementKind(1), None);
        textarea.insert_str("b");
        textarea.clear_history();

        textarea.insert_str_at(3, "X");
        assert_eq!(textarea.text(), "aXTOKENb");
        assert_eq!(textarea.cursor(), 8);
        assert_eq!(textarea.elements()[0].range, 2..7);
        assert!(textarea.undo());
        assert_eq!(textarea.text(), "aTOKENb");
        assert_eq!(textarea.elements()[0].range, 1..6);
    }

    #[test]
    fn canonical_adapter_keeps_elements_atomic_for_motion_and_deletion() {
        let mut backward = TextArea::new();
        backward.insert_str("a");
        let id = backward.insert_element("TOKEN", ElementKind(1), None);
        backward.insert_str("b");
        let range = backward.elements()[0].range.clone();
        backward.set_cursor(range.end);
        backward.move_cursor_left();
        assert_eq!(backward.cursor(), range.start);
        backward.move_cursor_right();
        assert_eq!(backward.cursor(), range.end);
        backward.delete_backward(1);
        assert_eq!(backward.text(), "ab");
        assert!(backward.elements().iter().all(|element| element.id != id));

        let mut forward = TextArea::new();
        forward.insert_str("a");
        forward.insert_element("TOKEN", ElementKind(1), None);
        forward.insert_str("b");
        let range = forward.elements()[0].range.clone();
        forward.set_cursor(range.start);
        forward.delete_forward(1);
        assert_eq!(forward.text(), "ab");
        assert!(forward.elements().is_empty());
    }

    #[test]
    fn canonical_adapter_ignores_element_newlines_and_restores_kills() {
        let mut textarea = TextArea::new();
        textarea.insert_str("a");
        textarea.insert_element("X\nY", ElementKind(1), None);
        textarea.insert_str("b\nc");
        textarea.clear_history();

        textarea.set_cursor(0);
        textarea.move_cursor_to_end_of_line(true);
        assert_eq!(textarea.cursor(), 5);
        textarea.set_cursor(0);
        textarea.kill_to_end_of_line();
        assert_eq!(textarea.text(), "\nc");
        assert_eq!(textarea.kill_buffer, "aX\nYb");

        assert!(textarea.undo());
        assert_eq!(textarea.text(), "aX\nYb\nc");
        assert_eq!(textarea.elements().len(), 1);
        assert!(textarea.redo());
        assert_eq!(textarea.text(), "\nc");
        assert!(textarea.elements().is_empty());
    }

    #[test]
    fn canonical_adapter_preserves_right_affinity_through_undo_redo() {
        let woman = "👩";
        let tail = "👩🏽\u{200d}💻";
        let original = format!("{woman}{tail}");
        let mut textarea = ta_with(&original);
        textarea.clear_history();
        textarea.set_cursor(woman.len());

        textarea.insert_str("\u{200d}");
        assert_eq!(textarea.text().graphemes(true).count(), 1);
        assert_eq!(textarea.cursor(), textarea.text().len());

        assert!(textarea.undo());
        assert_eq!(textarea.text(), original);
        assert_eq!(textarea.cursor(), woman.len());
        assert!(textarea.redo());
        assert_eq!(textarea.text().graphemes(true).count(), 1);
        assert_eq!(textarea.cursor(), textarea.text().len());
    }

    #[test]
    fn is_undo_input_accepts_ctrl_and_cmd_z() {
        assert!(is_undo_input(&KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::CONTROL
        )));
        assert!(is_undo_input(&KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::SUPER
        )));
    }

    #[test]
    fn is_undo_input_rejects_redo_and_plain_z() {
        // Uppercase 'Z' (redo) stays excluded so the guard is disjoint from
        // the redo arm regardless of match order.
        assert!(!is_undo_input(&KeyEvent::new(
            KeyCode::Char('Z'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        // A bare 'z' (no chord modifier) is plain typing, not undo.
        assert!(!is_undo_input(&KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::NONE
        )));
        assert!(!is_undo_input(&KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn insert_and_replace_update_cursor_and_text() {
        // insert helpers
        let mut t = ta_with("hello");
        t.set_cursor(5);
        t.insert_str("!");
        assert_eq!(t.text(), "hello!");
        assert_eq!(t.cursor(), 6);

        t.insert_str_at(0, "X");
        assert_eq!(t.text(), "Xhello!");
        assert_eq!(t.cursor(), 7);

        // Insert after the cursor should not move it
        t.set_cursor(1);
        let end = t.text().len();
        t.insert_str_at(end, "Y");
        assert_eq!(t.text(), "Xhello!Y");
        assert_eq!(t.cursor(), 1);

        // replace_range cases
        // 1) cursor before range
        let mut t = ta_with("abcd");
        t.set_cursor(1);
        t.replace_range(2..3, "Z");
        assert_eq!(t.text(), "abZd");
        assert_eq!(t.cursor(), 1);

        // 2) cursor inside range
        let mut t = ta_with("abcd");
        t.set_cursor(2);
        t.replace_range(1..3, "Q");
        assert_eq!(t.text(), "aQd");
        assert_eq!(t.cursor(), 2);

        // 3) cursor after range with shifted by diff
        let mut t = ta_with("abcd");
        t.set_cursor(4);
        t.replace_range(0..1, "AA");
        assert_eq!(t.text(), "AAbcd");
        assert_eq!(t.cursor(), 5);
    }

    #[test]
    fn delete_backward_and_forward_edges() {
        let mut t = ta_with("abc");
        t.set_cursor(1);
        t.delete_backward(1);
        assert_eq!(t.text(), "bc");
        assert_eq!(t.cursor(), 0);

        // deleting backward at start is a no-op
        t.set_cursor(0);
        t.delete_backward(1);
        assert_eq!(t.text(), "bc");
        assert_eq!(t.cursor(), 0);

        // forward delete removes next grapheme
        t.set_cursor(1);
        t.delete_forward(1);
        assert_eq!(t.text(), "b");
        assert_eq!(t.cursor(), 1);

        // forward delete at end is a no-op
        t.set_cursor(t.text().len());
        t.delete_forward(1);
        assert_eq!(t.text(), "b");
    }

    #[test]
    fn delete_backward_word_and_kill_line_variants() {
        // delete backward word at end removes the whole previous word
        let mut t = ta_with("hello   world  ");
        t.set_cursor(t.text().len());
        t.delete_backward_word();
        assert_eq!(t.text(), "hello   ");
        assert_eq!(t.cursor(), 8);

        // From inside a word, delete from word start to cursor
        let mut t = ta_with("foo bar");
        t.set_cursor(6); // inside "bar" (after 'a')
        t.delete_backward_word();
        assert_eq!(t.text(), "foo r");
        assert_eq!(t.cursor(), 4);

        // From end, delete the last word only
        let mut t = ta_with("foo bar");
        t.set_cursor(t.text().len());
        t.delete_backward_word();
        assert_eq!(t.text(), "foo ");
        assert_eq!(t.cursor(), 4);

        let mut t = ta_with("hello-world");
        t.set_cursor(t.text().len());
        t.delete_backward_word();
        assert_eq!(t.text(), "hello-");
        assert_eq!(t.cursor(), "hello-".len());

        // kill_to_end_of_line when not at EOL
        let mut t = ta_with("abc\ndef");
        t.set_cursor(1); // on first line, middle
        t.kill_to_end_of_line();
        assert_eq!(t.text(), "a\ndef");
        assert_eq!(t.cursor(), 1);

        // kill_to_end_of_line when at EOL deletes newline
        let mut t = ta_with("abc\ndef");
        t.set_cursor(3); // EOL of first line
        t.kill_to_end_of_line();
        assert_eq!(t.text(), "abcdef");
        assert_eq!(t.cursor(), 3);

        // kill_to_beginning_of_line from middle of line
        let mut t = ta_with("abc\ndef");
        t.set_cursor(5); // on second line, after 'e'
        t.kill_to_beginning_of_line();
        assert_eq!(t.text(), "abc\nef");

        // kill_to_beginning_of_line at beginning of non-first line removes the previous newline
        let mut t = ta_with("abc\ndef");
        t.set_cursor(4); // beginning of second line
        t.kill_to_beginning_of_line();
        assert_eq!(t.text(), "abcdef");
        assert_eq!(t.cursor(), 3);

        // kill_current_line from middle of single line
        let mut t = ta_with("hello world");
        t.set_cursor(5);
        t.kill_current_line();
        assert_eq!(t.text(), "");
        assert_eq!(t.cursor(), 0);

        // kill_current_line from middle of multiline
        let mut t = ta_with("abc\ndef\nghi");
        t.set_cursor(5);
        t.kill_current_line();
        assert_eq!(t.text(), "abc\n\nghi");
        assert_eq!(t.cursor(), 4);

        // kill_current_line on empty line joins with previous
        let mut t = ta_with("abc\n\nghi");
        t.set_cursor(4);
        t.kill_current_line();
        assert_eq!(t.text(), "abc\nghi");
        assert_eq!(t.cursor(), 3);

        // kill_current_line at beginning of only line
        let mut t = ta_with("hello");
        t.set_cursor(0);
        t.kill_current_line();
        assert_eq!(t.text(), "");
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn delete_forward_word_variants() {
        let mut t = ta_with("hello   world ");
        t.set_cursor(0);
        t.delete_forward_word();
        assert_eq!(t.text(), "   world ");
        assert_eq!(t.cursor(), 0);

        let mut t = ta_with("hello   world ");
        t.set_cursor(1);
        t.delete_forward_word();
        assert_eq!(t.text(), "h   world ");
        assert_eq!(t.cursor(), 1);

        let mut t = ta_with("hello   world");
        t.set_cursor(t.text().len());
        t.delete_forward_word();
        assert_eq!(t.text(), "hello   world");
        assert_eq!(t.cursor(), t.text().len());

        let mut t = ta_with("foo   \nbar");
        t.set_cursor(3);
        t.delete_forward_word();
        assert_eq!(t.text(), "foo");
        assert_eq!(t.cursor(), 3);

        let mut t = ta_with("foo\nbar");
        t.set_cursor(3);
        t.delete_forward_word();
        assert_eq!(t.text(), "foo");
        assert_eq!(t.cursor(), 3);

        let mut t = ta_with("hello-world");
        t.set_cursor(0);
        t.delete_forward_word();
        assert_eq!(t.text(), "-world");
        assert_eq!(t.cursor(), 0);

        let mut t = ta_with("hello   world ");
        t.set_cursor(t.text().len() + 10);
        t.delete_forward_word();
        assert_eq!(t.text(), "hello   world ");
        assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn super_right_moves_to_end_of_line() {
        let mut t = ta_with("hello world\nsecond line");
        t.set_cursor(3); // middle of "hello world"
        t.input(KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER));
        assert_eq!(t.cursor(), 11); // end of "hello world" (before \n)

        // Already at end of line → stays there
        t.input(KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER));
        assert_eq!(t.cursor(), 11);
    }

    #[test]
    fn super_left_moves_to_beginning_of_line() {
        let mut t = ta_with("hello world\nsecond line");
        let second_line_start = t.text().find("second").unwrap();
        t.set_cursor(second_line_start + 4); // middle of "second line"
        t.input(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER));
        assert_eq!(t.cursor(), second_line_start);

        // Already at beginning of line → stays there
        t.input(KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER));
        assert_eq!(t.cursor(), second_line_start);
    }

    #[test]
    fn super_backspace_kills_to_beginning_of_line() {
        let mut t = ta_with("hello world\nsecond line");
        let second_line_start = t.text().find("second").unwrap();
        t.set_cursor(second_line_start + 7); // after "second "
        t.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER));
        assert_eq!(t.text(), "hello world\nline");
        assert_eq!(t.cursor(), second_line_start);
    }

    #[test]
    fn ctrl_u_kills_to_beginning_of_line_keeps_text_after_cursor() {
        let mut t = ta_with("hello world");
        t.set_cursor(5);
        t.input(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(t.text(), " world");
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn delete_forward_word_handles_atomic_elements() {
        let kind = ElementKind(0);

        let mut t = TextArea::new();
        t.insert_element("<element>", kind, None);
        t.insert_str(" tail");

        t.set_cursor(0);
        t.delete_forward_word();
        assert_eq!(t.text(), " tail");
        assert_eq!(t.cursor(), 0);

        let mut t = TextArea::new();
        t.insert_str("   ");
        t.insert_element("<element>", kind, None);
        t.insert_str(" tail");

        t.set_cursor(0);
        t.delete_forward_word();
        assert_eq!(t.text(), " tail");
        assert_eq!(t.cursor(), 0);

        let mut t = TextArea::new();
        t.insert_str("prefix ");
        t.insert_element("<element>", kind, None);
        t.insert_str(" tail");

        // cursor in the middle of the element, delete_forward_word deletes the element
        let elem_range = t.elements()[0].range.clone();
        let _ = t
            .text
            .set_cursor_byte(elem_range.start + (elem_range.len() / 2));
        t.delete_forward_word();
        assert_eq!(t.text(), "prefix  tail");
        assert_eq!(t.cursor(), elem_range.start);
    }

    // ===== Phase 1: Typed element tests =====

    #[test]
    fn element_id_is_unique_and_stable() {
        let mut t = TextArea::new();
        let kind = ElementKind(1);

        let id1 = t.insert_element("aaa", kind, None);
        let id2 = t.insert_element("bbb", kind, None);
        assert_ne!(id1, id2);

        // ids survive after deletion of the first element
        t.set_cursor(0);
        t.delete_forward(1); // deletes "aaa" atomically
        assert_eq!(t.elements().len(), 1);
        assert_eq!(t.elements()[0].id, id2);
    }

    #[test]
    fn element_kind_preserved() {
        let mut t = TextArea::new();
        let kind_paste = ElementKind(1);
        let kind_file = ElementKind(2);

        t.insert_element("paste", kind_paste, None);
        t.insert_element("file", kind_file, None);

        assert_eq!(t.elements()[0].kind, kind_paste);
        assert_eq!(t.elements()[1].kind, kind_file);
    }

    #[test]
    fn element_at_cursor_returns_element() {
        let mut t = TextArea::new();
        let kind = ElementKind(0);

        t.insert_str("before ");
        let id = t.insert_element("[paste]", kind, None);
        t.insert_str(" after");

        // Cursor is at end of element after insert_element
        // Move to start of element
        t.set_cursor(7); // "before " is 7 bytes, element starts at 7
        let elem = t.element_at_cursor().expect("should find element");
        assert_eq!(elem.id, id);
        assert_eq!(elem.kind, kind);

        // Cursor before element
        t.set_cursor(0);
        assert!(t.element_at_cursor().is_none());

        // Cursor after element
        t.set_cursor(t.text().len());
        assert!(t.element_at_cursor().is_none());
    }

    #[test]
    fn element_text_returns_buffer_text() {
        let mut t = TextArea::new();
        let id = t.insert_element("raw buffer content", ElementKind(0), None);
        assert_eq!(t.element_text(id), Some("raw buffer content"));

        // Non-existent id returns None
        let fake_id = ElementId(9999);
        assert_eq!(t.element_text(fake_id), None);
    }

    #[test]
    fn element_display_can_be_set_and_updated() {
        let mut t = TextArea::new();
        let display = Line::from("[Pasted 5 lines]");
        let id = t.insert_element("lots of raw text here", ElementKind(1), Some(display));

        // Verify display is set
        let elem = &t.elements()[0];
        assert!(elem.display.is_some());
        assert_eq!(
            elem.display.as_ref().unwrap().to_string(),
            "[Pasted 5 lines]"
        );

        // Update display
        let new_display = Line::from("[Pasted 5 lines, 200 chars]");
        t.set_element_display(id, Some(new_display));
        let elem = &t.elements()[0];
        assert_eq!(
            elem.display.as_ref().unwrap().to_string(),
            "[Pasted 5 lines, 200 chars]"
        );

        // Clear display
        t.set_element_display(id, None);
        assert!(t.elements()[0].display.is_none());

        // Buffer text is unchanged
        assert_eq!(t.element_text(id), Some("lots of raw text here"));
    }

    #[test]
    fn insert_element_returns_id_for_metadata_tracking() {
        let mut t = TextArea::new();
        let mut metadata: std::collections::HashMap<ElementId, String> =
            std::collections::HashMap::new();

        let id1 = t.insert_element("paste1", ElementKind(1), None);
        metadata.insert(id1, "First paste".to_string());

        let id2 = t.insert_element("paste2", ElementKind(1), None);
        metadata.insert(id2, "Second paste".to_string());

        // Verify we can look up metadata by id
        assert_eq!(metadata.get(&id1), Some(&"First paste".to_string()));
        assert_eq!(metadata.get(&id2), Some(&"Second paste".to_string()));

        // Delete first element
        t.set_cursor(0);
        t.delete_forward(1);

        // id2 still valid in our metadata map
        let remaining = &t.elements()[0];
        assert_eq!(remaining.id, id2);
        assert_eq!(
            metadata.get(&remaining.id),
            Some(&"Second paste".to_string())
        );
    }
