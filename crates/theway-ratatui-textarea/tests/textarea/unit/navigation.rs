    #[test]
    fn home_end_use_logical_line_when_soft_wrapped() {
        // width 4 → "abcd" | "efgh" | "ij"
        let mut t = ta_with("abcdefghij");
        let _ = t.desired_height(4);
        t.set_cursor(6); // mid second visual row

        t.input(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(t.cursor(), 0);
        t.input(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(t.cursor(), t.text().len());

        // Super+Left/Right stay on the visual wrap row.
        t.set_cursor(6);
        t.move_cursor_to_beginning_of_line(false);
        assert_eq!(t.cursor(), 4);
        t.move_cursor_to_end_of_line(false);
        assert_eq!(t.cursor(), 7);

        // Multiline: Home/End stay on this logical line, not wrap-row or buffer.
        let mut multi = ta_with("abcdefghij\nxyz");
        let _ = multi.desired_height(4);
        multi.set_cursor(6);
        multi.input(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(multi.cursor(), 0);
        multi.set_cursor(6);
        multi.input(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(multi.cursor(), "abcdefghij".len());
        multi.set_cursor("abcdefghij\nxy".len());
        multi.input(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(multi.cursor(), "abcdefghij\n".len());
        multi.input(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(multi.cursor(), multi.text().len());

        // Ctrl+A/E stay logical (and chain across lines).
        t.set_cursor(6);
        t.move_cursor_to_beginning_of_line(true);
        assert_eq!(t.cursor(), 0);
        t.set_cursor(6);
        t.move_cursor_to_end_of_line(true);
        assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn end_of_line_or_down_at_end_of_text() {
        let mut t = ta_with("one\ntwo");
        // Place cursor at absolute end of the text
        t.set_cursor(t.text().len());
        // Should remain at end without panicking
        t.move_cursor_to_end_of_line(true);
        assert_eq!(t.cursor(), t.text().len());

        // Also verify behavior when at EOL of a non-final line:
        let eol_first_line = 3; // index of '\n' in "one\ntwo"
        t.set_cursor(eol_first_line);
        t.move_cursor_to_end_of_line(true);
        assert_eq!(t.cursor(), t.text().len()); // moves to end of next (last) line
    }

    #[test]
    fn word_navigation_helpers() {
        let t = ta_with("  alpha  beta   gamma");
        let mut t = t; // make mutable for set_cursor
        // Put cursor after "alpha"
        let after_alpha = t.text().find("alpha").unwrap() + "alpha".len();
        t.set_cursor(after_alpha);
        assert_eq!(t.beginning_of_previous_word(), 2); // skip initial spaces

        // Put cursor at start of beta
        let beta_start = t.text().find("beta").unwrap();
        t.set_cursor(beta_start);
        assert_eq!(t.end_of_next_word(), beta_start + "beta".len());

        // If at end, end_of_next_word returns len
        t.set_cursor(t.text().len());
        assert_eq!(t.end_of_next_word(), t.text().len());
    }

    #[test]
    fn word_navigation_splits_on_hyphen() {
        let mut t = ta_with("hello-world");
        let hyphen = t.text().find('-').unwrap();
        let after_hyphen = hyphen + 1;

        t.set_cursor(t.text().len());
        assert_eq!(t.beginning_of_previous_word(), after_hyphen);

        t.set_cursor(after_hyphen);
        assert_eq!(t.beginning_of_previous_word(), hyphen);

        t.set_cursor(hyphen);
        assert_eq!(t.beginning_of_previous_word(), 0);

        t.set_cursor(0);
        assert_eq!(t.end_of_next_word(), hyphen);

        t.set_cursor(hyphen);
        assert_eq!(t.end_of_next_word(), after_hyphen);

        t.set_cursor(after_hyphen);
        assert_eq!(t.end_of_next_word(), t.text().len());
    }

    #[test]
    fn alt_arrow_navigation_splits_on_hyphen() {
        let mut t = ta_with("hello-world");
        let hyphen = t.text().find('-').unwrap();
        let after_hyphen = hyphen + 1;
        let end = t.text().len();

        t.set_cursor(0);
        t.input(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(t.cursor(), hyphen);

        t.input(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(t.cursor(), after_hyphen);

        t.input(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(t.cursor(), end);

        t.input(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(t.cursor(), after_hyphen);

        t.input(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(t.cursor(), hyphen);

        t.input(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(t.cursor(), 0);
    }

    #[test]
    fn cursor_at_wrap_boundary_shows_on_next_line() {
        // When typing fills an entire line, the cursor sits at the exact wrap
        // boundary.  It should be reported on the *next* visual line at col 0,
        // not at col == width (which is the invisible right border).

        // Case 1: text exactly fills one line — cursor at text.len()
        let mut t = ta_with("abcde");
        let area = Rect::new(0, 0, 5, 3); // width 5
        t.set_cursor(5); // cursor right after 'e'

        let (x, y) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 0, "cursor x should be 0 (start of virtual next line)");
        assert_eq!(y, 1, "cursor y should be 1 (next line)");

        // Case 2: text wraps — cursor at the boundary between two wrapped lines
        let mut t = ta_with("abcdefgh");
        let area = Rect::new(0, 0, 5, 3); // width 5, wraps after 'e'
        // cursor at position 5 = start of "fgh" = should be col 0, row 1
        t.set_cursor(5);

        let (x, y) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 0, "cursor at wrap point should be col 0 of next line");
        assert_eq!(y, 1, "cursor at wrap point should be on second visual line");
    }

    #[test]
    fn wrapping_and_cursor_positions() {
        let mut t = ta_with("hello world here");
        let area = Rect::new(0, 0, 6, 10); // width 6 -> wraps words
        // desired height counts wrapped lines
        assert!(t.desired_height(area.width) >= 3);

        // Place cursor in "world"
        let world_start = t.text().find("world").unwrap();
        t.set_cursor(world_start + 3);
        let (_x, y) = t.cursor_pos(area).unwrap();
        assert_eq!(y, 1); // world should be on second wrapped line

        // With state and small height, cursor is mapped onto visible row
        let mut state = TextAreaState::default();
        let small_area = Rect::new(0, 0, 6, 1);
        // First call: cursor not visible -> effective scroll ensures it is
        let (_x, y) = t.cursor_pos_with_state(small_area, state).unwrap();
        assert_eq!(y, 0);

        // Render with state to update actual scroll value
        let mut buf = Buffer::empty(small_area);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), small_area, &mut buf, &mut state);
        // After render, state.scroll should be adjusted so cursor row fits
        let effective_lines = t.desired_height(small_area.width);
        assert!(state.scroll < effective_lines);
    }

    #[test]
    fn cursor_pos_with_state_basic_and_scroll_behaviors() {
        // Case 1: No wrapping needed, height fits — scroll ignored, y maps directly.
        let mut t = ta_with("hello world");
        t.set_cursor(3);
        let area = Rect::new(2, 5, 20, 3);
        // Even if an absurd scroll is provided, when content fits the area the
        // effective scroll is 0 and the cursor position matches cursor_pos.
        let bad_state = TextAreaState { scroll: 999 };
        let (x1, y1) = t.cursor_pos(area).unwrap();
        let (x2, y2) = t.cursor_pos_with_state(area, bad_state).unwrap();
        assert_eq!((x2, y2), (x1, y1));

        // Case 2: Cursor below the current window — y should be clamped to the
        // bottom row (area.height - 1) after adjusting effective scroll.
        let mut t = ta_with("one two three four five six");
        // Force wrapping to many visual lines.
        let wrap_width = 4;
        let _ = t.desired_height(wrap_width);
        // Put cursor somewhere near the end so it's definitely below the first window.
        t.set_cursor(t.text().len().saturating_sub(2));
        let small_area = Rect::new(0, 0, wrap_width, 2);
        let state = TextAreaState { scroll: 0 };
        let (_x, y) = t.cursor_pos_with_state(small_area, state).unwrap();
        assert_eq!(y, small_area.y + small_area.height - 1);

        // Case 3: Cursor above the current window — y should be top row (0)
        // when the provided scroll is too large.
        let mut t = ta_with("alpha beta gamma delta epsilon zeta");
        let wrap_width = 5;
        let lines = t.desired_height(wrap_width);
        // Place cursor near start so an excessive scroll moves it to top row.
        t.set_cursor(1);
        let area = Rect::new(0, 0, wrap_width, 3);
        let state = TextAreaState {
            scroll: lines.saturating_mul(2),
        };
        let (_x, y) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!(y, area.y);
    }

    #[test]
    fn screen_spans_of_range_single_row() {
        let t = ta_with("xy /model tail");
        let area = Rect::new(2, 1, 40, 3);
        let state = TextAreaState::default();

        // "/model" = bytes 3..9, all on the first visual row.
        let spans = t.screen_spans_of_range(3..9, area, state);
        assert_eq!(spans, vec![Rect::new(5, 1, 6, 1)]);

        // Degenerate ranges yield no spans.
        assert!(t.screen_spans_of_range(4..4, area, state).is_empty());
        assert!(t.screen_spans_of_range(4..999, area, state).is_empty());
    }

    #[test]
    fn screen_spans_of_range_rejects_non_char_boundaries() {
        // 'é' spans bytes 1..3; an endpoint inside it must yield no spans
        // (tolerated like the other invalid-range shapes, never a panic).
        let t = ta_with("héllo");
        let area = Rect::new(0, 0, 10, 2);
        let state = TextAreaState::default();

        assert!(t.screen_spans_of_range(2..5, area, state).is_empty());
        assert!(t.screen_spans_of_range(0..2, area, state).is_empty());
    }

    #[test]
    fn screen_spans_of_range_covers_wrapped_rows() {
        // A token wider than the wrap width must split at the line end and
        // report one span per visual row it lands on.
        let mut t = ta_with("aa /pr-workflow");
        t.show_scrollbar = false;
        let area = Rect::new(0, 0, 8, 4);
        let state = TextAreaState::default();

        // "/pr-workflow" = bytes 3..15, display width 12 > wrap width 8.
        let spans = t.screen_spans_of_range(3..15, area, state);
        assert!(
            spans.len() >= 2,
            "token must cover multiple rows: {spans:?}"
        );
        assert!(spans.iter().all(|r| r.height == 1));
        for pair in spans.windows(2) {
            assert_eq!(pair[1].y, pair[0].y + 1, "rows must be consecutive");
        }
        for r in &spans[1..] {
            assert_eq!(r.x, area.x, "continuation rows start at the left edge");
            assert!(r.right() <= area.x + area.width);
        }
        // Tokens contain no whitespace, so no cell is lost at wrap boundaries:
        // the summed span widths equal the token's display width.
        let total: u16 = spans.iter().map(|r| r.width).sum();
        assert_eq!(total, 12);
    }

    #[test]
    fn screen_spans_of_range_skips_offscreen_rows() {
        // Cursor at the end scrolls the viewport to the tail: the token's
        // first row is above the viewport, but its visible tail must still
        // be reported (screen_position_of on the start would return None).
        let mut t = ta_with("/pr-workflow abc");
        t.show_scrollbar = false;
        let area = Rect::new(0, 0, 8, 2);
        let state = TextAreaState::default();

        let spans = t.screen_spans_of_range(0..12, area, state);
        assert!(!spans.is_empty(), "visible token tail must be reported");
        for r in &spans {
            assert!((area.y..area.y + area.height).contains(&r.y));
            assert!(r.width > 0 && r.right() <= area.x + area.width);
        }
        let total: u16 = spans.iter().map(|r| r.width).sum();
        assert!(
            total < 12,
            "off-screen head must not be reported: {spans:?}"
        );
    }

    #[test]
    fn screen_spans_of_range_uses_display_width() {
        // 2-cell CJK chars: 日本語 (9 bytes, display width 6) at wrap width 4
        // renders as 日本 / 語.
        let mut t = ta_with("日本語");
        t.show_scrollbar = false;
        let area = Rect::new(1, 0, 4, 3);
        let state = TextAreaState::default();

        let spans = t.screen_spans_of_range(0..9, area, state);
        assert_eq!(spans, vec![Rect::new(1, 0, 4, 1), Rect::new(1, 1, 2, 1)]);
    }

    #[test]
    fn screen_spans_of_range_clamps_to_content_edge() {
        // Overflowing content puts the scrollbar up, so content is only
        // `tw = width - 1` columns. Row 0's byte range keeps its trailing
        // wrap spaces ("ab   " measures 5), but the reported span must stop
        // at the content edge (4), never reaching the scrollbar column.
        let mut t = ta_with("ab   cd ef gh");
        t.set_cursor(0);
        let area = Rect::new(0, 0, 5, 2);
        let state = TextAreaState::default();

        let spans = t.screen_spans_of_range(0..5, area, state);
        assert_eq!(spans, vec![Rect::new(0, 0, 4, 1)]);
    }

    #[test]
    fn wrapped_navigation_across_visual_lines() {
        let mut t = ta_with("abcdefghij");
        t.show_scrollbar = false;
        // Force wrapping at width 4: lines -> ["abcd", "efgh", "ij"]
        let _ = t.desired_height(4);

        // From the very start, moving down should go to the start of the next wrapped line (index 4)
        t.set_cursor(0);
        t.move_cursor_down();
        assert_eq!(t.cursor(), 4);

        // Cursor at boundary index 4 should be displayed at start of second wrapped line
        t.set_cursor(4);
        let area = Rect::new(0, 0, 4, 10);
        let (x, y) = t.cursor_pos(area).unwrap();
        assert_eq!((x, y), (0, 1));

        // With state and small height, cursor should be visible at row 0, col 0
        let small_area = Rect::new(0, 0, 4, 1);
        let state = TextAreaState::default();
        let (x, y) = t.cursor_pos_with_state(small_area, state).unwrap();
        assert_eq!((x, y), (0, 0));

        // Place cursor in the middle of the second wrapped line ("efgh"), at 'g'
        t.set_cursor(6);
        // Move up should go to same column on previous wrapped line -> index 2 ('c')
        t.move_cursor_up();
        assert_eq!(t.cursor(), 2);

        // Move down should return to same position on the next wrapped line -> back to index 6 ('g')
        t.move_cursor_down();
        assert_eq!(t.cursor(), 6);

        // Move down again should go to third wrapped line. Target col is 2, but the line has len 2 -> clamp to end
        t.move_cursor_down();
        assert_eq!(t.cursor(), t.text().len());
    }

    #[test]
    fn cursor_pos_with_state_after_movements() {
        let mut t = ta_with("abcdefghij");
        // Wrap width 4 -> visual lines: abcd | efgh | ij
        let _ = t.desired_height(4);
        let area = Rect::new(0, 0, 4, 2);
        let mut state = TextAreaState::default();
        let mut buf = Buffer::empty(area);

        // Start at beginning
        t.set_cursor(0);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        let (x, y) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!((x, y), (0, 0));

        // Move down to second visual line; should be at bottom row (row 1) within 2-line viewport
        t.move_cursor_down();
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        let (x, y) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!((x, y), (0, 1));

        // Move down to third visual line; viewport scrolls and keeps cursor on bottom row
        t.move_cursor_down();
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        let (x, y) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!((x, y), (0, 1));

        // Move up to second visual line; with current scroll, it appears on top row
        t.move_cursor_up();
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        let (x, y) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!((x, y), (0, 0));

        // Column preservation across moves: set to col 2 on first line, move down
        t.set_cursor(2);
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        let (x0, y0) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!((x0, y0), (2, 0));
        t.move_cursor_down();
        ratatui::widgets::StatefulWidgetRef::render_ref(&(&t), area, &mut buf, &mut state);
        let (x1, y1) = t.cursor_pos_with_state(area, state).unwrap();
        assert_eq!((x1, y1), (2, 1));
    }

    #[test]
    fn wrapped_navigation_with_newlines_and_spaces() {
        // Include spaces and an explicit newline to exercise boundaries
        let mut t = ta_with("word1  word2\nword3");
        // Width 6 will wrap "word1  " and then "word2" before the newline
        let _ = t.desired_height(6);

        // Put cursor on the second wrapped line before the newline, at column 1 of "word2"
        let start_word2 = t.text().find("word2").unwrap();
        t.set_cursor(start_word2 + 1);

        // Up should go to first wrapped line, column 1 -> index 1
        t.move_cursor_up();
        assert_eq!(t.cursor(), 1);

        // Down should return to the same visual column on "word2"
        t.move_cursor_down();
        assert_eq!(t.cursor(), start_word2 + 1);

        // Down again should cross the logical newline to the next visual line ("word3"), clamped to its length if needed
        t.move_cursor_down();
        let start_word3 = t.text().find("word3").unwrap();
        assert!(t.cursor() >= start_word3 && t.cursor() <= start_word3 + "word3".len());
    }

    #[test]
    fn wrapped_navigation_with_wide_graphemes() {
        // Four thumbs up, each of display width 2, with width 3 to force wrapping inside grapheme boundaries
        let mut t = ta_with("👍👍👍👍");
        let _ = t.desired_height(3);

        // Put cursor after the second emoji (which should be on first wrapped line)
        t.set_cursor("👍👍".len());

        // Move down should go to the start of the next wrapped line (same column preserved but clamped)
        t.move_cursor_down();
        // We expect to land somewhere within the third emoji or at the start of it
        let pos_after_down = t.cursor();
        assert!(pos_after_down >= "👍👍".len());

        // Moving up should take us back to the original position
        t.move_cursor_up();
        assert_eq!(t.cursor(), "👍👍".len());
    }

    #[test]
    fn wrapped_navigation_with_zwj_graphemes() {
        let grapheme = "👩\u{200D}💻";
        let mut t = ta_with(&format!("{grapheme}{grapheme}{grapheme}"));
        let _ = t.desired_height(4);

        t.set_cursor(grapheme.len() * 2);

        t.move_cursor_down();
        let pos_after_down = t.cursor();
        assert!(pos_after_down >= grapheme.len() * 2);

        t.move_cursor_up();
        assert_eq!(t.cursor(), grapheme.len() * 2);
    }

    #[test]
    fn element_aware_wrap_ranges_preserve_zwj_graphemes() {
        let grapheme = "👩\u{200D}💻";
        let mut t = TextArea::new();
        t.insert_str(&format!("{grapheme}{grapheme}"));
        t.insert_element("raw", ElementKind(0), Some(Line::from("[P]")));

        let ranges = {
            let lines = t.wrapped_lines(2);
            lines.iter().cloned().collect::<Vec<_>>()
        };

        assert_eq!(ranges.len(), 3);
        assert_eq!(&t.text()[ranges[0].clone()], grapheme);
        assert_eq!(&t.text()[ranges[1].clone()], grapheme);
    }

    #[test]
    fn fuzz_textarea_randomized() {
        // Deterministic seed for reproducibility
        // Seed the RNG based on the current day in Pacific Time (PST/PDT). This
        // keeps the fuzz test deterministic within a day while still varying
        // day-to-day to improve coverage.
        let pst_today_seed: u64 = (chrono::Utc::now() - chrono::Duration::hours(8))
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp() as u64;
        let mut rng = rand::rngs::StdRng::seed_from_u64(pst_today_seed);

        for _case in 0..500 {
            let mut ta = TextArea::new();
            let mut state = TextAreaState::default();
            // Track element payloads we insert. Payloads use characters '[' and ']' which
            // are not produced by rand_grapheme(), avoiding accidental collisions.
            let mut elem_texts: Vec<String> = Vec::new();
            let mut next_elem_id: usize = 0;
            // Start with a random base string
            let base_len = rng.random_range(0..30);
            let mut base = String::new();
            for _ in 0..base_len {
                base.push_str(&rand_grapheme(&mut rng));
            }
            ta.set_text(&base);
            // Choose a valid char boundary for initial cursor
            let mut boundaries: Vec<usize> = vec![0];
            boundaries.extend(ta.text().char_indices().map(|(i, _)| i).skip(1));
            boundaries.push(ta.text().len());
            let init = boundaries[rng.random_range(0..boundaries.len())];
            ta.set_cursor(init);

            let mut width: u16 = rng.random_range(1..=12);
            let mut height: u16 = rng.random_range(1..=4);

            for _step in 0..60 {
                // Mostly stable width/height, occasionally change
                if rng.random_bool(0.1) {
                    width = rng.random_range(1..=12);
                }
                if rng.random_bool(0.1) {
                    height = rng.random_range(1..=4);
                }

                // Pick an operation
                match rng.random_range(0..18) {
                    0 => {
                        // insert small random string at cursor
                        let len = rng.random_range(0..6);
                        let mut s = String::new();
                        for _ in 0..len {
                            s.push_str(&rand_grapheme(&mut rng));
                        }
                        ta.insert_str(&s);
                    }
                    1 => {
                        // Include mid-grapheme char boundaries so normalization stays exercised.
                        let mut b: Vec<usize> = vec![0];
                        b.extend(ta.text().char_indices().map(|(i, _)| i).skip(1));
                        b.push(ta.text().len());
                        let i1 = rng.random_range(0..b.len());
                        let i2 = rng.random_range(0..b.len());
                        let (start, end) = if b[i1] <= b[i2] {
                            (b[i1], b[i2])
                        } else {
                            (b[i2], b[i1])
                        };
                        let insert_len = rng.random_range(0..=4);
                        let mut s = String::new();
                        for _ in 0..insert_len {
                            s.push_str(&rand_grapheme(&mut rng));
                        }
                        let before = ta.text().len();
                        let atomic_ranges = ta.element_ranges();
                        let plan = ta
                            .text
                            .plan_replace_byte_range(start..end, &s, &atomic_ranges);
                        let normalized_len = plan.replaced_byte_range().len();
                        ta.replace_range(start..end, &s);
                        let after = ta.text().len();
                        assert_eq!(
                            after as isize,
                            before as isize + (s.len() as isize) - (normalized_len as isize)
                        );
                    }
                    2 => ta.delete_backward(rng.random_range(0..=3)),
                    3 => ta.delete_forward(rng.random_range(0..=3)),
                    4 => ta.delete_backward_word(),
                    5 => ta.kill_to_beginning_of_line(),
                    6 => ta.kill_to_end_of_line(),
                    7 => ta.move_cursor_left(),
                    8 => ta.move_cursor_right(),
                    9 => ta.move_cursor_up(),
                    10 => ta.move_cursor_down(),
                    11 => ta.move_cursor_to_beginning_of_line(true),
                    12 => ta.move_cursor_to_end_of_line(true),
                    13 => {
                        // Insert an element with a unique sentinel payload
                        let payload =
                            format!("[[EL#{}:{}]]", next_elem_id, rng.random_range(1000..9999));
                        next_elem_id += 1;
                        ta.insert_element(&payload, ElementKind(0), None);
                        elem_texts.push(payload);
                    }
                    14 => {
                        // Try inserting inside an existing element (should clamp to boundary)
                        if let Some(payload) = elem_texts.choose(&mut rng).cloned()
                            && let Some(start) = ta.text().find(&payload)
                        {
                            let end = start + payload.len();
                            if end - start > 2 {
                                let pos = rng.random_range(start + 1..end - 1);
                                let ins = rand_grapheme(&mut rng);
                                ta.insert_str_at(pos, &ins);
                            }
                        }
                    }
                    15 => {
                        // Replace a range that intersects an element -> whole element should be replaced
                        if let Some(payload) = elem_texts.choose(&mut rng).cloned()
                            && let Some(start) = ta.text().find(&payload)
                        {
                            let end = start + payload.len();
                            // Create an intersecting range [start-δ, end-δ2)
                            let mut s = start.saturating_sub(rng.random_range(0..=2));
                            let mut e = (end + rng.random_range(0..=2)).min(ta.text().len());
                            // Align to char boundaries to satisfy String::replace_range contract
                            let txt = ta.text();
                            while s > 0 && !txt.is_char_boundary(s) {
                                s -= 1;
                            }
                            while e < txt.len() && !txt.is_char_boundary(e) {
                                e += 1;
                            }
                            if s < e {
                                // Small replacement text
                                let mut srep = String::new();
                                for _ in 0..rng.random_range(0..=2) {
                                    srep.push_str(&rand_grapheme(&mut rng));
                                }
                                ta.replace_range(s..e, &srep);
                            }
                        }
                    }
                    16 => {
                        // Try setting the cursor to a position inside an element; it should clamp out
                        if let Some(payload) = elem_texts.choose(&mut rng).cloned()
                            && let Some(start) = ta.text().find(&payload)
                        {
                            let end = start + payload.len();
                            if end - start > 2 {
                                let pos = rng.random_range(start + 1..end - 1);
                                ta.set_cursor(pos);
                            }
                        }
                    }
                    _ => {
                        // Jump to word boundaries
                        if rng.random_bool(0.5) {
                            let p = ta.beginning_of_previous_word();
                            ta.set_cursor(p);
                        } else {
                            let p = ta.end_of_next_word();
                            ta.set_cursor(p);
                        }
                    }
                }

                // Sanity invariants
                assert!(ta.cursor() <= ta.text().len());

                // Element invariants
                for payload in &elem_texts {
                    if let Some(start) = ta.text().find(payload) {
                        let end = start + payload.len();
                        // 1) Text inside elements matches the initially set payload
                        assert_eq!(&ta.text()[start..end], payload);
                        // 2) Cursor is never strictly inside an element
                        let c = ta.cursor();
                        assert!(
                            c <= start || c >= end,
                            "cursor inside element: {start}..{end} at {c}"
                        );
                    }
                }

                // Render and compute cursor positions; ensure they are in-bounds and do not panic
                let area = Rect::new(0, 0, width, height);
                // Stateless render into an area tall enough for all wrapped lines
                let total_lines = ta.desired_height(width);
                let full_area = Rect::new(0, 0, width, total_lines.max(1));
                let mut buf = Buffer::empty(full_area);
                ratatui::widgets::WidgetRef::render_ref(&(&ta), full_area, &mut buf);

                // cursor_pos: x must be within width when present
                let _ = ta.cursor_pos(area);

                // cursor_pos_with_state: always within viewport rows
                let (_x, _y) = ta
                    .cursor_pos_with_state(area, state)
                    .unwrap_or((area.x, area.y));

                // Stateful render should not panic, and updates scroll
                let mut sbuf = Buffer::empty(area);
                ratatui::widgets::StatefulWidgetRef::render_ref(
                    &(&ta),
                    area,
                    &mut sbuf,
                    &mut state,
                );

                // After wrapping, desired height equals the number of lines we would render without scroll
                let total_lines = total_lines as usize;
                // state.scroll must not exceed total_lines when content fits within area height
                if (height as usize) >= total_lines {
                    assert_eq!(state.scroll, 0);
                }
            }
        }
    }

    // ── Mouse M1: Screen→Buffer mapping tests ──
