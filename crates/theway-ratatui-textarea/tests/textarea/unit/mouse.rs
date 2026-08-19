    #[test]
    fn inline_element_is_undoable() {
        let mut ta = TextArea::new();
        ta.insert_str("A ");
        let id = ta.insert_element("multi\nline", ElementKind(1), None);
        ta.insert_str(" B");
        // Buffer: "A multi\nline B", element at 2..12

        assert_eq!(ta.elements().len(), 1);

        ta.inline_element(id);
        assert!(ta.elements().is_empty());
        assert_eq!(ta.text(), "A multi\nline B");

        // Undo should restore the element.
        assert!(ta.undo());
        assert_eq!(ta.elements().len(), 1);
        assert_eq!(ta.element_text(id), Some("multi\nline"));
    }

    #[test]
    fn inline_nonexistent_element_returns_false() {
        let mut ta = ta_with("hello");
        let fake_id = ElementId(9999);
        assert!(!ta.inline_element(fake_id));
    }

    #[test]
    fn inline_element_cursor_at_element_start() {
        let mut ta = TextArea::new();
        let id = ta.insert_element("elem", ElementKind(0), None);
        ta.insert_str(" tail");
        // Cursor is after " tail" → at end.
        // Move cursor to element start.
        ta.set_cursor(0);

        ta.inline_element(id);
        // Element removed, text unchanged.
        assert!(ta.elements().is_empty());
        // Cursor at end of inlined region.
        assert_eq!(ta.cursor(), 4);
    }

    // ── Click-on-element edge cases ──

    #[test]
    fn click_on_element_second_half_snaps_to_start() {
        // Element with a wide display: clicking on the right half should still
        // snap cursor to element start and emit a Click element event.
        let mut ta = TextArea::new();
        ta.insert_str("ab");
        // Element display is "ELEM" (4 chars). Buffer text is "xy".
        let display = Line::from("ELEM");
        let id = ta.insert_element("xy", ElementKind(0), Some(display));
        ta.insert_str("cd");
        // Buffer: "abxycd", element at 2..4, display "ELEM" (4 wide)
        // Visual: a b E L E M c d
        //         0 1 2 3 4 5 6 7

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();
        ta.set_cursor(0);

        // Click on col 5 → second half of "ELEM" display.
        // display_col_to_buffer_pos should return elem_end=4 (closer to end).
        // handle_mouse should detect this as on-element and snap to start.
        let action = ta.handle_mouse(mouse_down(5, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        let ev = ta.poll_element_event().expect("should emit element click");
        assert_eq!(ev.id, id);
        assert_eq!(ev.kind, TextElementEventKind::Click);
        assert_eq!(ta.cursor(), 2); // element start
    }

    #[test]
    fn click_on_element_first_half_snaps_to_start() {
        let mut ta = TextArea::new();
        ta.insert_str("ab");
        let display = Line::from("ELEM");
        let id = ta.insert_element("xy", ElementKind(0), Some(display));
        ta.insert_str("cd");
        ta.set_cursor(0);

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click on col 2 → first half of "ELEM" display.
        let action = ta.handle_mouse(mouse_down(2, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        let ev = ta.poll_element_event().expect("should emit element click");
        assert_eq!(ev.id, id);
        assert_eq!(ev.kind, TextElementEventKind::Click);
        assert_eq!(ta.cursor(), 2);
    }

    #[test]
    fn click_after_element_places_cursor_not_element() {
        let mut ta = TextArea::new();
        ta.insert_str("ab");
        let display = Line::from("EL");
        ta.insert_element("xy", ElementKind(0), Some(display));
        ta.insert_str("cd");
        ta.set_cursor(0);
        // Visual: a b E L c d
        //         0 1 2 3 4 5

        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        // Click on col 4 → 'c' (after element).
        let action = ta.handle_mouse(mouse_down(4, 0), area, state);
        assert_eq!(action, MouseAction::CursorPlaced);
        assert_eq!(ta.cursor(), 4); // byte 4 = 'c'
    }

    // ── Mouse wheel tests ──

    fn mouse_scroll_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_scroll_up(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn scroll_down_returns_scrolled() {
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee");
        ta.set_cursor(0);
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState::default();

        let action = ta.handle_mouse(mouse_scroll_down(5, 1), area, state);
        assert_eq!(action, MouseAction::Scrolled);
    }

    #[test]
    fn scroll_up_returns_scrolled() {
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee");
        ta.set_cursor(ta.text().len());
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState { scroll: 2 };

        let action = ta.handle_mouse(mouse_scroll_up(5, 1), area, state);
        assert_eq!(action, MouseAction::Scrolled);
    }

    #[test]
    fn scroll_down_when_content_fits_returns_nothing() {
        let mut ta = ta_with("short");
        let area = Rect::new(0, 0, 40, 5);
        let state = TextAreaState::default();

        let action = ta.handle_mouse(mouse_scroll_down(5, 1), area, state);
        assert_eq!(action, MouseAction::Nothing);
    }

    #[test]
    fn mousewheel_scrolls_viewport_not_cursor() {
        // Mousewheel should scroll the viewport without moving the cursor.
        // The cursor should stay at its current buffer position.
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        let area = Rect::new(0, 0, 40, 5); // only 5 lines visible
        let state = TextAreaState::default();

        // Place cursor on "line 2"
        let line2_start = text.find("line 2").unwrap();
        ta.set_cursor(line2_start);
        let cursor_before = ta.cursor();

        // Scroll down
        ta.handle_mouse(mouse_scroll_down(0, 0), area, state);

        // Cursor must NOT have moved
        assert_eq!(
            ta.cursor(),
            cursor_before,
            "mousewheel should not move cursor"
        );
    }

    #[test]
    fn click_after_scroll_places_cursor_at_clicked_line() {
        // After scrolling the viewport away from the cursor via mousewheel,
        // clicking on a visible line should place the cursor on THAT line —
        // not jump to some other position based on the old cursor location.
        let text = (0..40)
            .map(|i| format!("line {:02}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        // height=20 → scroll_lines_for_height returns 3 lines/tick
        let area = Rect::new(0, 0, 40, 20);
        let mut state = TextAreaState::default();

        // Cursor starts at line 0.
        ta.set_cursor(0);

        // Render to initialize state.scroll.
        let mut buf = Buffer::empty(area);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);

        // Scroll down 3 ticks (3 lines × 3 = 9 lines).
        for _ in 0..3 {
            ta.handle_mouse(mouse_scroll_down(0, 0), area, state);
            ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);
        }

        // Viewport should now start around line 9.
        assert!(
            state.scroll >= 9,
            "viewport should have scrolled; scroll={}",
            state.scroll
        );

        // Click on visual row 0 (which is now "line 09" or similar).
        ta.handle_mouse(mouse_down(0, 0), area, state);

        // The cursor should now be on a line that was VISIBLE.
        let cursor = ta.cursor();
        let lines = ta.wrapped_lines(area.width);
        let cursor_line = TextArea::wrapped_line_index_by_start(&lines, cursor).unwrap();
        assert!(
            cursor_line >= state.scroll as usize
                && cursor_line < (state.scroll + area.height) as usize,
            "click on visible row 0 should place cursor on a visible line; \
             cursor_line={cursor_line}, scroll={}, visible=[{}..{})",
            state.scroll,
            state.scroll,
            state.scroll as usize + area.height as usize,
        );
    }

    #[test]
    fn drag_select_after_scroll_selects_visible_text() {
        // After scrolling, drag-selecting should work on the visible text,
        // not jump the viewport back.
        let text = (0..20)
            .map(|i| format!("line {:02}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        let area = Rect::new(0, 0, 40, 5);
        let mut state = TextAreaState::default();

        ta.set_cursor(0);

        // Render + scroll down.
        let mut buf = Buffer::empty(area);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);
        for _ in 0..3 {
            ta.handle_mouse(mouse_scroll_down(0, 0), area, state);
            ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);
        }
        let scroll_after = state.scroll;

        // Click-down on row 1, then drag to row 3.
        ta.handle_mouse(mouse_down(0, 1), area, state);
        // Re-render so state.scroll updates after the click.
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);
        ta.handle_mouse(mouse_drag(5, 3), area, state);

        // Selection should exist.
        let sel = ta.selection_range().expect("drag should create selection");

        // The selected region should be within the visible range, not at line 0.
        let lines = ta.wrapped_lines(area.width);
        let sel_start_line = TextArea::wrapped_line_index_by_start(&lines, sel.start).unwrap();
        assert!(
            sel_start_line >= scroll_after as usize,
            "selection start should be in scrolled region; \
             sel_start_line={sel_start_line}, scroll={scroll_after}"
        );
    }

    #[test]
    fn drag_outside_after_mousewheel_still_scrolls() {
        // After mousewheel scrolling during a drag, dragging outside the
        // textarea area should continue to auto-scroll the viewport.
        let text = (0..30)
            .map(|i| format!("line {:02}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let mut ta = ta_with(&text);
        let area = Rect::new(0, 0, 40, 5);
        let mut state = TextAreaState::default();

        ta.set_cursor(0);

        // Render to initialize state.
        let mut buf = Buffer::empty(area);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);

        // Start drag at row 0.
        ta.handle_mouse(mouse_down(0, 0), area, state);
        ta.handle_mouse(mouse_drag(0, 1), area, state);
        assert!(ta.selection_range().is_some());

        // Mousewheel scroll down during drag.
        ta.handle_mouse(mouse_scroll_down(0, 0), area, state);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);
        let scroll_after_wheel = state.scroll;

        // Now drag below the area (row = area.y + area.height = 5).
        // This should auto-scroll the viewport further down.
        // We need to bypass throttle, so reset the timer.
        ta.last_drag_scroll = None;
        ta.drag_scroll_steps = 0;
        ta.handle_mouse(mouse_drag(0, area.y + area.height), area, state);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&ta), area, &mut buf, &mut state);

        assert!(
            state.scroll > scroll_after_wheel,
            "drag-below after mousewheel should continue scrolling; \
             scroll={}, expected > {scroll_after_wheel}",
            state.scroll
        );
    }

    #[test]
    fn scroll_during_drag_preserves_selection_anchor() {
        // Start drag at "bbb", scroll down — selection should extend,
        // anchor stays at original position.
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee\nfff\nggg");
        ta.set_cursor(0); // put cursor at start so scroll=0 is consistent
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState::default();

        // Click-down on "bbb" (row 1, col 1 → byte 5 = second 'b')
        ta.handle_mouse(mouse_down(1, 1), area, state);
        let anchor = ta.cursor();
        assert_eq!(
            anchor, 5,
            "click on bbb col 1 should place cursor at byte 5"
        );

        // Start drag → creates selection
        ta.handle_mouse(mouse_drag(2, 1), area, state);
        assert!(
            ta.selection_range().is_some(),
            "drag should create selection"
        );

        // Now scroll down while dragging
        let action = ta.handle_mouse(mouse_scroll_down(1, 1), area, state);
        assert_eq!(action, MouseAction::Scrolled);

        // Selection should still exist and anchor should not have moved
        let sel = ta
            .selection_range()
            .expect("selection should survive scroll");
        assert!(
            sel.contains(&anchor),
            "anchor byte {anchor} should still be inside selection {sel:?}"
        );
    }

    #[test]
    fn scroll_during_drag_extends_selection_head() {
        // Scroll down during drag should move the selection head forward.
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee\nfff\nggg");
        ta.set_cursor(0);
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState::default();

        // Click-down on "aaa" (row 0, col 1)
        ta.handle_mouse(mouse_down(1, 0), area, state);
        // Start drag
        ta.handle_mouse(mouse_drag(2, 0), area, state);
        let sel_before = ta.selection_range().unwrap();

        // Scroll down
        ta.handle_mouse(mouse_scroll_down(1, 1), area, state);
        let sel_after = ta.selection_range().unwrap();

        // Selection should have grown (head moved forward)
        assert!(
            sel_after.end > sel_before.end,
            "scroll-down during drag should extend selection: before={sel_before:?} after={sel_after:?}"
        );
        // Anchor should not have moved
        assert_eq!(sel_after.start, sel_before.start);
    }

    #[test]
    fn down_during_active_drag_does_not_reset_anchor() {
        // Some terminals re-emit Down(Left) after a scroll event even though
        // the button was held the whole time.  When `drag_active` is true,
        // a Down should be treated as a drag continuation, not a new click.
        let mut ta = ta_with("aaa\nbbb\nccc\nddd\neee\nfff\nggg");
        ta.set_cursor(0);
        let area = Rect::new(0, 0, 40, 3);
        let state = TextAreaState::default();

        // Click on "aaa" (row 0, col 1), then drag to start selection.
        ta.handle_mouse(mouse_down(1, 0), area, state);
        ta.handle_mouse(mouse_drag(2, 0), area, state);
        let anchor_before = ta.selection_range().unwrap().start;

        // Scroll down while dragging.
        ta.handle_mouse(mouse_scroll_down(1, 1), area, state);
        assert!(
            ta.selection_range().is_some(),
            "selection must survive scroll"
        );

        // Simulate terminal re-emitting Down(Left) at a different row.
        ta.handle_mouse(mouse_down(1, 2), area, state);

        // Selection should still exist and anchor should NOT have moved.
        let sel = ta
            .selection_range()
            .expect("Down during drag must not kill selection");
        assert_eq!(
            sel.start, anchor_before,
            "anchor must not reset: expected {anchor_before}, got {}",
            sel.start
        );
    }

    // ── Drag-scroll acceleration / distance helpers ──

    #[test]
    fn drag_scroll_interval_ramps_up() {
        // Step 0 → 80ms, step 1 → 60ms, step 2+ → 40ms.
        assert_eq!(TextArea::drag_scroll_interval(0), 80);
        assert_eq!(TextArea::drag_scroll_interval(1), 60);
        assert_eq!(TextArea::drag_scroll_interval(2), 40);
        assert_eq!(TextArea::drag_scroll_interval(100), 40);
    }

    #[test]
    fn drag_scroll_lines_for_distance_tiers() {
        // Close: 1 line, farther: more lines.
        assert_eq!(TextArea::drag_scroll_lines_for_distance(1), 1);
        assert_eq!(TextArea::drag_scroll_lines_for_distance(2), 1);
        assert_eq!(TextArea::drag_scroll_lines_for_distance(3), 2);
        assert_eq!(TextArea::drag_scroll_lines_for_distance(5), 2);
        assert_eq!(TextArea::drag_scroll_lines_for_distance(6), 3);
        assert_eq!(TextArea::drag_scroll_lines_for_distance(10), 3);
        assert_eq!(TextArea::drag_scroll_lines_for_distance(20), 5);
    }

    // ── Scrollbar tests ──

    #[test]
    fn scrollbar_not_shown_when_content_fits() {
        // 3 lines of text in a 5-row viewport → no scrollbar needed.
        let mut ta = TextArea::new();
        ta.insert_str("aaa\nbbb\nccc");
        let area = Rect::new(0, 0, 20, 5);
        let (cw, needs) = ta.content_width(area.width, area.height);
        assert!(!needs, "should not need scrollbar when content fits");
        assert_eq!(cw, 20, "full width when no scrollbar");
    }

    #[test]
    fn scrollbar_shown_when_content_overflows() {
        // 10 lines of text in a 5-row viewport → scrollbar needed.
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let (cw, needs) = ta.content_width(area.width, area.height);
        assert!(needs, "should need scrollbar when content overflows");
        assert_eq!(cw, 19, "width reduced by 1 for scrollbar");
    }

    #[test]
    fn scrollbar_respects_show_scrollbar_false() {
        let mut ta = TextArea::new();
        ta.show_scrollbar = false;
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let (cw, needs) = ta.content_width(area.width, area.height);
        assert!(!needs, "scrollbar disabled");
        assert_eq!(cw, 20, "full width when scrollbar disabled");
    }

    #[test]
    fn scrollbar_wrapping_uses_narrower_width() {
        // A line that fits in 20 cols but not in 19 should wrap differently
        // when scrollbar is present.
        let mut ta = TextArea::new();
        // 19 'a's → fits in 19 cols (no wrap).
        // Then enough other lines to overflow the viewport.
        ta.insert_str(&format!("{}\n2\n3\n4\n5\n6", "a".repeat(19)));
        let area = Rect::new(0, 0, 20, 5);
        let (cw, needs) = ta.content_width(area.width, area.height);
        assert!(needs, "overflows");
        assert_eq!(cw, 19);
        // The 19-char line should NOT wrap at width 19 — it fits exactly.
        let lines = ta.wrapped_lines(cw);
        // First wrapped line should contain all 19 chars.
        assert_eq!(&ta.text()[lines[0].clone()], &"a".repeat(19));
    }

    #[test]
    fn click_on_scrollbar_column_scrolls() {
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        // Move cursor to start so we can verify it doesn't move.
        let _ = ta.text.set_cursor_byte(0);
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click on the scrollbar column (rightmost column = 19),
        // at the bottom row of the viewport → should scroll to end.
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
        // Cursor should not have moved — scrollbar click doesn't place cursor.
        assert_eq!(ta.cursor(), 0);
        // scroll_override should be set.
        assert!(ta.scroll_override.is_some());
    }

    #[test]
    fn click_on_scrollbar_top_scrolls_to_top() {
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Scroll to the bottom first so the top of the track is NOT the thumb.
        ta.scroll_override = Some(5);
        // Click at top of scrollbar (row 0) — should be track, jump to top.
        ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 19,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        assert_eq!(ta.scroll_override, Some(0));
    }

    #[test]
    fn click_on_text_area_does_not_trigger_scrollbar() {
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Click on column 18 (text area, not scrollbar column 19).
        let action = ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 18,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        // Should place cursor, not scroll.
        assert_eq!(action, MouseAction::CursorPlaced);
        assert!(!ta.scrollbar_dragging);
    }

    #[test]
    fn drag_on_scrollbar_scrolls_proportionally() {
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let state = TextAreaState::default();
        // Scroll to bottom so the thumb is at the bottom, then click
        // on the track at row 0 to start a track-based drag.
        ta.scroll_override = Some(5);
        ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 19,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        assert!(ta.scrollbar_dragging);
        let scroll_at_top = ta.scroll_override.unwrap();
        assert_eq!(scroll_at_top, 0, "track click at top should jump to 0");
        // Drag to middle of scrollbar track.
        ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 19,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        let scroll_at_mid = ta.scroll_override.unwrap();
        assert!(
            scroll_at_mid > scroll_at_top,
            "dragging down should scroll further"
        );
        // Drag to bottom.
        ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 19,
                row: 4,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        let scroll_at_bottom = ta.scroll_override.unwrap();
        assert!(
            scroll_at_bottom > scroll_at_mid,
            "dragging to bottom should scroll to max"
        );
        // Mouse up ends drag.
        ta.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 19,
                row: 4,
                modifiers: KeyModifiers::NONE,
            },
            area,
            state,
        );
        assert!(!ta.scrollbar_dragging);
    }

    #[test]
    fn scrollbar_render_produces_track_and_thumb() {
        // Render a textarea with overflow and verify the scrollbar column
        // has non-default styled cells.
        let mut ta = TextArea::new();
        ta.insert_str("1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let mut state = TextAreaState::default();
        StatefulWidgetRef::render_ref(&&ta, area, &mut buf, &mut state);

        let sb_col = 19u16;
        // All cells in the scrollbar column should have the track bg color.
        let mut has_thumb = false;
        for row in 0..5u16 {
            let cell = &buf[(sb_col, row)];
            // Track bg is Rgb(45,45,55); check bg is set.
            assert!(cell.style().bg.is_some(), "scrollbar cell should have bg");
            if cell.symbol() != " " {
                has_thumb = true;
            }
        }
        assert!(has_thumb, "should have at least one thumb cell");
    }

    #[test]
    fn no_scrollbar_column_when_content_fits() {
        // When content fits, the rightmost column should not have
        // scrollbar styling.
        let mut ta = TextArea::new();
        ta.insert_str("hello");
        let area = Rect::new(0, 0, 20, 5);
        let mut buf = Buffer::empty(area);
        let mut state = TextAreaState::default();
        StatefulWidgetRef::render_ref(&&ta, area, &mut buf, &mut state);

        let last_col = 19u16;
        let cell = &buf[(last_col, 0u16)];
        // Should be default (empty space), not scrollbar styled.
        assert!(
            cell.style().bg.is_none() || !matches!(cell.style().bg, Some(Color::Rgb(32, 35, 53))),
            "should not have scrollbar bg when content fits"
        );
    }
