    #[test]
    fn elements_returns_sorted_slice() {
        let mut t = TextArea::new();
        let kind = ElementKind(0);

        t.insert_str("aaa ");
        t.insert_element("BBB", kind, None);
        t.insert_str(" ccc ");
        t.insert_element("DDD", kind, None);

        let elems = t.elements();
        assert_eq!(elems.len(), 2);
        assert!(elems[0].range.start < elems[1].range.start);
        assert_eq!(&t.text()[elems[0].range.clone()], "BBB");
        assert_eq!(&t.text()[elems[1].range.clone()], "DDD");
    }

    // ===== Phase 2: Display rendering & truncation tests =====

    #[test]
    fn render_element_with_display_shows_display_text() {
        use ratatui::style::Stylize;

        let mut t = TextArea::new();
        let display = Line::from("[Pasted]".cyan());
        t.insert_element("raw content here", ElementKind(0), Some(display));

        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        // The rendered buffer should show "[Pasted]" not "raw content here"
        let rendered: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect::<String>();
        let rendered = rendered.trim_end();
        assert_eq!(rendered, "[Pasted]");
    }

    #[test]
    fn render_element_without_display_shows_buffer_text_cyan() {
        let mut t = TextArea::new();
        t.insert_element("hello", ElementKind(0), None);

        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        // Should show "hello" with cyan foreground
        let cell = buf.cell((0, 0)).unwrap();
        assert_eq!(cell.symbol(), "h");
        assert_eq!(cell.fg, Color::Cyan);
    }

    #[test]
    fn truncate_line_display_no_truncation_needed() {
        let line = Line::from("[Short]");
        let result = truncate_line_display(&line, 20);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "[Short]");
    }

    #[test]
    fn truncate_line_display_with_bracket_preservation() {
        let line: Line<'static> = Line::from("[Pasted ~100 lines]");
        // Width 12: budget = 12 - 2 (ellipsis + bracket) = 10 chars content
        let result = truncate_line_display(&line, 12);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with(']'), "should preserve ]: got {text:?}");
        assert!(text.contains('…'), "should contain ellipsis: got {text:?}");
        assert_eq!(text, "[Pasted ~1…]");
    }

    #[test]
    fn truncate_line_display_without_bracket() {
        let line: Line<'static> = Line::from("very long display text");
        let result = truncate_line_display(&line, 10);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('…'));
        assert!(!text.ends_with(']'));
        // 9 chars content + 1 ellipsis = 10
        assert_eq!(text, "very long…");
    }

    #[test]
    fn truncate_line_display_zero_width() {
        let line: Line<'static> = Line::from("[Pasted]");
        let result = truncate_line_display(&line, 0);
        assert!(result.spans.is_empty() || result.width() == 0);
    }

    #[test]
    fn truncate_line_display_width_1() {
        let line: Line<'static> = Line::from("[Pasted]");
        let result = truncate_line_display(&line, 1);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        // Only room for "…" (the bracket can't fit with content)
        assert_eq!(text, "…");
    }

    #[test]
    fn truncate_preserves_multi_span_styles() {
        use ratatui::text::Span;

        let line: Line<'static> = Line::from(vec![
            Span::styled("[", Style::default().fg(Color::Yellow)),
            Span::styled("Pasted ~100 lines", Style::default().fg(Color::Cyan)),
            Span::styled("]", Style::default().fg(Color::Yellow)),
        ]);
        let result = truncate_line_display(&line, 10);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with(']'));
        assert!(text.contains('…'));
        // "[" (1) + content budget (10-2=8) + "…" (1) + "]" (1) = 11? No...
        // Budget: 10 - 2 = 8 for content. "[" is 1, so 7 more chars of "Pasted ~"
        // Result: "[Pasted …]" which is 10 wide
        assert!(
            result.width() <= 10,
            "width should be <= 10, got {}",
            result.width()
        );
    }

    #[test]
    fn render_element_with_prefix_text() {
        let mut t = TextArea::new();
        t.insert_str("hi ");
        let display = Line::from("[P]");
        t.insert_element("raw", ElementKind(0), Some(display));

        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        let rendered: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect::<String>();
        let rendered = rendered.trim_end();
        assert_eq!(rendered, "hi [P]");
    }

    #[test]
    fn render_text_after_element_uses_display_width() {
        // User scenario: "foo " + element("Clean build", display="[📎 Pasted 1 line, 11 chars]") + " abcde"
        // Display: "[📎 Pasted 1 line, 11 chars]" = 1+2+1+23+1 = 28 display cols
        // Buffer: "Clean build" = 11 bytes
        // Without fix, text after element renders at buffer x, overlapping with element display.
        let mut t = TextArea::new();
        t.insert_str("foo ");
        let display = Line::from(vec![
            ratatui::text::Span::raw("["),
            ratatui::text::Span::raw("📎 "),
            ratatui::text::Span::raw("Pasted 1 line, 11 chars"),
            ratatui::text::Span::raw("]"),
        ]);
        t.insert_element("Clean build", ElementKind(0), Some(display));
        t.insert_str(" abcde");

        // Area wide enough to fit everything
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        // Verify key cells: text before element, element display, text after element.
        // "foo " occupies cols 0-3
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "f");
        assert_eq!(buf.cell((3, 0)).unwrap().symbol(), " ");

        // Element display starts at col 4: "[📎 Pasted 1 line, 11 chars]"
        assert_eq!(buf.cell((4, 0)).unwrap().symbol(), "[");
        assert_eq!(buf.cell((5, 0)).unwrap().symbol(), "📎");
        // col 6 is the wide-char continuation cell
        assert_eq!(buf.cell((7, 0)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((8, 0)).unwrap().symbol(), "P");

        // Element display ends at col 31 ("]")
        assert_eq!(buf.cell((31, 0)).unwrap().symbol(), "]");

        // Text after element: " abcde" starting at col 32
        assert_eq!(buf.cell((32, 0)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((33, 0)).unwrap().symbol(), "a");
        assert_eq!(buf.cell((34, 0)).unwrap().symbol(), "b");
        assert_eq!(buf.cell((35, 0)).unwrap().symbol(), "c");
        assert_eq!(buf.cell((36, 0)).unwrap().symbol(), "d");
        assert_eq!(buf.cell((37, 0)).unwrap().symbol(), "e");
    }

    #[test]
    fn render_text_after_wider_display_element_simple() {
        // Simpler case: element buffer text "x" (1 byte), display "[LONG]" (6 cols)
        // Suffix text "!" should render at column 6, not column 1.
        let mut t = TextArea::new();
        let display = Line::from("[LONG]");
        t.insert_element("x", ElementKind(0), Some(display));
        t.insert_str("!");

        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        let rendered: String = (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect::<String>();
        let rendered = rendered.trim_end();
        assert_eq!(rendered, "[LONG]!");
    }

    // ===== Phase 3: Display projection tests =====

    #[test]
    fn display_width_of_range_plain_text() {
        let t = ta_with("hello world");
        assert_eq!(t.display_width_of_range(0, 5), 5); // "hello"
        assert_eq!(t.display_width_of_range(0, 11), 11); // "hello world"
        assert_eq!(t.display_width_of_range(6, 11), 5); // "world"
    }

    #[test]
    fn insert_expands_tabs_to_spaces() {
        let mut t = TextArea::new();
        assert_eq!(t.tab_width(), 4);
        t.insert_str("a\tb");
        assert_eq!(t.text(), "a    b");
        assert_eq!(t.cursor(), 6);
        assert_eq!(t.display_width_of_range(0, t.text().len()), 6);

        let mut t2 = TextArea::new();
        t2.set_tab_width(8);
        t2.insert_str("x\ty");
        assert_eq!(t2.text(), "x        y");
        assert_eq!(t2.cursor(), 10);
        assert_eq!(t2.display_width_of_range(0, t2.text().len()), 10);

        let mut t3 = TextArea::new();
        t3.insert_str("\ta");
        assert_eq!(t3.text(), "    a");
        t3.set_text("");
        t3.insert_str("a\t");
        assert_eq!(t3.text(), "a    ");
        assert_eq!(t3.cursor(), 5);
        t3.set_text("");
        t3.insert_str("\t\t");
        assert_eq!(t3.text(), "        ");
        assert_eq!(t3.display_width_of_range(0, 8), 8);
        t3.insert_str("");
        assert_eq!(t3.text(), "        ");
        t3.insert_str_at(0, "z\t");
        assert_eq!(t3.text(), "z            ");
    }

    #[test]
    fn set_text_and_replace_expand_tabs() {
        let mut t = TextArea::new();
        t.set_text("col1\tcol2");
        assert_eq!(t.text(), "col1    col2");

        t.replace_range(4..8, "\t");
        assert_eq!(t.text(), "col1    col2");

        t.replace_range(4..4, "\tx");
        assert_eq!(t.text(), "col1    x    col2");
        // Insert-only replace places cursor at end of inserted expansion (4 spaces + 'x').
        assert_eq!(&t.text()[4..9], "    x");

        let mut t0 = TextArea::new();
        t0.set_tab_width(0);
        t0.insert_str("a\tb");
        assert_eq!(t0.text(), "a\tb");
        t0.set_text("x\ty");
        assert_eq!(t0.text(), "x\ty");
        // Passthrough: display width matches unicode-width (no expansion).
        assert_eq!(
            t0.display_width_of_range(0, t0.text().len()),
            "x\ty".width()
        );
        t0.set_cursor(t0.text().len());
        let area = Rect::new(0, 0, 80, 1);
        let (x, _y) = t0.cursor_pos(area).unwrap();
        assert_eq!(x as usize, "x\ty".width());
    }

    #[test]
    fn remaining_tabs_count_in_display_width_and_cursor() {
        // Simulate leftover tabs without going through expand (tab_width set after set_text
        // would still expand on set_text; inject via tab_width=0 then enable display tabs).
        let mut t = TextArea::new();
        t.set_tab_width(0);
        t.set_text("a\tb\tc");
        assert!(t.text().contains('\t'));
        t.set_tab_width(4);
        // "a" + 4 + "b" + 4 + "c" = 11
        assert_eq!(t.display_width_of_range(0, t.text().len()), 11);
        t.set_cursor(t.text().len());
        let area = Rect::new(0, 0, 80, 1);
        let (x, _y) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 11);
    }

    #[test]
    fn set_tab_width_does_not_rewrite_existing_spaces() {
        let mut t = TextArea::new();
        t.insert_str("a\tb");
        assert_eq!(t.text(), "a    b");
        t.set_tab_width(8);
        assert_eq!(t.text(), "a    b");
        t.insert_str("\tc");
        assert_eq!(t.text(), "a    b        c");
    }

    #[test]
    fn multi_column_paste_tabs_readable() {
        let mut t = TextArea::new();
        t.insert_str("Name\tAge\tCity\nAda\t36\tLondon");
        assert_eq!(t.text(), "Name    Age    City\nAda    36    London");
        assert_eq!(t.cursor(), t.text().len());
        let end = t.text().len();
        let area = Rect::new(0, 0, 80, 3);
        let (x, _y) = t.cursor_pos(area).unwrap();
        // Ada(3) + 4 + 36(2) + 4 + London(6) = 19
        assert_eq!(x, 19);
        let bol = t.text().rfind('\n').map(|i| i + 1).unwrap_or(0);
        assert_eq!(x as usize, t.display_width_of_range(bol, end));
        let last_line = &t.text()[bol..];
        let (paint, paint_w) = paint_plain_for_display(last_line, 80, 4);
        assert_eq!(paint.as_ref(), last_line);
        assert_eq!(paint_w, 19);
    }

    #[test]
    fn insert_element_expands_tabs_and_covers_full_range() {
        let mut t = TextArea::new();
        t.insert_element("a\tb", ElementKind(0), None);
        assert_eq!(t.text(), "a    b");
        assert_eq!(t.elements().len(), 1);
        assert_eq!(t.elements()[0].range, 0..6);
        assert_eq!(t.cursor(), 6);

        let mut t2 = TextArea::new();
        t2.insert_element("a\tb\nc\td", ElementKind(1), Some(Line::from("[P]")));
        assert_eq!(t2.text(), "a    b\nc    d");
        assert_eq!(t2.elements()[0].range, 0..t2.text().len());
        assert_eq!(t2.cursor(), t2.text().len());
        assert!(!t2.text().contains('\t'));
    }

    #[test]
    fn replace_range_with_element_expands_tabs() {
        let mut t = TextArea::new();
        t.insert_str("xx");
        t.replace_range_with_element(0..2, "a\tb", ElementKind(0), None);
        assert_eq!(t.text(), "a    b");
        assert_eq!(t.elements()[0].range, 0..6);
        assert_eq!(t.cursor(), 6);
    }

    #[test]
    fn unicode_plus_tabs_expansion_and_residual() {
        let mut t = TextArea::new();
        t.insert_str("名\tAge");
        // 名 is typically width 2; plus 4 spaces + Age
        assert_eq!(t.text(), "名    Age");
        assert_eq!(
            t.display_width_of_range(0, t.text().len()),
            "名".width() + 4 + 3
        );
        t.set_cursor(t.text().len());
        let area = Rect::new(0, 0, 80, 1);
        let (x, _) = t.cursor_pos(area).unwrap();
        assert_eq!(x as usize, "名".width() + 4 + 3);

        let mut t2 = TextArea::new();
        t2.set_tab_width(0);
        t2.set_text("😀\tb");
        t2.set_tab_width(4);
        let expected = "😀".width() + 4 + 1;
        assert_eq!(t2.display_width_of_range(0, t2.text().len()), expected);
        t2.set_cursor(t2.text().len());
        let (x, _) = t2.cursor_pos(area).unwrap();
        assert_eq!(x as usize, expected);
    }

    #[test]
    fn tab_helpers_clip_and_paint() {
        assert_eq!(expand_tabs_with_width("a\tb", 4).as_ref(), "a    b");
        assert!(matches!(
            expand_tabs_with_width("a\tb", 0),
            std::borrow::Cow::Borrowed("a\tb")
        ));
        assert!(matches!(
            expand_tabs_with_width("ab", 4),
            std::borrow::Cow::Borrowed("ab")
        ));
        assert_eq!(plain_display_width_with_tab("a\tb\tc", 4), 11);
        assert_eq!(
            plain_display_width_with_tab("a\tb\tc", 0),
            "a\tb\tc".width()
        );
        assert_eq!(clip_str_to_display_width_with_tab("a\tb", 3, 4), "a");
        let (paint, w) = paint_plain_for_display("a\tb", 80, 4);
        assert_eq!(paint.as_ref(), "a    b");
        assert_eq!(w, 6);
        let (paint2, w2) = paint_plain_for_display("a\tb", 3, 4);
        assert_eq!(paint2.as_ref(), "a");
        assert_eq!(w2, 1);
    }

    #[test]
    fn display_width_of_range_with_display_element() {
        let mut t = TextArea::new();
        t.insert_str("ab");
        // Element has 100 bytes of buffer text but displays as "[P]" (3 chars)
        let buffer_text = "x".repeat(100);
        let display = Line::from("[P]");
        t.insert_element(&buffer_text, ElementKind(0), Some(display));
        t.insert_str("cd");

        // Range covering just "ab" = 2
        assert_eq!(t.display_width_of_range(0, 2), 2);
        // Range covering "ab" + element = 2 + 3 = 5
        assert_eq!(t.display_width_of_range(0, 102), 5);
        // Range covering "ab" + element + "cd" = 2 + 3 + 2 = 7
        assert_eq!(t.display_width_of_range(0, 104), 7);
        // Range covering just the element = 3
        assert_eq!(t.display_width_of_range(2, 102), 3);
        // Range covering just "cd" = 2
        assert_eq!(t.display_width_of_range(102, 104), 2);
    }

    #[test]
    fn cursor_pos_uses_display_width() {
        let mut t = TextArea::new();
        t.insert_str("ab");
        let buffer_text = "x".repeat(50);
        let display = Line::from("[P]");
        t.insert_element(&buffer_text, ElementKind(0), Some(display));
        t.insert_str("cd");

        // Cursor at end of element (buffer pos 52)
        t.set_cursor(52);
        let area = Rect::new(0, 0, 80, 1);
        let (x, _y) = t.cursor_pos(area).unwrap();
        // Expected: "ab" (2) + "[P]" (3) = column 5
        assert_eq!(x, 5);

        // Cursor at end of text (buffer pos 54)
        t.set_cursor(54);
        let (x, _y) = t.cursor_pos(area).unwrap();
        // Expected: "ab" (2) + "[P]" (3) + "cd" (2) = column 7
        assert_eq!(x, 7);

        // Cursor at start
        t.set_cursor(0);
        let (x, _y) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 0);
    }

    #[test]
    fn display_width_no_elements() {
        let t = ta_with("abc");
        assert_eq!(t.display_width_of_range(0, 3), 3);
        assert_eq!(t.display_width_of_range(1, 2), 1);
        assert_eq!(t.display_width_of_range(3, 3), 0);
    }

    #[test]
    fn display_width_element_without_display() {
        let mut t = TextArea::new();
        t.insert_element("elem", ElementKind(0), None);
        // No display override — width should equal buffer text width
        assert_eq!(t.display_width_of_range(0, 4), 4);
    }

    // ===== Wide unicode in display text =====

    #[test]
    fn display_width_with_wide_unicode_display() {
        let mut t = TextArea::new();
        t.insert_str("ab");
        // Display text has emoji (each width 2) and CJK
        let display = Line::from("📎漢字"); // 2 + 2 + 2 = 6 display columns
        t.insert_element("raw", ElementKind(0), Some(display));
        t.insert_str("cd");

        // "ab" = 2, element display = 6, "cd" = 2 → total 10
        assert_eq!(t.display_width_of_range(0, 2), 2);
        assert_eq!(t.display_width_of_range(2, 5), 6); // element "raw" = 3 bytes
        assert_eq!(t.display_width_of_range(0, 7), 10); // "ab" + elem + "cd"
    }

    #[test]
    fn cursor_pos_with_wide_unicode_display() {
        let mut t = TextArea::new();
        t.insert_str("a");
        // Element display is "🚀" (width 2), buffer text is "xyz" (3 bytes)
        let display = Line::from("🚀");
        t.insert_element("xyz", ElementKind(0), Some(display));
        t.insert_str("b");

        let area = Rect::new(0, 0, 40, 1);

        // Cursor after "a" (at element start → element_at_cursor returns it)
        t.set_cursor(1);
        let (x, _) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 1); // "a" = 1 col

        // Cursor after element (buffer pos 4 = 1 + 3)
        t.set_cursor(4);
        let (x, _) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 3); // "a" (1) + "🚀" (2) = 3

        // Cursor at end "b" (buffer pos 5)
        t.set_cursor(5);
        let (x, _) = t.cursor_pos(area).unwrap();
        assert_eq!(x, 4); // "a" (1) + "🚀" (2) + "b" (1) = 4
    }

    #[test]
    fn truncate_display_with_wide_unicode() {
        // Display: "📎paste" = 2+5 = 7 cols, truncate to 5
        let line: Line<'static> = Line::from("📎paste");
        let result = truncate_line_display(&line, 5);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        // Budget = 5 - 1 (ellipsis) = 4 content cols → "📎pa" (2+1+1=4)
        assert!(text.contains('…'));
        assert!(
            result.width() <= 5,
            "width should be <= 5, got {}",
            result.width()
        );
    }

    #[test]
    fn clip_str_to_display_width_preserves_zwj_graphemes() {
        let s = "👩\u{200D}💻a";

        assert_eq!(clip_str_to_display_width(s, 0), "");
        assert_eq!(clip_str_to_display_width(s, 1), "");
        assert_eq!(clip_str_to_display_width(s, 2), "👩\u{200D}💻");
        assert_eq!(clip_str_to_display_width(s, 3), s);
    }

    #[test]
    fn truncate_display_preserves_zwj_graphemes() {
        let line: Line<'static> = Line::from("👩\u{200D}💻abc");
        let result = truncate_line_display(&line, 3);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();

        assert_eq!(text, "👩\u{200D}💻…");
        assert_eq!(result.width(), 3);
    }

    #[test]
    fn truncate_display_wide_char_at_boundary() {
        // Display: "ab🚀cd" = 2+2+2 = 6 cols, truncate to 4
        // Budget = 4 - 1 = 3 content cols. "ab" = 2, "🚀" = 2 → doesn't fit → "ab…"
        let line: Line<'static> = Line::from("ab🚀cd");
        let result = truncate_line_display(&line, 4);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "ab…");
        assert!(result.width() <= 4);
    }

    #[test]
    fn truncate_display_bracket_with_wide_chars() {
        // "[📎 pasted]" = 1+2+1+6+1 = 11 cols, truncate to 7
        // Budget = 7 - 2 (ellipsis + bracket) = 5 content cols → "[📎 p…]"
        let line: Line<'static> = Line::from("[📎 pasted]");
        let result = truncate_line_display(&line, 7);
        let text: String = result.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with(']'), "should preserve ]: got {text:?}");
        assert!(text.contains('…'));
        assert!(result.width() <= 7, "got {}", result.width());
    }

    #[test]
    fn render_element_with_wide_unicode_display() {
        let mut t = TextArea::new();
        let display = Line::from("📎漢");
        t.insert_element("hidden text", ElementKind(0), Some(display));

        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::WidgetRef::render_ref(&(&t), area, &mut buf);

        // Should show "📎漢" (4 display cols: 2+2)
        let cell0 = buf.cell((0, 0)).unwrap();
        assert_eq!(cell0.symbol(), "📎");
        let cell2 = buf.cell((2, 0)).unwrap();
        assert_eq!(cell2.symbol(), "漢");
    }

    // ===== Element-aware editing behavior (explicit tests) =====

    #[test]
    fn backspace_at_element_end_deletes_entire_element() {
        let mut t = TextArea::new();
        t.insert_str("before ");
        t.insert_element("[paste]", ElementKind(0), None);
        // Cursor is now at end of element
        assert_eq!(t.cursor(), 14); // "before " (7) + "[paste]" (7)

        t.delete_backward(1);
        assert_eq!(t.text(), "before ");
        assert_eq!(t.cursor(), 7);
        assert!(t.elements().is_empty());
    }

    #[test]
    fn delete_at_element_start_deletes_entire_element() {
        let mut t = TextArea::new();
        t.insert_element("[paste]", ElementKind(0), None);
        t.insert_str(" after");
        t.set_cursor(0);

        t.delete_forward(1);
        assert_eq!(t.text(), " after");
        assert_eq!(t.cursor(), 0);
        assert!(t.elements().is_empty());
    }

    #[test]
    fn left_right_navigation_jumps_over_element() {
        let mut t = TextArea::new();
        t.insert_str("a");
        t.insert_element("[elem]", ElementKind(0), None);
        t.insert_str("b");
        // text = "a[elem]b", element at 1..7

        // Start at end, move left: should jump from 8 → 7 (before 'b'),
        // then 7 → 1 (before element, atomic jump), then 1 → 0
        t.set_cursor(8);
        t.move_cursor_left(); // 8 → 7
        assert_eq!(t.cursor(), 7);
        t.move_cursor_left(); // 7 → 1 (atomic jump over "[elem]")
        assert_eq!(t.cursor(), 1);
        t.move_cursor_left(); // 1 → 0
        assert_eq!(t.cursor(), 0);

        // Now right: 0 → 1, then 1 → 7 (atomic jump), then 7 → 8
        t.move_cursor_right(); // 0 → 1
        assert_eq!(t.cursor(), 1);
        t.move_cursor_right(); // 1 → 7 (atomic jump over "[elem]")
        assert_eq!(t.cursor(), 7);
        t.move_cursor_right(); // 7 → 8
        assert_eq!(t.cursor(), 8);
    }

    #[test]
    fn word_delete_backward_removes_element_atomically() {
        let mut t = TextArea::new();
        t.insert_str("prefix ");
        t.insert_element("[pasted content]", ElementKind(0), None);
        // Cursor at end of element
        assert_eq!(t.cursor(), 23); // 7 + 16

        t.delete_backward_word();
        // Should remove the entire element (it's one "word" unit)
        assert_eq!(t.text(), "prefix ");
        assert!(t.elements().is_empty());
    }

    #[test]
    fn word_delete_forward_removes_element_atomically() {
        let mut t = TextArea::new();
        t.insert_element("[element]", ElementKind(0), None);
        t.insert_str(" suffix");
        t.set_cursor(0);

        t.delete_forward_word();
        assert_eq!(t.text(), " suffix");
        assert!(t.elements().is_empty());
    }

    #[test]
    fn kill_to_eol_removes_element_in_range() {
        let mut t = TextArea::new();
        t.insert_str("start ");
        t.insert_element("[elem]", ElementKind(0), None);
        t.insert_str(" end");
        t.set_cursor(6); // right after "start "

        t.kill_to_end_of_line();
        assert_eq!(t.text(), "start ");
        assert!(t.elements().is_empty());
    }

    // ===== Element newline skipping in BOL/EOL =====
    //
    // Elements with multi-line buffer text (e.g. paste blocks) should be
    // treated as atomic for line navigation. Newlines inside elements are
    // NOT line boundaries.

    #[test]
    fn ctrl_e_skips_newline_inside_element() {
        // "foo <element:line1\nline2> bar"
        // Ctrl-E from start should go to end of the whole line, not stop
        // at the \n inside the element.
        let mut t = TextArea::new();
        t.insert_str("foo ");
        t.insert_element("line1\nline2", ElementKind(1), None);
        t.insert_str(" bar");
        // buffer = "foo line1\nline2 bar" (19 bytes), element at 4..15

        t.set_cursor(0);
        t.move_cursor_to_end_of_line(false);
        assert_eq!(t.cursor(), t.text().len()); // should reach end of "foo ... bar"
    }
