    /// Hard-split fragments must project source spans from a monotonic
    /// cursor: a linked run followed by a plain run of the same substring
    /// ("aa") must not re-match earlier bytes — that leaks link style and
    /// hyperlink ranges into the plain fragment.
    #[test]
    fn test_table_hard_split_adjacent_link_and_plain_spans_stay_separate() {
        // Cell "x aaaaaa" wraps at content width 2 into fragments
        // "x" / "aa" / "aa" / "aa": two linked, then one plain.
        let md = "| A |\n|---|\n| x [aaaa](https://example.com)aa |\n\n";

        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(6),
        );
        let lines = lines_to_text(&output.lines);

        // Each content line renders its fragment as exactly one span — a
        // mis-projected fragment straddles two source spans and splits.
        let fragment_spans: Vec<Vec<&str>> = output
            .lines
            .iter()
            .zip(&lines)
            .filter(|(_, text)| text.starts_with('│'))
            .map(|(line, _)| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .filter(|c| !c.contains('│') && !c.trim().is_empty())
                    .collect()
            })
            .collect();
        assert_eq!(
            fragment_spans,
            [["A"], ["x"], ["aa"], ["aa"], ["aa"]],
            "fragments must not straddle span boundaries: {lines:#?}"
        );

        // Only the two linked fragments carry hyperlinks, sharing one id and
        // covering exactly the linked text on their lines.
        let links: Vec<_> = output
            .hyperlinks
            .iter()
            .filter(|h| h.url == "https://example.com")
            .collect();
        assert_eq!(links.len(), 2, "plain fragment must not link: {links:#?}");
        assert_eq!(links[0].id, links[1].id, "fragments must share one link id");
        for link in &links {
            // All chars on these lines are one display cell wide.
            let line = &lines[link.line_index];
            let covered: String = line
                .chars()
                .skip(link.column_range.start)
                .take(link.column_range.len())
                .collect();
            assert_eq!(
                covered, "aa",
                "hyperlink range must cover exactly the linked text: {line:?}"
            );
        }

        // The trailing plain "aa" must render unstyled (no link leak).
        let last_link_line = links.iter().map(|l| l.line_index).max().unwrap_or(0);
        let plain_line = &output.lines[last_link_line + 1];
        let default_style = ratatui::style::Style::default();
        for span in &plain_line.spans {
            let content = span.content.trim();
            if !content.is_empty() && !span.content.contains('│') {
                assert_eq!(content, "aa", "plain fragment content: {plain_line:?}");
                assert_eq!(
                    span.style, default_style,
                    "plain fragment must not inherit link styling"
                );
            }
        }
    }

    /// Table source map: rendered line numbers must not exceed the table's
    /// actual source line count, and must map to the correct source lines.
    #[test]
    fn test_table_source_map_stays_within_bounds() {
        // 4 source lines: header (0), separator (1), row1 (2), row2 (3)
        let md = "| A | B |\n|---|---|\n| x | y |\n| w | z |\n\n";

        let table_start_line = 0usize;
        let table_source_lines = 4usize; // header + separator + 2 rows

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);

        for (i, &src_line) in output.line_source_map.iter().enumerate() {
            assert!(
                src_line < table_start_line + table_source_lines,
                "Rendered line {i} maps to source line {src_line}, \
                 but table only has {table_source_lines} source lines \
                 (0..{}). Source map: {:?}",
                table_start_line + table_source_lines,
                output.line_source_map,
            );
        }
    }

    /// Table source map: header, separator, and body rows map to correct offsets.
    #[test]
    fn test_table_source_map_correct_offsets() {
        let md = "| H1 | H2 |\n|----|----|\n| r1 | r2 |\n| r3 | r4 |\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let map = &output.line_source_map;

        // Find which rendered lines contain table content.
        // Source offsets: header=0, separator=1, row1=2, row2=3
        for (i, line_text) in text.iter().enumerate() {
            let src = map[i];
            if line_text.contains("H1") || line_text.contains("H2") {
                assert_eq!(
                    src, 0,
                    "Header content line {i} should map to source 0, got {src}"
                );
            }
            if line_text.contains("r1") || line_text.contains("r2") {
                assert_eq!(
                    src, 2,
                    "Row 1 content line {i} should map to source 2, got {src}"
                );
            }
            if line_text.contains("r3") || line_text.contains("r4") {
                assert_eq!(
                    src, 3,
                    "Row 2 content line {i} should map to source 3, got {src}"
                );
            }
        }
    }

    /// Table source map with cell wrapping: wrapped continuation lines must
    /// map to the same source line as the first visual line of that row.
    #[test]
    fn test_table_source_map_with_cell_wrapping() {
        let md = "| Name | Description |\n|------|-------------|\n| short | A very long description that will wrap |\n\n";

        let mut buffers = crate::MarkdownBuffers::new();
        let (output, _) = crate::render_markdown_ratatui_with_buffers_width(
            md,
            test_style::STYLE,
            true,
            &mut buffers,
            None,
            Some(30),
        );

        let table_source_lines = 3; // header + separator + 1 row
        for (i, &src_line) in output.line_source_map.iter().enumerate() {
            assert!(
                src_line < table_source_lines,
                "Wrapped table line {i} maps to source {src_line}, \
                 exceeds table source lines ({table_source_lines}). Map: {:?}",
                output.line_source_map,
            );
        }
    }

    /// Fenced block with `lineStart:lineEnd:path` (citation-style) uses the file
    /// extension for syntect, same as a ` ```rust` block.
    #[test]
    fn test_citation_code_fence_highlights_as_rust() {
        let syntect = crate::syntax::test_syntect();
        let code = "const DEFAULT_READ_LIMIT: usize = 2000;\n";
        let md_cite = format!("```37:65:crates/x/read.rs\n{code}```\n\n");
        let md_rust = format!("```rust\n{code}```\n\n");

        let (out_cite, _) =
            render_markdown_ratatui_full(&md_cite, test_style::STYLE, true, Some(syntect));
        let (out_rust, _) =
            render_markdown_ratatui_full(&md_rust, test_style::STYLE, true, Some(syntect));

        fn const_line_span_count(out: &crate::MarkdownRenderOutput) -> usize {
            out.lines
                .iter()
                .find(|l| l.spans.iter().any(|s| s.content.as_ref().contains("const")))
                .expect("line with 'const' should exist")
                .spans
                .len()
        }

        let s_cite = const_line_span_count(&out_cite);
        let s_rust = const_line_span_count(&out_rust);
        assert_eq!(
            s_cite, s_rust,
            "citation fence should match ```rust highlight shape"
        );
        assert!(
            s_cite > 1,
            "const line should have multiple styled spans, got {s_cite}"
        );
    }

    /// InlineHtml (e.g. `<PathBuf>`) inside a table cell must not leak raw
    /// text below the rendered table. Regression for the Replace-inside-table bug.
    #[test]
    fn test_table_inline_html_no_raw_text_leak_ratatui() {
        let md = "| Col A | Col B |\n|-------|-------|\n| Arc<PathBuf> | optimization |\n| normal | row |\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let joined = text.join("\n");

        // The raw markdown pipe syntax must not appear in rendered output
        assert!(
            !joined.contains("| normal"),
            "Raw markdown table syntax leaked below rendered table. Lines: {text:#?}"
        );
        assert!(
            !joined.contains("| optimization"),
            "Raw table cell content leaked as plain text. Lines: {text:#?}"
        );
    }

    /// ANSI render path: same regression — InlineHtml Replace must not
    /// corrupt `last_pos` and re-emit table content as raw text.
    #[test]
    fn test_table_inline_html_no_raw_text_leak_ansi() {
        let md = "| Col A | Col B |\n|-------|-------|\n| Arc<PathBuf> | optimization |\n| normal | row |\n\n";

        let (output, _) = crate::render_markdown(md, test_style::STYLE, true, None);

        assert!(
            !output.contains("| normal"),
            "Raw markdown table syntax leaked in ANSI output. Got: {output}"
        );
        assert!(
            !output.contains("| optimization"),
            "Raw table cell content leaked in ANSI output. Got: {output}"
        );
    }

    /// InlineHtml content must be captured into table cells so it appears
    /// in the formatted table, not silently dropped.
    #[test]
    fn test_table_inline_html_captured_in_cell() {
        let md = "| Type |\n|------|\n| Arc<PathBuf> |\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let all_text: String = text.join("");

        assert!(
            all_text.contains("<PathBuf>"),
            "InlineHtml content should appear in formatted table cell. Got: {text:#?}"
        );
        assert!(
            all_text.contains("Arc"),
            "Text before InlineHtml should appear in cell. Got: {text:#?}"
        );
    }

    /// Multiple HTML-like tags across different cells and rows must all
    /// render correctly without leaking.
    #[test]
    fn test_table_multiple_inline_html_tags() {
        let md = "| Input | Output |\n|-------|--------|\n| Vec<String> | Option<i32> |\n| Box<dyn Trait> | Result<T> |\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let joined = text.join("\n");

        // No raw pipe-delimited rows should leak
        assert!(
            !joined.contains("| Vec"),
            "Raw table syntax leaked with multiple HTML tags. Lines: {text:#?}"
        );
        assert!(
            !joined.contains("| Box"),
            "Raw table syntax leaked with multiple HTML tags. Lines: {text:#?}"
        );

        // Cell content should be present in the table
        let all_text: String = text.join("");
        assert!(
            all_text.contains("Vec"),
            "Vec should appear in table. Got: {text:#?}"
        );
        assert!(
            all_text.contains("Box"),
            "Box should appear in table. Got: {text:#?}"
        );
    }

    /// ANSI render path: multiple HTML-like tags across cells and rows
    /// must not leak raw text via the `current` accumulator merge logic.
    #[test]
    fn test_table_multiple_inline_html_tags_ansi() {
        let md = "| Input | Output |\n|-------|--------|\n| Vec<String> | Option<i32> |\n| Box<dyn Trait> | Result<T> |\n\n";

        let (output, _) = crate::render_markdown(md, test_style::STYLE, true, None);

        assert!(
            !output.contains("| Vec"),
            "Raw table syntax leaked in ANSI multi-tag output. Got: {output}"
        );
        assert!(
            !output.contains("| Box"),
            "Raw table syntax leaked in ANSI multi-tag output. Got: {output}"
        );
    }

    /// Leading content before a table exercises the `push` flush at the
    /// Table Start event followed by the `current.0` reset after rendering.
    #[test]
    fn test_table_with_leading_text_ansi() {
        let md = "Hello world\n\n| Col |\n|-----|\n| Arc<PathBuf> |\n\n";

        let (output, _) = crate::render_markdown(md, test_style::STYLE, true, None);

        assert!(
            output.contains("Hello world"),
            "Leading text should be present. Got: {output}"
        );
        assert!(
            !output.contains("| Col"),
            "Raw table syntax leaked after leading text in ANSI output. Got: {output}"
        );
    }

    #[test]
    fn test_table_br_tag_becomes_line_break() {
        let md = "| Col |\n|-----|\n| hello<br>world |\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let joined = text.join("\n");

        assert!(!joined.contains("<br>"), "literal <br> leaked: {joined}");
        assert!(
            joined.contains("hello") && joined.contains("world"),
            "cell content missing: {joined}"
        );
        assert!(
            !text
                .iter()
                .any(|l| l.contains("hello") && l.contains("world")),
            "hello and world must be on separate visual lines: {joined}"
        );
    }

    #[test]
    fn test_table_br_tag_variants() {
        let md = "| Col |\n|-----|\n| a<BR>b<br/>c<br />d |\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let joined = text.join("\n");

        for tag in ["<BR>", "<br/>", "<br />"] {
            assert!(!joined.contains(tag), "literal {tag} leaked: {joined}");
        }
        for ch in ['a', 'b', 'c', 'd'] {
            assert!(
                text.iter().any(|l| l.contains(ch)),
                "segment '{ch}' missing: {joined}"
            );
        }
    }

    #[test]
    fn test_table_br_tag_ansi() {
        let md = "| Col |\n|-----|\n| hello<br>world |\n\n";

        let (output, _) = crate::render_markdown(md, test_style::STYLE, true, None);
        assert!(
            !output.contains("<br>"),
            "literal <br> in ANSI output: {output}"
        );
    }

    #[test]
    fn test_br_tag_outside_table() {
        let md = "hello<br>world\n\n";

        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let joined = text.join("\n");

        assert!(
            !joined.contains("<br>"),
            "literal <br> outside table: {joined}"
        );
    }

    // CommonMark soft breaks collapse to a single space inside a plain
    // paragraph; hard breaks and block-container continuations (list
    // items, blockquotes) still split into separate visual lines.

    #[test]
    fn test_soft_break_plain_paragraph_collapses_to_space() {
        let md = "Foo bar\nbaz qux.";
        for pretty in [false, true] {
            let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, pretty, None);
            let text = lines_to_text(&output.lines);
            assert_eq!(text, vec!["Foo bar baz qux."], "pretty={pretty}: {text:?}");
        }
    }

    #[test]
    fn test_soft_break_original_bug_repro() {
        let md = "- Tiny emit guard in pretty.rs: empty-reflowed KDoc output with no \"<decl>\n\" pollution).";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text.len(), 1, "got: {text:?}");
        assert!(
            text[0].contains("no \"<decl> \" pollution)."),
            "got: {text:?}"
        );
    }

    #[test]
    fn test_soft_break_multiple_consecutive() {
        let md = "alpha\nbeta\ngamma";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text, vec!["alpha beta gamma"], "got: {text:?}");
    }

    #[test]
    fn test_soft_break_around_inline_html_decl_tag() {
        // <decl> arrives as Event::InlineHtml; the following `\n` is the soft break.
        let md = "Foo bar <decl>\nbaz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text.len(), 1, "got: {text:?}");
        assert!(text[0].contains("<decl> baz qux."), "got: {text:?}");
    }

    #[test]
    fn test_soft_break_ansi_render_path_no_mid_sentence_newline() {
        // render_ansi has its own `split('\n')` loop; verify the parser fix reaches it.
        let md = "Foo bar\nbaz qux.";
        let (output, _) = crate::render_markdown(md, test_style::STYLE, false, None);
        let body = output.trim_end_matches('\n');
        assert!(!body.contains('\n'), "{output:?}");
        assert!(body.contains("Foo bar baz qux."), "{output:?}");
    }

    #[test]
    fn test_hard_break_two_trailing_spaces_still_breaks() {
        let md = "Foo bar  \nbaz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text.len(), 2, "got: {text:?}");
        assert_eq!(text[0].trim_end(), "Foo bar");
        assert_eq!(text[1], "baz qux.");
    }

    #[test]
    fn test_hard_break_backslash_still_breaks() {
        let md = "Foo bar\\\nbaz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text.len(), 2, "got: {text:?}");
        assert!(text[0].starts_with("Foo bar"), "got: {text:?}");
        assert_eq!(text[1], "baz qux.");
    }

    #[test]
    fn test_inline_br_tag_still_breaks() {
        let md = "Foo bar<br>baz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        let joined = text.join("\n");
        assert!(!joined.contains("<br>"), "{joined:?}");
        assert!(text.len() >= 2, "{text:?}");
    }

    #[test]
    fn test_code_block_internal_newlines_still_break() {
        let md = "```rust\nfn foo() {}\nfn bar() {}\n```\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines);
        let foo_idx = text.iter().position(|l| l.contains("fn foo() {}"));
        let bar_idx = text.iter().position(|l| l.contains("fn bar() {}"));
        assert!(
            foo_idx.is_some() && bar_idx.is_some() && foo_idx != bar_idx,
            "got: {text:?}",
        );
    }

    #[test]
    fn test_inline_code_with_real_newline_still_splits() {
        // Inline code's `\n` is part of the Event::Code source slice; no
        // SoftBreak fires, so the fix must not over-collapse it.
        let md = "foo `bar\nbaz` qux";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines);
        assert!(text.len() >= 2, "got: {text:?}");
        let bar_idx = text.iter().position(|l| l.contains("bar"));
        let baz_idx = text.iter().position(|l| l.contains("baz"));
        assert!(
            bar_idx.is_some() && baz_idx.is_some() && bar_idx != baz_idx,
            "got: {text:?}",
        );
    }

    #[test]
    fn test_soft_break_in_bullet_list_item_preserves_lines() {
        // Lazy continuation inside a list item is a soft break, but the
        // continuation indent belongs to a new visual line; collapsing
        // would leave stray indent whitespace mid-line.
        let md = "- first line\n  second line\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text.len(), 2, "got: {text:?}");
        assert!(text[0].contains("first line"), "got: {text:?}");
        assert!(text[1].contains("second line"), "got: {text:?}");
    }

    #[test]
    fn test_soft_break_in_blockquote_preserves_lines() {
        // Continuation `>` markers belong to new visual lines; collapsing
        // would leak a stray `│` (pretty) or `>` (raw) mid-paragraph.
        let md = "> first line\n> second line\n";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, true, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text.len(), 2, "got: {text:?}");
        assert!(text[0].contains("first line"), "got: {text:?}");
        assert!(text[1].contains("second line"), "got: {text:?}");
        assert!(
            !text[0].contains("second line") && !text[1].contains("first line"),
            "lines must not collapse: {text:?}",
        );
    }

    #[test]
    fn test_soft_break_crlf_range_preserves_length() {
        // pulldown emits SoftBreak with a 2-byte range for CRLF; the
        // transform must replace both bytes to keep the byte-length
        // invariant force transforms rely on in render_ansi.
        let md = "Foo bar\r\nbaz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        let text = lines_to_text(&output.lines);
        assert_eq!(text, vec!["Foo bar  baz qux."], "got: {text:?}");

        let (ansi, _) = crate::render_markdown(md, test_style::STYLE, false, None);
        assert!(!ansi.trim_end_matches('\n').contains('\n'), "{ansi:?}");
        assert!(ansi.contains("Foo bar  baz qux."), "{ansi:?}");
    }

    #[test]
    fn test_source_map_preserved_for_soft_break_collapse() {
        let md = "Foo bar\nbaz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        assert_eq!(output.lines.len(), 1);
        assert_eq!(output.line_source_map.len(), 1);
        assert!(
            output.line_source_map[0] <= 1,
            "got {}",
            output.line_source_map[0]
        );
    }

    #[test]
    fn test_source_map_preserved_for_hard_break() {
        let md = "Foo bar  \nbaz qux.";
        let (output, _) = render_markdown_ratatui_full(md, test_style::STYLE, false, None);
        assert_eq!(output.lines.len(), 2, "lines: {:?}", output.lines);
        assert_eq!(output.line_source_map, vec![0, 1]);
    }

    // Soft-break inside a markdown link is covered by
    // `hyperlinks::hyperlink_tests::soft_break_inside_link_text_preserves_column_range`.

    /// An indented fenced code block (common when an LLM nests code under a
    /// list, or simply indents the fence) must render the same as a
    /// non-indented one: pulldown-cmark strips the indentation from the
    /// content, and the renderer must hide the indentation on the opening
    /// fence line too. Regression test for the bug where the first content
    /// line kept its leading indentation and a spurious blank line was
    /// appended.
    #[test]
    fn test_indented_fenced_code_block_strips_indentation() {
        let syn = crate::syntax::test_syntect();
        let indented = "  ```cpp\n  cellContChargeLimits_S cellContChargeLimits;\n  cellChargeTables_S     cellChargeTables;\n  ```\n";

        let (output, _) =
            render_markdown_ratatui_full(indented, test_style::STYLE, true, Some(syn));
        let text = lines_to_text(&output.lines);

        assert_eq!(
            text,
            vec![
                "cellContChargeLimits_S cellContChargeLimits;",
                "cellChargeTables_S     cellChargeTables;",
            ],
            "indented code block should render dedented with no spurious blank line: {text:#?}",
        );
    }

    /// Indented and non-indented code blocks must produce identical pretty
    /// output (the indentation is purely structural).
    #[test]
    fn test_indented_code_block_matches_non_indented() {
        let syn = crate::syntax::test_syntect();
        let non_indented = "```rust\nfn main() {\n    let x = 1;\n}\n```\n";
        let indented = "   ```rust\n   fn main() {\n       let x = 1;\n   }\n   ```\n";

        let (out_plain, _) =
            render_markdown_ratatui_full(non_indented, test_style::STYLE, true, Some(syn));
        let (out_indent, _) =
            render_markdown_ratatui_full(indented, test_style::STYLE, true, Some(syn));

        assert_eq!(
            lines_to_text(&out_plain.lines),
            lines_to_text(&out_indent.lines),
            "indented fenced code block should match non-indented output",
        );
    }

    /// A fenced code block nested inside a list item renders dedented, with a
    /// single blank separator before the code and no leading indentation
    /// leaking onto the first code line.
    #[test]
    fn test_code_block_in_list_strips_indentation() {
        let syn = crate::syntax::test_syntect();
        let in_list = "1. Do this:\n   ```cpp\n   int x = 1;\n   int y = 2;\n   ```\n";

        let (output, _) = render_markdown_ratatui_full(in_list, test_style::STYLE, true, Some(syn));
        let text = lines_to_text(&output.lines);

        // No rendered line should begin with leftover indentation.
        let x_idx = text.iter().position(|l| l.contains("int x = 1;")).unwrap();
        let y_idx = text.iter().position(|l| l.contains("int y = 2;")).unwrap();
        assert_eq!(text[x_idx], "int x = 1;", "first code line: {text:#?}");
        assert_eq!(text[y_idx], "int y = 2;", "second code line: {text:#?}");
        assert!(
            text.last().is_some_and(|l| !l.is_empty()),
            "no spurious trailing blank line: {text:#?}",
        );
    }
