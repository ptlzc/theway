    use crate::render_markdown_ratatui_full;
    use crate::style::test_style;

    fn lines_to_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// A fenced `mermaid` block renders as a diagram in pretty mode.
    #[test]
    fn test_mermaid_block_renders_diagram() {
        let md = "```mermaid\ngraph TD\n  A[Start] --> B[End]\n```\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines).join("\n");
        assert!(
            text.contains('┌') || text.contains('╭'),
            "expected box-drawing, got:\n{text}"
        );
        assert!(text.contains("Start") && text.contains("End"), "{text}");
        assert!(text.contains('▼'), "expected an arrowhead, got:\n{text}");
        assert!(!text.contains("```"), "fences should be hidden:\n{text}");
    }

    /// A mermaid fence with trailing info tokens still renders a diagram.
    #[test]
    fn test_mermaid_block_with_info_extras_renders() {
        let md = "```mermaid theme=dark\ngraph TD\n  A[X] --> B[Y]\n```\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines).join("\n");
        assert!(
            text.contains('▼'),
            "info extras should still draw a diagram:\n{text}"
        );
    }

    /// Raw mode shows the mermaid source instead of the diagram.
    #[test]
    fn test_mermaid_block_raw_mode_shows_source() {
        let md = "```mermaid\ngraph TD\n  A[Start] --> B[End]\n```\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines).join("\n");
        assert!(text.contains("graph TD"), "raw should show source:\n{text}");
        assert!(
            !text.contains('▼'),
            "raw should not draw a diagram:\n{text}"
        );
    }

    /// Pretty mode must remove the opening `[` from `[text](url)` links.
    /// Regression test: apply_transforms treated replace-with-empty-string
    /// as "no transform applied" because it checked `result.is_empty()`.
    #[test]
    fn test_pretty_link_bracket_removed() {
        let text = "Here is a [link](https://example.com) in text.\n\n";
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        let lines = lines_to_text(&output.lines);

        assert!(
            !lines[0].contains("[link"),
            "Pretty mode should remove '[' from link. Got: {:?}",
            lines[0]
        );
        assert!(
            lines[0].contains("link (https://example.com)"),
            "Pretty mode should render 'link (url)'. Got: {:?}",
            lines[0]
        );
    }

    /// Same regression for images: `![img](src)` should not show `[img`.
    #[test]
    fn test_pretty_image_bracket_removed() {
        let text = "An ![image](src.png) here.\n\n";
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        let lines = lines_to_text(&output.lines);

        let img_line = &lines[0];
        assert!(
            !img_line.contains("[image"),
            "Pretty mode should remove '[' from image. Got: {:?}",
            img_line
        );
    }

    /// Regression: `count_newlines_in_range` panics when a checkpoint byte
    /// offset from a thematic break falls inside a multi-byte character in
    /// subsequent content (e.g., a 4-byte emoji like 📐).
    ///
    /// Minimal repro: thematic break `---` followed by heading with emoji.
    /// The checkpoint creates a byte offset that lands mid-emoji when used
    /// to slice `self.text` in `text[from..to]`.
    /// Nested blockquote with paragraph break and list inside inner quote.
    #[test]
    fn test_nested_blockquote_with_list() {
        let md = "> Foo\n>\n> > Bar\n> >\n> > - Baz\n";

        let (raw_output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        assert_eq!(
            lines_to_text(&raw_output.lines),
            vec!["> Foo", ">", "> > Bar", "> >", "> > - Baz"],
            "raw mode",
        );

        let (pretty_output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        assert_eq!(
            lines_to_text(&pretty_output.lines),
            vec!["│ Foo", "│", "│ │ Bar", "│ │", "│ │ • Baz"],
            "pretty mode",
        );
    }

    #[test]
    fn test_emoji_after_thematic_break_does_not_panic() {
        // "---\n\n## 📐 H\n\n" — 📐 is at bytes 8..12, checkpoint offset
        // lands at byte 10 (inside the emoji), causing a panic in
        // count_newlines_in_range which does text[from..to].
        let md = "---\n\n## 📐 H\n\n";
        let (_output, _cp) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
    }

    #[test]
    fn test_list_followed_by_code_block_has_separator() {
        // A list item followed by a code block should have a blank line between them
        let md = "1. Hello\n```python\nworld\n```\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        eprintln!("Lines: {text:#?}");

        // Find the list item line and the code block line
        let hello_idx = text.iter().position(|l| l.contains("Hello")).unwrap();
        let world_idx = text.iter().position(|l| l.contains("world")).unwrap();

        // There should be at least one blank line between them
        assert!(
            world_idx - hello_idx >= 2,
            "Expected blank line between list item and code block. \
             hello at {hello_idx}, world at {world_idx}. Lines: {text:#?}"
        );
    }

    #[test]
    fn test_code_block_empty_line_has_bg() {
        use ratatui::style::Color;

        // Create a style with a visible code_background
        let mut style = test_style::STYLE;
        style.code_background = anstyle::Style::new()
            .bg_color(Some(anstyle::Color::Rgb(anstyle::RgbColor(30, 30, 46))));

        let md = "```\nline1\n\nline3\n```\n";
        let (output, _) = render_markdown_ratatui_full(md, style, true, None);

        let expected_bg = Color::Rgb(30, 30, 46);

        // All lines inside the code block should have the bg set
        for (i, line) in output.lines.iter().enumerate() {
            assert_eq!(
                line.style.bg,
                Some(expected_bg),
                "Line {i} ({:?}) should have code_background, got {:?}",
                lines_to_text(std::slice::from_ref(line))[0],
                line.style.bg,
            );
        }
    }

    /// Regression: an unterminated bare fence with no trailing newline (the tail of a
    /// streamed message) must keep code_background on its final line.
    #[test]
    fn test_unterminated_untagged_fence_final_line_has_bg() {
        use ratatui::style::Color;

        let mut style = test_style::STYLE;
        style.code_background = anstyle::Style::new()
            .bg_color(Some(anstyle::Color::Rgb(anstyle::RgbColor(30, 30, 46))));

        let md = "```\nline1\n\nfinal line";
        let (output, _) = render_markdown_ratatui_full(md, style, true, None);

        let texts = lines_to_text(&output.lines);
        assert!(
            texts.last().is_some_and(|l| l.contains("final line")),
            "expected the newline-less final line in output: {texts:#?}"
        );
        let expected_bg = Color::Rgb(30, 30, 46);
        for (i, line) in output.lines.iter().enumerate() {
            assert_eq!(
                line.style.bg,
                Some(expected_bg),
                "Line {i} ({:?}) should have code_background",
                texts[i],
            );
        }
    }

    /// Tables wider than max_table_width should be constrained to fit.
    #[test]
    fn test_table_constrained_to_max_width() {
        use unicode_width::UnicodeWidthStr;

        let md = "| Column A | Column B | Column C |\n|----------|----------|----------|\n| value 1  | value 2  | value 3  |\n\n";

        // Render without constraint — table uses natural widths
        let (output_full, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let full_lines = lines_to_text(&output_full.lines);
        let full_max_width = full_lines.iter().map(|l| l.width()).max().unwrap_or(0);

        // Render with narrow constraint
        let narrow = 30;
        assert!(
            full_max_width > narrow,
            "Table should be wider than {narrow} naturally"
        );

        let mut buffers = crate::MarkdownBuffers::new();
        let (output_narrow, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(narrow),
        );
        let narrow_lines = lines_to_text(&output_narrow.lines);
        let narrow_max_width = narrow_lines.iter().map(|l| l.width()).max().unwrap_or(0);

        assert!(
            narrow_max_width <= narrow,
            "Constrained table should fit within {narrow} columns, got {narrow_max_width}. Lines: {narrow_lines:#?}"
        );

        // All table lines should still have consistent widths
        let table_widths: Vec<usize> = narrow_lines.iter().map(|l| l.width()).collect();
        let first_width = table_widths[0];
        for (i, &w) in table_widths.iter().enumerate() {
            assert_eq!(
                w, first_width,
                "Table line {i} has width {w}, expected {first_width}"
            );
        }
    }

    /// When columns are shrunk, long cell content should be wrapped within the cell.
    #[test]
    fn test_table_cell_wrapping() {
        let md = "| Very Long Column Name |\n|-----------------------|\n| Short |\n\n";

        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(15),
        );
        let text = lines_to_text(&output.lines);
        eprintln!("Wrapped table: {text:#?}");

        // The header "Very Long Column Name" should be wrapped across multiple lines
        // since it doesn't fit in the constrained column width.
        // All content should still be present (no truncation).
        let all_text: String = text.join("");
        assert!(
            all_text.contains("Very") && all_text.contains("Long") && all_text.contains("Name"),
            "All header words should be present (wrapped, not truncated). Got: {text:#?}"
        );
    }

    /// Cell wrapping should break at punctuation/symbols, not mid-word.
    /// Punct chars attach to whichever side gives a smaller max segment.
    #[test]
    fn test_table_cell_wraps_at_punctuation() {
        use crate::parse::cell_word_separator;

        fn words(s: &str) -> Vec<String> {
            cell_word_separator(s)
                .map(|w| format!("[{}|{}]", w.word, w.whitespace))
                .collect()
        }

        // Equal-length sides: tie goes to attach-left
        assert_eq!(words("foo/bar"), vec!["[foo/|]", "[bar|]"]);

        // Break at space after comma (whitespace break, no attachment choice)
        assert_eq!(words("hello, world"), vec!["[hello,| ]", "[world|]"]);

        // Plain words only break on spaces
        assert_eq!(words("hello world"), vec!["[hello| ]", "[world|]"]);

        // Single-char segments separated by hyphens
        assert_eq!(words("a-b-c"), vec!["[a-|]", "[b-|]", "[c|]"]);

        // Unequal sides: punct attaches to shorter side to minimize max
        // ABCD-EFG: left gives max(5,3)=5, right gives max(4,4)=4 → right
        assert_eq!(words("ABCD-EFG"), vec!["[ABCD|]", "[-EFG|]"]);

        // Comma and dot between digits stay together (number formatting)
        assert_eq!(words("$145,000"), vec!["[$145,000|]"]);
        assert_eq!(words("3.14"), vec!["[3.14|]"]);
        assert_eq!(words("1.0.2"), vec!["[1.0.2|]"]);

        // Hyphens between digits are breakable (phones, dates, IDs)
        // Attachment is chosen to minimize max segment width.
        // 2019-03-15: right gives max(4,3,3)=4 < left max(5,3,2)=5
        assert_eq!(words("2019-03-15"), vec!["[2019|]", "[-03|]", "[-15|]"]);
        // 555-0101: right gives max(3,5)=5 vs left max(4,4)=4 → left
        assert_eq!(words("555-0101"), vec!["[555-|]", "[0101|]"]);
        // Verify a full phone number breaks correctly
        let phone = words("+44-20-7555-0118");
        // All segments should be present, phone is breakable
        assert!(phone.len() > 1, "phone number should be breakable");
        assert_eq!(
            words("(415) 555-0101"),
            vec!["[(415)| ]", "[555-|]", "[0101|]"]
        );
        // EMP-1001: no digit before `-`, and `1` after is not alphabetic →
        // stays together (it's an ID, not digit-punct-digit)
        assert_eq!(words("EMP-1001"), vec!["[EMP-1001|]"]);
    }

    /// URLs should be treated as unbreakable words so that terminal
    /// Cmd+Click detection works when table cells wrap.
    #[test]
    fn test_table_cell_url_not_broken() {
        use crate::parse::cell_word_separator;

        fn words(s: &str) -> Vec<String> {
            cell_word_separator(s)
                .map(|w| format!("[{}|{}]", w.word, w.whitespace))
                .collect()
        }

        // A URL should be a single unbreakable word
        assert_eq!(
            words("https://example.com/path/to/page"),
            vec!["[https://example.com/path/to/page|]"]
        );

        // URL with text before and after breaks at spaces, URL stays intact
        assert_eq!(
            words("see https://example.com/foo for details"),
            vec![
                "[see| ]",
                "[https://example.com/foo| ]",
                "[for| ]",
                "[details|]"
            ]
        );

        // http:// URLs are also preserved
        assert_eq!(
            words("http://example.com/a-b/c"),
            vec!["[http://example.com/a-b/c|]"]
        );

        // Multiple URLs in the same cell
        assert_eq!(
            words("https://a.com/x https://b.com/y"),
            vec!["[https://a.com/x| ]", "[https://b.com/y|]"]
        );

        // URL with query params and fragments
        assert_eq!(
            words("https://example.com/search?q=hello&lang=en#results"),
            vec!["[https://example.com/search?q=hello&lang=en#results|]"]
        );

        // Non-http schemes (ftp, ssh, etc.) are also preserved
        assert_eq!(
            words("ftp://files.example.com/pub/data"),
            vec!["[ftp://files.example.com/pub/data|]"]
        );
        assert_eq!(
            words("ssh://git@github.com/org/repo"),
            vec!["[ssh://git@github.com/org/repo|]"]
        );
    }

    /// Inline formatting (bold, italic, code) should be preserved per-span
    /// when table cells are wrapped across multiple visual lines.
    #[test]
    fn test_table_preserves_inline_formatting() {
        // Table with inline code in a cell
        let md = "| A | B |\n|---|---|\n| 1 | hello world `abc` |\n\n";

        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(30), // narrow enough to force wrapping in column B
        );

        // Find the lines that contain "abc" — they should have a styled span
        // with the code style, not just plain text.
        let mut found_code_span = false;
        for line in &output.lines {
            for span in &line.spans {
                if span.content.contains("abc") {
                    // Inline code should have some style applied (not default)
                    let default_style = ratatui::style::Style::default();
                    assert_ne!(
                        span.style, default_style,
                        "Inline code `abc` should have code formatting, got default style"
                    );
                    found_code_span = true;
                }
            }
        }
        assert!(
            found_code_span,
            "Should find a span containing 'abc' with code formatting"
        );
    }

    /// Regression: table cells containing multi-byte UTF-8 characters (em-dash '—',
    /// CJK, emoji, etc.) could panic with "byte index N is not a char boundary"
    /// when cell wrapping causes `prev_len` (sum of wrapped-line byte lengths) to
    /// land inside a multi-byte character sequence.
    #[test]
    fn test_table_cell_with_multibyte_chars_does_not_panic() {
        // Em-dash '—' is 3 bytes (0xE2 0x80 0x94). Force wrapping so the
        // prev_len calculation for the second visual line can land mid-char.
        let md = "| A |\n|---|\n| hello world — goodbye world |\n\n";
        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(20), // narrow enough to force wrapping around the em-dash
        );
        let text = lines_to_text(&output.lines);
        let all_text: String = text.join("");
        // All content should still be present (no truncation or crash).
        assert!(
            all_text.contains("hello") && all_text.contains("goodbye"),
            "All cell words should be present after wrapping. Got: {text:#?}"
        );
    }

    /// Same regression for CJK and emoji characters in table cells.
    #[test]
    fn test_table_cell_with_cjk_and_emoji_does_not_panic() {
        // Mix CJK (3 bytes each), emoji (4 bytes), and ASCII to stress char boundaries.
        let md = "| Col |\n|-----|\n| \u{4F60}\u{597D}\u{4E16}\u{754C} hello \u{1F680}\u{1F30D} world |\n\n";
        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(15),
        );
        let text = lines_to_text(&output.lines);
        let all_text: String = text.join("");
        assert!(
            all_text.contains("hello") && all_text.contains("world"),
            "ASCII words should survive wrapping with CJK/emoji. Got: {text:#?}"
        );
    }

    /// Split rendered table lines into logical rows of per-column cell text,
    /// concatenating wrapped fragments *without* inserting spaces so that
    /// hard-split tokens reconstruct exactly.
    fn reconstruct_table_cells(lines: &[String]) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut current: Option<Vec<String>> = None;
        for line in lines {
            if let Some(inner) = line.strip_prefix('│').and_then(|l| l.strip_suffix('│')) {
                let cells: Vec<&str> = inner.split('│').collect();
                let row = current.get_or_insert_with(|| vec![String::new(); cells.len()]);
                for (acc, fragment) in row.iter_mut().zip(cells) {
                    acc.push_str(fragment.trim());
                }
            } else if let Some(row) = current.take() {
                // Border or separator line closes the logical row.
                rows.push(row);
            }
        }
        if let Some(row) = current.take() {
            rows.push(row);
        }
        rows
    }

    /// A six-column table of unbreakable tokens must reflow inside cells and
    /// grow taller — never exceed the width budget or lose the right border.
    #[test]
    fn test_table_six_col_unbreakable_tokens_fit_width_50_40_30() {
        use unicode_width::UnicodeWidthStr;

        let md = "| Alpha | Bravo | Ident | DeptName | RoleName | Amount |\n\
                  |---|---|---|---|---|---|\n\
                  | LongalphaToken | TokenTwo | ID-AA1001 | EngineeringOps | ManagerRole | $145,000 |\n\n";

        let (output_full, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let full_lines = lines_to_text(&output_full.lines);
        let full_max_width = full_lines.iter().map(|l| l.width()).max().unwrap_or(0);

        for narrow in [50usize, 40, 30] {
            assert!(
                full_max_width > narrow,
                "fixture must be naturally wider than {narrow}, got {full_max_width}"
            );

            let mut buffers = crate::MarkdownBuffers::new();
            let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
                md,
                test_style::STYLE,
                true,
                &mut buffers,
                None,
                Some(narrow),
            );
            let lines = lines_to_text(&output.lines);
            assert!(!lines.is_empty(), "table should render at width {narrow}");

            let first_width = lines.first().map(|l| l.width()).unwrap_or(0);
            for (i, line) in lines.iter().enumerate() {
                let w = line.width();
                assert!(w <= narrow, "line {i} width {w} exceeds {narrow}: {line:?}");
                assert_eq!(
                    w, first_width,
                    "line {i} width {w} != {first_width}: {line:?}"
                );
                let right_edge = line.trim_end().chars().last();
                assert!(
                    matches!(right_edge, Some('│' | '┐' | '┘' | '┤')),
                    "line {i} lost its right border at width {narrow}: {line:?}"
                );
            }

            // Width pressure must add visual lines (taller rows), not clip.
            let content_lines = lines.iter().filter(|l| l.starts_with('│')).count();
            assert!(
                content_lines > 2,
                "cells should wrap onto extra visual lines at width {narrow}, got {content_lines}"
            );
            assert!(
                lines.len() > full_lines.len(),
                "table should grow taller at width {narrow}: {} vs {} lines",
                lines.len(),
                full_lines.len()
            );

            // Every token must survive the reflow in its own column — each
            // body cell is a single token, so reconstruction is exact.
            let rows = reconstruct_table_cells(&lines);
            assert_eq!(rows.len(), 2, "header + one body row, got {rows:#?}");
            assert_eq!(
                rows[1],
                [
                    "LongalphaToken",
                    "TokenTwo",
                    "ID-AA1001",
                    "EngineeringOps",
                    "ManagerRole",
                    "$145,000",
                ],
                "body cells must reconstruct exactly at width {narrow}"
            );
        }
    }

    /// Hard splits must fall on grapheme boundaries — CJK, VS16 emoji, and
    /// ZWJ clusters stay intact and every line fits the assigned width.
    /// Fixtures are single unbreakable words so the word separator never
    /// contributes break points of its own.
    #[test]
    fn test_wrap_cell_text_grapheme_hard_split_stays_within_width() {
        use unicode_segmentation::UnicodeSegmentation;
        use unicode_width::UnicodeWidthStr;

        for text in [
            "你好世界⚠\u{FE0F}",
            "编码测试👩\u{200D}🚀",
            "你好世👩\u{200D}🚀。",
        ] {
            let widest = text.graphemes(true).map(|g| g.width()).max().unwrap_or(0);
            // Representable width: at least the widest single grapheme.
            let width = widest.max(4);

            let lines = crate::parse::MarkdownParser::wrap_cell_text(text, width);
            assert!(!lines.is_empty(), "wrap must never return an empty vec");
            assert!(
                lines.len() > 1,
                "fixture should force a hard split: {text:?} at width {width}"
            );
            for line in &lines {
                assert!(
                    line.width() <= width,
                    "line {line:?} wider than {width} for {text:?}"
                );
            }
            assert_eq!(
                lines.concat(),
                text,
                "no graphemes may be dropped or reordered"
            );

            // Every split point must be a grapheme boundary of the source.
            let boundaries: std::collections::HashSet<usize> = text
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .chain(std::iter::once(text.len()))
                .collect();
            let mut offset = 0usize;
            for line in &lines {
                offset += line.len();
                assert!(
                    boundaries.contains(&offset),
                    "split at byte {offset} falls inside a grapheme of {text:?}"
                );
            }
        }
    }

    /// A markdown link hard-wrapped across visual lines keeps one shared
    /// hyperlink id + url, in-bounds column ranges, and link styling on
    /// every fragment.
    #[test]
    fn test_table_hard_wrapped_styled_link_keeps_id_and_bounds() {
        use unicode_width::UnicodeWidthStr;

        let md = "| L |\n|---|\n| [clickmenowplease](https://example.com) |\n\n";
        let narrow = 12;

        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(narrow),
        );
        let lines = lines_to_text(&output.lines);

        for (i, line) in lines.iter().enumerate() {
            assert!(
                line.width() <= narrow,
                "line {i} exceeds width {narrow}: {line:?}"
            );
        }

        // All label characters survive the hard wrap (no inserted spaces).
        let rows = reconstruct_table_cells(&lines);
        assert!(
            rows.iter().any(|r| r.concat().contains("clickmenowplease")),
            "label must reconstruct intact: {rows:#?}"
        );

        let links: Vec<_> = output
            .hyperlinks
            .iter()
            .filter(|h| h.url == "https://example.com")
            .collect();
        assert!(
            links.len() >= 2,
            "wrapped label should produce multiple link fragments: {links:#?}"
        );
        let first_id = links[0].id;
        for link in &links {
            assert_eq!(link.id, first_id, "fragments must share one link id");
            let line = &lines[link.line_index];
            assert!(
                link.column_range.end <= line.width(),
                "range {:?} exceeds line {} width {}: {line:?}",
                link.column_range,
                link.line_index,
                line.width()
            );
        }

        // Label fragments keep link styling instead of default spans.
        let default_style = ratatui::style::Style::default();
        let styled_fragments = output
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| {
                let content = span.content.trim();
                !content.is_empty()
                    && "clickmenowplease".contains(content)
                    && span.style != default_style
            })
            .count();
        assert!(
            styled_fragments >= 2,
            "wrapped label fragments must keep link styling"
        );
    }
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/render/unit/blocks.rs"));
