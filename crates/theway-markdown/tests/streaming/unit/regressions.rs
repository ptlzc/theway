    /// Math content must render identically whether it arrives whole or
    /// split at ANY byte boundary (checkpoint/tail re-render interplay).
    #[test]
    fn test_math_doc_2way_splits_match_full() {
        let text = MATH_DOC;

        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // Sanity: the full render actually produced converted math.
        let joined = full_lines.join("\n");
        assert!(joined.contains("e^(iπ) + 1 = 0"), "inline $ math: {joined}");
        assert!(
            joined.contains("∫₀^∞ e⁻ˣ dx = 1"),
            "display $$ math: {joined}"
        );
        assert!(joined.contains("α + β"), "paren inline math: {joined}");
        assert!(
            joined.contains("u + v"),
            "padded paren inline math: {joined}"
        );
        assert!(
            joined.contains("(a+b)/2 ≥ √(ab)"),
            "bracket display math: {joined}"
        );
        assert!(joined.contains("x²"), "table cell math: {joined}");
        assert!(joined.contains("p → q"), "list item math: {joined}");
        assert!(joined.contains("E = mc²"), "blockquote math: {joined}");
        assert!(joined.contains("h = x³"), "heading bracket math: {joined}");
        assert!(joined.contains("f(x) = x²"), "aligned env: {joined}");

        let mut failures = Vec::new();
        for split_at in 1..text.len() {
            if !text.is_char_boundary(split_at) {
                continue;
            }

            let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
            renderer.push_and_render(&text[..split_at], None);
            renderer.push_and_render(&text[split_at..], None);
            let streaming_output = renderer.view();

            let streaming_lines: Vec<String> = streaming_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();

            if streaming_lines != full_lines {
                let diff_line = streaming_lines
                    .iter()
                    .zip(full_lines.iter())
                    .enumerate()
                    .find(|(_, (s, f))| s != f)
                    .map(|(i, _)| i);
                failures.push(format!(
                    "byte {}: stream={} lines, full={} lines, first_diff={:?}",
                    split_at,
                    streaming_lines.len(),
                    full_lines.len(),
                    diff_line,
                ));
            }
        }

        assert!(
            failures.is_empty(),
            "{} failures out of {} split points:\n{}",
            failures.len(),
            text.len() - 1,
            failures.join("\n")
        );
    }

    fn view_text_lines(r: &StreamingMarkdownRenderer) -> Vec<String> {
        r.view()
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// `\(…\)` / `\[…\]` inside a table cell must converge under
    /// streaming and convert to Unicode (the normalizer rewrites them to
    /// `$`/`$$` before parsing, so the in-cell math path handles them).
    #[test]
    fn streaming_table_with_backslash_math_matches_full() {
        let doc =
            "| Mode | Metric |\n|------|--------|\n| Open | \\(\\alpha\\) then \\[x^2\\] |\n\n";
        assert_streaming_matches_full_both(doc);
        let joined = view_text_lines(&{
            let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
            r.push_and_render(doc, None);
            r.finish(None);
            r
        })
        .join("\n");
        assert!(joined.contains('α'), "cell math converted: {joined:?}");
        assert!(
            joined.contains("x²"),
            "cell display math converted: {joined:?}"
        );
        assert!(!joined.contains("\\("), "no raw TeX: {joined:?}");
    }

    /// `clone()` must reproduce the rendered output exactly even when the source
    /// contained backslash math (which is normalized into `source`).
    #[test]
    fn clone_reproduces_backslash_math_output() {
        let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        r.push_and_render("Sum \\(\\alpha + \\beta\\) and \\[x^2\\].\n\n", None);
        let cloned = r.clone();
        assert_eq!(view_text_lines(&r), view_text_lines(&cloned));
    }

    /// `clone()` must preserve the normalizer's held-back pending state: stream
    /// up to a chunk boundary that holds back a trailing `\`, clone, then feed
    /// the completion to both — they must stay identical and convert correctly.
    #[test]
    fn clone_preserves_held_back_pending() {
        let mut r = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        r.push_and_render("ab \\(\\alpha\\) cd \\", None); // trailing `\` held back
        let mut cloned = r.clone();
        r.push_and_render("(\\beta\\) ef\n\n", None);
        cloned.push_and_render("(\\beta\\) ef\n\n", None);
        r.finish(None);
        cloned.finish(None);
        assert_eq!(view_text_lines(&r), view_text_lines(&cloned));
        let joined = view_text_lines(&r).join("\n");
        assert!(
            joined.contains('α') && joined.contains('β'),
            "both math spans converted: {joined:?}"
        );
        assert!(!joined.contains('\\'), "no raw backslashes: {joined:?}");
    }

    /// finish() (full re-render) must also match the incremental view for
    /// math-heavy content streamed in small chunks.
    #[test]
    fn test_math_doc_small_chunks_finish_matches_full() {
        let text = MATH_DOC;
        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for chunk in split_into_chunks(text, 3) {
            renderer.push_and_render(chunk, None);
        }
        renderer.finish(None);
        let view = renderer.view();

        let view_lines: Vec<String> = view
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(view_lines, full_lines);
    }
