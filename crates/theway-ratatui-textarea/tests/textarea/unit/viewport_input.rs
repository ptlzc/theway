    #[test]
    fn cursor_pos_accounts_for_scrollbar_width() {
        // When scrollbar is shown, cursor position should use content width,
        // not full area width.
        let mut ta = TextArea::new();
        // Fill 18 chars + enough lines to overflow.
        ta.insert_str(&format!("{}\n2\n3\n4\n5\n6", "x".repeat(18)));
        let _ = ta.text.set_cursor_byte(0); // at start
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        let pos = ta.cursor_pos_with_state(area, state);
        // Cursor at pos 0 should be at (0, 0).
        assert_eq!(pos, Some((0, 0)));
    }

    #[test]
    fn click_on_scrollbar_thumb_does_not_jump() {
        // With 10 lines in a 5-row viewport, the thumb is near the top
        // when scroll is at 0.  Clicking on the thumb should NOT jump —
        // it should just start a drag from the current position.
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();

        // Scroll is at 0 — thumb should be at the top of the track.
        // Click on row 0 (top of track = on the thumb).
        let action = ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 19,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        assert_eq!(action, MouseAction::Scrolled);
        assert!(ta.scrollbar_dragging);
        // The scroll should NOT have changed — thumb click = no jump.
        assert!(
            ta.scroll_override.is_none() || ta.scroll_override == Some(0),
            "thumb click should not jump: {:?}",
            ta.scroll_override,
        );
    }

    #[test]
    fn click_on_scrollbar_track_jumps() {
        // Clicking on the track (outside the thumb) should jump.
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();

        // Scroll at 0, thumb near top.  Click at bottom of track (row 4)
        // which should be on the track, not the thumb.
        let action = ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 19,
                row: 4,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        assert_eq!(action, MouseAction::Scrolled);
        // Should have jumped to a non-zero scroll position.
        assert!(
            ta.scroll_override.unwrap_or(0) > 0,
            "track click should jump"
        );
    }

    // ── Clipboard provider tests ──

    #[test]
    fn default_clipboard_provider_round_trips() {
        let mut ta = TextArea::new();
        ta.insert_str("hello world");
        // Select all via set_selection and cut
        ta.set_selection(0, 5);
        ta.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        // take_clipboard returns the cut text
        assert_eq!(ta.take_clipboard(), Some("hello".to_string()));
        // Ctrl-V pastes it back
        ta.input(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(ta.text(), "hello world");
    }

    #[test]
    fn custom_clipboard_provider() {
        #[derive(Debug)]
        struct TestClip {
            stored: Option<String>,
        }
        impl ClipboardProvider for TestClip {
            fn get(&mut self) -> Option<String> {
                self.stored.clone()
            }
            fn set(&mut self, text: &str) {
                self.stored = Some(format!("CUSTOM:{text}"));
            }
        }

        let mut ta = TextArea::new();
        ta.set_clipboard_provider(Box::new(TestClip { stored: None }));
        ta.insert_str("abc");
        ta.set_selection(0, 3);
        // Ctrl-X should call provider.set with "abc"
        ta.input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        // Ctrl-V should paste from provider.get → "CUSTOM:abc"
        ta.input(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(ta.text(), "CUSTOM:abc");
    }

    #[test]
    fn ctrl_v_pastes_from_provider() {
        #[derive(Debug)]
        struct PreloadedClip;
        impl ClipboardProvider for PreloadedClip {
            fn get(&mut self) -> Option<String> {
                Some("pasted!".to_string())
            }
            fn set(&mut self, _text: &str) {}
        }

        let mut ta = TextArea::new();
        ta.set_clipboard_provider(Box::new(PreloadedClip));
        ta.input(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(ta.text(), "pasted!");
    }

    #[test]
    fn copy_on_selection_finalized_sets_provider() {
        // Drag-select → mouse up should call provider.set
        #[derive(Debug)]
        struct RecordingClip {
            last_set: Option<String>,
        }
        impl ClipboardProvider for RecordingClip {
            fn get(&mut self) -> Option<String> {
                self.last_set.clone()
            }
            fn set(&mut self, text: &str) {
                self.last_set = Some(text.to_string());
            }
        }

        let mut ta = TextArea::new();
        ta.set_clipboard_provider(Box::new(RecordingClip { last_set: None }));
        ta.insert_str("hello");
        ta.set_cursor(0);

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click at 0, drag to 5, release
        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(5, 0), area, state);
        ta.handle_mouse(mouse_up(5, 0), area, state);

        // Now Ctrl-V should paste "hello" (from provider)
        ta.set_cursor(5);
        ta.input(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert_eq!(ta.text(), "hellohello");
    }

    // ── Hover / element event tests ──

    fn mouse_moved(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn hover_enter_on_element() {
        let mut ta = TextArea::new();
        ta.insert_str("hi ");
        let id = ta.insert_element("elem", ElementKind(0), None);
        ta.insert_str(" bye");

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Move over plain text — no event
        ta.handle_mouse(mouse_moved(0, 0), area, state);
        assert!(ta.poll_element_event().is_none());

        // Move over element (col 3)
        ta.handle_mouse(mouse_moved(3, 0), area, state);
        let ev = ta.poll_element_event().expect("should emit HoverEnter");
        assert_eq!(ev.id, id);
        assert_eq!(ev.kind, TextElementEventKind::HoverEnter);
    }

    #[test]
    fn hover_leave_on_element() {
        let mut ta = TextArea::new();
        ta.insert_str("hi ");
        let id = ta.insert_element("elem", ElementKind(0), None);
        ta.insert_str(" bye");

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Enter the element
        ta.handle_mouse(mouse_moved(3, 0), area, state);
        ta.poll_element_event(); // consume

        // Leave the element
        ta.handle_mouse(mouse_moved(0, 0), area, state);
        let ev = ta.poll_element_event().expect("should emit HoverLeave");
        assert_eq!(ev.id, id);
        assert_eq!(ev.kind, TextElementEventKind::HoverLeave);
    }

    #[test]
    fn hover_stays_on_same_element_no_event() {
        let mut ta = TextArea::new();
        ta.insert_str("hi ");
        ta.insert_element("elem", ElementKind(0), None);
        ta.insert_str(" bye");

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Enter the element (col 3)
        ta.handle_mouse(mouse_moved(3, 0), area, state);
        ta.poll_element_event(); // consume enter

        // Move within the element (col 4) — no new event
        ta.handle_mouse(mouse_moved(4, 0), area, state);
        assert!(ta.poll_element_event().is_none());
    }

    #[test]
    fn hover_between_two_elements() {
        let mut ta = TextArea::new();
        let id1 = ta.insert_element("AA", ElementKind(0), None);
        ta.insert_str(" ");
        let id2 = ta.insert_element("BB", ElementKind(0), None);
        // Visual: A A   B B
        //         0 1 2 3 4

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Hover element 1
        ta.handle_mouse(mouse_moved(0, 0), area, state);
        let ev = ta.poll_element_event().unwrap();
        assert_eq!(ev.id, id1);
        assert_eq!(ev.kind, TextElementEventKind::HoverEnter);

        // Move to element 2 — should emit enter for id2
        // (HoverLeave for id1 gets overwritten by HoverEnter for id2)
        ta.handle_mouse(mouse_moved(3, 0), area, state);
        let ev = ta.poll_element_event().unwrap();
        assert_eq!(ev.id, id2);
        assert_eq!(ev.kind, TextElementEventKind::HoverEnter);
    }

    // ── set_scroll_override / scroll_override tests ────────────────────

    #[test]
    fn scroll_override_getter_setter() {
        let mut ta = TextArea::new();
        assert_eq!(ta.scroll_override(), None);
        ta.set_scroll_override(Some(5));
        assert_eq!(ta.scroll_override(), Some(5));
        ta.set_scroll_override(None);
        assert_eq!(ta.scroll_override(), None);
    }

    /// Helper: stateful render (saves typing the full trait path).
    fn render_stateful(ta: &TextArea, area: Rect, buf: &mut Buffer, state: &mut TextAreaState) {
        ratatui::widgets::StatefulWidgetRef::render_ref(&ta, area, buf, state);
    }

    #[test]
    fn scroll_override_forces_viewport_ignoring_cursor() {
        // With 20 lines, cursor at end, and viewport of 5 rows,
        // effective_scroll normally follows the cursor to the bottom.
        // set_scroll_override(Some(0)) should force viewport to top.
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        ta.set_cursor(ta.text().len()); // cursor at end
        let area = Rect::new(0, 0, 40, 5);
        let mut state = TextAreaState::default();
        let mut buf = Buffer::empty(area);

        // Render without override: cursor-follow scrolls to bottom.
        render_stateful(&ta, area, &mut buf, &mut state);
        assert!(state.scroll > 0, "should scroll to show cursor at end");
        let normal_scroll = state.scroll;

        // Set override to 0 and render: viewport at top despite cursor at end.
        ta.set_scroll_override(Some(0));
        render_stateful(&ta, area, &mut buf, &mut state);
        assert_eq!(state.scroll, 0, "override should force scroll to 0");

        // Clear override: cursor-follow resumes.
        ta.set_scroll_override(None);
        render_stateful(&ta, area, &mut buf, &mut state);
        assert_eq!(
            state.scroll, normal_scroll,
            "clearing override should resume cursor-follow"
        );
    }

    #[test]
    fn scroll_override_clamped_to_max() {
        // Override value larger than max_scroll should be clamped.
        let text = "line 0\nline 1\nline 2"; // 3 lines
        let mut ta = ta_with(text);
        ta.set_cursor(0);
        let area = Rect::new(0, 0, 40, 2); // 2 rows visible, max_scroll = 1
        let mut state = TextAreaState::default();
        let mut buf = Buffer::empty(area);

        ta.set_scroll_override(Some(999));
        render_stateful(&ta, area, &mut buf, &mut state);
        assert_eq!(state.scroll, 1, "override should be clamped to max_scroll");
    }

    #[test]
    fn scroll_override_survives_render_cycles() {
        // The override should persist across multiple renders (unlike
        // mousewheel override which clears on cursor movement).
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        ta.set_cursor(ta.text().len());
        let area = Rect::new(0, 0, 40, 5);
        let mut state = TextAreaState::default();
        let mut buf = Buffer::empty(area);

        ta.set_scroll_override(Some(3));
        for _ in 0..5 {
            render_stateful(&ta, area, &mut buf, &mut state);
            assert_eq!(state.scroll, 3, "override should persist across renders");
        }
    }

    #[test]
    fn scroll_override_save_restore_round_trip() {
        // Simulates the collapsed-prompt pattern:
        // 1. Render normally (cursor-follow)
        // 2. Save state.scroll + scroll_override
        // 3. Override to 0, render collapsed
        // 4. Restore both → next render shows original viewport
        let text = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        ta.set_cursor(ta.text().len()); // cursor at end
        let area = Rect::new(0, 0, 40, 5);
        let mut state = TextAreaState::default();
        let mut buf = Buffer::empty(area);

        // 1. Initial render — cursor-follow scrolls to bottom.
        render_stateful(&ta, area, &mut buf, &mut state);
        let original_scroll = state.scroll;
        let original_override = ta.scroll_override();
        assert!(original_scroll > 0);
        assert_eq!(original_override, None);

        // 2. "Collapse": override to 0, render a few frames.
        ta.set_scroll_override(Some(0));
        for _ in 0..3 {
            render_stateful(&ta, area, &mut buf, &mut state);
            assert_eq!(state.scroll, 0);
        }

        // 3. Restore both.
        ta.set_scroll_override(original_override);
        state.scroll = original_scroll;

        // 4. Render "uncollapsed" — should show original position.
        render_stateful(&ta, area, &mut buf, &mut state);
        assert_eq!(
            state.scroll, original_scroll,
            "restored scroll should match original"
        );
    }

    #[test]
    fn scroll_override_save_restore_with_mousewheel() {
        // Same as above but the user had mousewheel-scrolled away from cursor
        // before collapse. Both state.scroll and scroll_override must be
        // saved/restored for the viewport to return to its pre-collapse position.
        let text = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        ta.set_cursor(0); // cursor at start
        let area = Rect::new(0, 0, 40, 5);
        let mut state = TextAreaState::default();
        let mut buf = Buffer::empty(area);

        // Render at start, then mousewheel to scroll viewport away from cursor.
        render_stateful(&ta, area, &mut buf, &mut state);
        assert_eq!(state.scroll, 0);
        for _ in 0..5 {
            ta.handle_mouse(mouse_scroll_down(0, 0), area, state);
            render_stateful(&ta, area, &mut buf, &mut state);
        }
        let mousewheel_scroll = state.scroll;
        let mousewheel_override = ta.scroll_override();
        assert!(mousewheel_scroll > 0, "should have scrolled away");
        assert!(mousewheel_override.is_some(), "mousewheel sets override");

        // "Collapse": save both, override to 0.
        let saved_scroll = state.scroll;
        let saved_override = ta.scroll_override();
        ta.set_scroll_override(Some(0));
        for _ in 0..3 {
            render_stateful(&ta, area, &mut buf, &mut state);
            assert_eq!(state.scroll, 0);
        }

        // Restore both.
        ta.set_scroll_override(saved_override);
        state.scroll = saved_scroll;

        // Render "uncollapsed" — viewport should be at the mousewheel position,
        // NOT snapped to cursor (which is at line 0).
        render_stateful(&ta, area, &mut buf, &mut state);
        assert_eq!(
            state.scroll, mousewheel_scroll,
            "viewport should restore to mousewheel position, not snap to cursor"
        );
    }

    #[test]
    fn shifted_character_classification_only_uppercases_letters() {
        for (input, expected) in [
            ('a', 'A'),
            ('z', 'Z'),
            ('A', 'A'),
            ('7', '7'),
            ('/', '/'),
            (';', ';'),
        ] {
            assert_eq!(
                classify_key_event(&KeyEvent::new(KeyCode::Char(input), KeyModifiers::SHIFT)),
                Some(EditCommand::Insert(expected))
            );
        }
    }

    #[test]
    fn modified_delete_and_arrow_keys_keep_word_semantics() {
        for modifiers in [
            KeyModifiers::ALT,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT | KeyModifiers::CONTROL,
        ] {
            assert_eq!(
                classify_key_event(&KeyEvent::new(KeyCode::Delete, modifiers)),
                Some(EditCommand::DeleteWordForward(WordStyle::Small)),
            );
            assert_eq!(
                classify_key_event(&KeyEvent::new(KeyCode::Left, modifiers)),
                Some(EditCommand::MoveWordLeft(WordStyle::Small)),
            );
            assert_eq!(
                classify_key_event(&KeyEvent::new(KeyCode::Right, modifiers)),
                Some(EditCommand::MoveWordRight(WordStyle::Small)),
            );
        }
        for modifiers in [KeyModifiers::ALT, KeyModifiers::SUPER] {
            assert_eq!(
                classify_key_event(&KeyEvent::new(KeyCode::Char('d'), modifiers)),
                Some(EditCommand::DeleteWordForward(WordStyle::Small)),
            );
        }
    }

    #[test]
    fn modifier_keys_do_not_insert_text() {
        let mut t = ta_with("hello");
        let len = t.text().len();
        t.input(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(t.text().len(), len);
    }

    #[test]
    fn alt_word_nav_preserved() {
        let mut t = ta_with("hello world");
        t.set_cursor(t.text().len());
        let text = t.text().to_owned();
        t.input(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(t.text(), text);
        assert!(t.cursor() < text.len());

        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(t.text(), text);
        assert!(t.cursor() > 0);
    }

    #[test]
    fn ctrl_alt_h_deletes_word() {
        let mut t = ta_with("hello world");
        t.set_cursor(t.text().len());
        t.input(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        assert_eq!(t.text(), "hello ");
    }

    #[test]
    fn plain_and_shifted_chars_insert() {
        let mut t = TextArea::new();
        for c in ['a', 'z', '1', '/', '@', '{', '!', '~'] {
            t.input(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(t.text(), "az1/@{!~");

        let mut t = TextArea::new();
        t.input(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SHIFT));
        t.input(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SHIFT));
        assert_eq!(t.text(), "AZ");
    }

    #[test]
    fn altgr_char_insertion_platform_dependent() {
        let mut t = TextArea::new();
        t.input(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ));
        if cfg!(target_os = "windows") {
            assert_eq!(t.text(), "@");
        } else {
            assert_eq!(t.text(), "");
        }
    }

    #[test]
    fn shift_number_trusts_terminal_character() {
        // QWERTZ: terminal sends Char('/') + SHIFT for Shift+7.
        let mut t = TextArea::new();
        t.input(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::SHIFT));
        assert_eq!(t.text(), "/");
    }
