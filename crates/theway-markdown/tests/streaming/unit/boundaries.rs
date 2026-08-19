    /// Test ALL possible split points for a smaller test document.
    /// For a document of N bytes, there are N-1 possible split points.
    /// This test catches edge cases at every possible boundary.
    #[test]
    fn test_all_split_points() {
        // A smaller document that covers key features
        let text = "# Heading\n\n1. Item\n2. Item\n\n> Quote\n\n";

        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // Test every possible split point
        for split_at in 1..text.len() {
            if !text.is_char_boundary(split_at) {
                continue;
            }

            let chunk1 = &text[..split_at];
            let chunk2 = &text[split_at..];

            let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
            renderer.push_and_render(chunk1, None);
            let view1 = renderer.view();
            let lines_after_chunk1: Vec<String> = view1
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            let frozen_before = (renderer.frozen.source_bytes, renderer.frozen.lines_len);

            renderer.push_and_render(chunk2, None);
            let streaming_output = renderer.view();

            let streaming_lines: Vec<String> = streaming_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();

            assert_eq!(
                streaming_lines,
                full_lines,
                "Split at byte {}: MISMATCH\n\
                chunk1: {:?}\n\
                chunk2: {:?}\n\
                After chunk1: {:?} (frozen: {:?})\n\
                Streaming lines: {:?}\n\
                Full lines: {:?}",
                split_at,
                chunk1,
                chunk2,
                lines_after_chunk1,
                frozen_before,
                streaming_lines,
                full_lines
            );
        }
    }

    /// Test blockquote specifically - this is reported as broken in demo
    #[test]
    fn test_blockquote_multiline_streaming() {
        let text = "> Line 1\n> Line 2\n\n";

        // Split in various ways
        for chunk_size in 1..=text.len() {
            let chunks = split_into_chunks(text, chunk_size);
            let rejoined: String = chunks.iter().copied().collect();
            assert_eq!(rejoined, text);

            let (full_output, _) =
                render_markdown_ratatui_full(text, test_style::STYLE, true, None);

            let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
            for chunk in &chunks {
                renderer.push_and_render(chunk, None);
                // push() now renders automatically
            }
            let streaming_output = renderer.view();

            let streaming_lines: Vec<String> = streaming_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            let full_lines: Vec<String> = full_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();

            assert_eq!(
                streaming_lines, full_lines,
                "Blockquote with chunk_size {}: mismatch\nStreaming: {:?}\nFull: {:?}",
                chunk_size, streaming_lines, full_lines
            );
        }
    }

    /// Test numbered list specifically - freestanding "1." bug
    #[test]
    fn test_numbered_list_streaming() {
        let text = "1. First\n2. Second\n3. Third\n\n";

        // Split in various ways
        for chunk_size in 1..=text.len() {
            let chunks = split_into_chunks(text, chunk_size);
            let rejoined: String = chunks.iter().copied().collect();
            assert_eq!(rejoined, text);

            let (full_output, _) =
                render_markdown_ratatui_full(text, test_style::STYLE, true, None);

            let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
            for chunk in &chunks {
                renderer.push_and_render(chunk, None);
                // push() now renders automatically
            }
            let streaming_output = renderer.view();

            let streaming_lines: Vec<String> = streaming_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            let full_lines: Vec<String> = full_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();

            assert_eq!(
                streaming_lines, full_lines,
                "Numbered list with chunk_size {}: mismatch\nStreaming: {:?}\nFull: {:?}",
                chunk_size, streaming_lines, full_lines
            );
        }
    }

    /// URL detection runs in BOTH `render()` and `finish()`.  This test
    /// pins:
    ///   1. The URL surfaces as a HyperlinkTarget during streaming
    ///      (every `push_and_render` call), not only after `finish()`.
    ///   2. `finish()` does not duplicate the URL HyperlinkTarget that
    ///      `render()` already added — the dedup in `detect_plain_urls`
    ///      makes the second pass idempotent.
    ///   3. The full `HyperlinkTarget` (URL + line_index + column_range)
    ///      is identical before and after `finish()`.  Ids may be
    ///      reassigned by `finish()` (it restarts the parser counter at
    ///      0), but every other field must match.
    #[test]
    fn streaming_byte_by_byte_url_appears_during_render_and_survives_finish() {
        let text = "See https://example.com for details.\n";

        // Stream byte-by-byte
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for byte in text.as_bytes() {
            let buf = [*byte];
            let s = std::str::from_utf8(&buf).expect("ascii test input");
            renderer.push_and_render(s, None);
        }

        // Snapshot the URL HyperlinkTarget before finish().
        let before: Vec<_> = renderer
            .view()
            .hyperlinks
            .iter()
            .filter(|h| h.url == "https://example.com")
            .map(|h| (h.url.clone(), h.line_index, h.column_range.clone()))
            .collect();
        assert_eq!(
            before.len(),
            1,
            "render() must surface the plain URL exactly once before finish(); \
             got hyperlinks: {:?}",
            renderer.view().hyperlinks,
        );

        // After finish: the URL must still be present, with the same
        // (URL, line_index, column_range), and no duplicates.
        renderer.finish(None);
        let after: Vec<_> = renderer
            .view()
            .hyperlinks
            .iter()
            .filter(|h| h.url == "https://example.com")
            .map(|h| (h.url.clone(), h.line_index, h.column_range.clone()))
            .collect();
        assert_eq!(
            before, after,
            "finish() must preserve URL HyperlinkTargets added by render() \
             (location-stable, no duplicates)",
        );
    }

    /// URL split across two `push_and_render` boundaries: the renderer
    /// must produce a single full-URL HyperlinkTarget after both chunks,
    /// not a stale partial-URL target left over from the first chunk.
    /// Regression guard against the dedup-overlap trap where a frozen
    /// partial-URL hyperlink would block detection of the full URL on
    /// the next render.
    #[test]
    fn streaming_url_split_across_chunks_produces_single_full_target() {
        let part1 = "[link](https://exam";
        let part2 = "ple.com/some/path)\n";

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render(part1, None);
        renderer.push_and_render(part2, None);
        let view = renderer.view();

        let full_url = "https://example.com/some/path";
        let matches: Vec<&HyperlinkTarget> = view
            .hyperlinks
            .iter()
            .filter(|h| h.url == full_url)
            .collect();
        // Pretty-mode `[link](url)` produces two HyperlinkTargets pointing
        // at the same URL: the parser one over "link" and the url_scan one
        // over the `(url)` suffix.  Pinning the EXACT count guards against
        // (a) a parser hyperlink dropped on the chunk-boundary, leaving
        // only url_scan's; and (b) url_scan adding a duplicate.
        assert_eq!(
            matches.len(),
            2,
            "the full URL must be present as exactly the parser + url_scan \
             HyperlinkTargets; got hyperlinks: {:?}",
            view.hyperlinks,
        );
        // The two ranges must be disjoint — the parser one covers "link",
        // the url_scan one covers the URL in the `(url)` suffix.
        let (a, b) = (&matches[0], &matches[1]);
        assert!(
            a.column_range.end <= b.column_range.start
                || b.column_range.end <= a.column_range.start,
            "the parser and url_scan ranges for the same URL must be disjoint; \
             got {:?} and {:?}",
            a.column_range,
            b.column_range,
        );
        // No partial-URL hyperlink should survive.
        let stale: Vec<&HyperlinkTarget> = view
            .hyperlinks
            .iter()
            .filter(|h| h.url.starts_with("https://exam") && h.url != full_url)
            .collect();
        assert!(
            stale.is_empty(),
            "no partial-URL HyperlinkTargets must linger across chunks; got {:?}",
            stale,
        );
    }

    /// Idempotency: repeated `render()` calls with no source change must
    /// produce the same `view().hyperlinks` — same URLs, line indices,
    /// column ranges, AND ids.  Without dedup, each call would re-add
    /// the url_scan results; without deterministic id assignment, ids
    /// would drift between calls and break OSC 8 grouping continuity.
    #[test]
    fn back_to_back_render_calls_are_idempotent() {
        let text = "See https://example.com and [link](https://other.example).\n";
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render(text, None);

        // Snap includes `id`: a regression where ids drift across renders
        // (e.g. a non-reset global counter) would fail here.  Since the
        // source is unchanged, the parser counter restarts at the same
        // `frozen.next_link_id` and the url_scan counter also resumes at
        // a stable value — every field of every hyperlink must match.
        let snap =
            |r: &StreamingMarkdownRenderer| -> Vec<(u32, String, usize, std::ops::Range<usize>)> {
                r.view()
                    .hyperlinks
                    .iter()
                    .map(|h| (h.id, h.url.clone(), h.line_index, h.column_range.clone()))
                    .collect()
            };
        let s1 = snap(&renderer);
        renderer.render(None);
        let s2 = snap(&renderer);
        renderer.render(None);
        let s3 = snap(&renderer);

        assert_eq!(s1, s2, "second render() must produce identical hyperlinks");
        assert_eq!(s2, s3, "third render() must produce identical hyperlinks");
    }

    /// Across multiple streaming chunks every emitted hyperlink id must
    /// be unique.  Catches a regression where url_scan reuses an id
    /// already assigned to a parser hyperlink (or vice versa), which
    /// would silently merge OSC 8 hyperlinks for the terminal.
    ///
    /// The first assertion is the regression target: it runs on the
    /// streaming-path output (post-`rerender_tail`, pre-`finish()`).  A
    /// regression in the `post_scan_next_id` vs `tail_next_link_id`
    /// bookkeeping (production change at the bottom of `rerender_tail`)
    /// would surface here.  We keep a second post-`finish()` assertion
    /// because `finish()`'s full re-render is the recovery path that
    /// users always see eventually — both must produce unique ids.
    #[test]
    fn render_assigns_monotonic_ids_across_chunks() {
        let chunks = [
            "Para one: [a](https://a.example).\n\n",
            "Para two: see https://b.example here.\n\n",
            "Para three: [c](https://c.example) end.\n",
        ];
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for chunk in &chunks {
            renderer.push_and_render(chunk, None);
        }

        // Pre-finish assertion: this is the regression target. `finish()`
        // restarts the parser counter at 0 and re-numbers everything, so
        // a `rerender_tail` ID-counter regression would NOT surface
        // after `finish()` — only here.
        let pre_finish_view = renderer.view();
        let pre_finish_ids: std::collections::HashSet<u32> =
            pre_finish_view.hyperlinks.iter().map(|h| h.id).collect();
        assert_eq!(
            pre_finish_ids.len(),
            pre_finish_view.hyperlinks.len(),
            "every hyperlink id must be unique BEFORE finish() (streaming path); \
             got: {:?}",
            pre_finish_view.hyperlinks,
        );

        // Secondary post-finish assertion: full-render path must also
        // produce unique ids.
        renderer.finish(None);
        let view = renderer.view();
        let ids: std::collections::HashSet<u32> = view.hyperlinks.iter().map(|h| h.id).collect();
        assert_eq!(
            ids.len(),
            view.hyperlinks.len(),
            "every hyperlink id must be unique after finish(); got: {:?}",
            view.hyperlinks,
        );
    }

    /// Pin: byte-by-byte streaming through a checkpoint advance with a
    /// URL straddling the freeze boundary must produce a single
    /// full-URL hyperlink (not a stuck partial-URL one left over from
    /// when only the prefix was visible).
    ///
    /// The PRE-finish assertion is the regression target: the
    /// concern is a partial-URL hyperlink left in the streaming-path
    /// `view().hyperlinks`.  `finish()` does a full re-render and would
    /// recover from any stuck state, masking the regression — so the
    /// streaming-path snapshot is taken first and the assertion runs on
    /// it directly.  The post-finish assertion is retained as a
    /// secondary check.
    #[test]
    fn streaming_byte_by_byte_through_checkpoint_with_url_at_boundary() {
        let text = "# Header\n\nSee https://example.com/path here.\n\n";
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for byte in text.as_bytes() {
            let buf = [*byte];
            let s = std::str::from_utf8(&buf).expect("ascii test input");
            renderer.push_and_render(s, None);
        }

        // Closure: assert the URL is fully present and no partial-URL
        // hyperlinks linger.  Used for both the pre- and post-finish
        // snapshots so they exercise the identical invariant.
        let assert_clean = |view: &MarkdownRenderView<'_>, when: &str| {
            let full_url_count = view
                .hyperlinks
                .iter()
                .filter(|h| h.url == "https://example.com/path")
                .count();
            assert_eq!(
                full_url_count, 1,
                "{when}: the full URL must be exactly one HyperlinkTarget; got: {:?}",
                view.hyperlinks,
            );
            let stale: Vec<&HyperlinkTarget> = view
                .hyperlinks
                .iter()
                .filter(|h| h.url.starts_with("https://") && h.url != "https://example.com/path")
                .collect();
            assert!(
                stale.is_empty(),
                "{when}: no partial-URL HyperlinkTargets must remain; got {:?}",
                stale,
            );
        };

        // Pre-finish: the regression target.
        let pre_finish_view = renderer.view();
        assert_clean(&pre_finish_view, "before finish()");

        // Post-finish: secondary check.
        renderer.finish(None);
        let view = renderer.view();
        assert_clean(&view, "after finish()");
    }

    /// A document with one markdown link `[a](url)` and one plain URL
    /// `https://b.example` after `finish` should have monotonic IDs:
    /// the markdown-link target with `id = 0`, and the plain-URL target
    /// with a higher id (continuing from `frozen.next_link_id`).
    ///
    /// NOTE: one might expect `id = 1` for the plain URL.
    /// In pretty mode, `[a](url)` renders as `a (url)`, so the url_scan
    /// pass assigns id=1 to the pretty-mode suffix first, pushing the
    /// plain URL to id=2.
    #[test]
    fn url_scan_ids_continue_from_frozen_counter() {
        let text = "[a](https://a.example) and https://b.example\n";

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render(text, None);
        let view = renderer.finish(None);
        let hyperlinks = view.hyperlinks;

        assert!(
            hyperlinks.len() >= 2,
            "expected at least 2 hyperlinks, got {}",
            hyperlinks.len()
        );

        let md_link = hyperlinks
            .iter()
            .find(|h| h.url == "https://a.example")
            .expect("markdown link target should exist");
        let plain_url = hyperlinks
            .iter()
            .find(|h| h.url == "https://b.example")
            .expect("plain URL target should exist");

        assert_eq!(md_link.id, 0, "markdown link should have id=0");
        // id=1 is taken by the pretty-mode URL suffix for `(https://a.example)`,
        // so the plain URL gets id=2.
        assert_eq!(
            plain_url.id, 2,
            "plain URL gets id=2 (parser id=0, pretty-mode suffix id=1)"
        );
    }

    /// Helper: split text into chunks of approximately `chunk_size` bytes.
    fn split_into_chunks(text: &str, chunk_size: usize) -> Vec<&str> {
        let mut chunks = Vec::new();
        let bytes = text.as_bytes();
        let mut start = 0;

        while start < bytes.len() {
            let end = (start + chunk_size).min(bytes.len());
            // Don't split in the middle of a UTF-8 char
            let mut actual_end = end;
            while actual_end > start && !text.is_char_boundary(actual_end) {
                actual_end -= 1;
            }
            if actual_end == start {
                actual_end = end;
                while actual_end < bytes.len() && !text.is_char_boundary(actual_end) {
                    actual_end += 1;
                }
            }
            chunks.push(&text[start..actual_end]);
            start = actual_end;
        }

        chunks
    }

    #[test]
    fn test_malformed_table_wraps_per_row() {
        // 11-column header but 12-cell delimiter — pulldown-cmark rejects
        // as a table.  Each pipe-prefixed row must stay on its own line
        // (soft break NOT collapsed) so the TUI can wrap it.
        let text = "\
| ColA | ColB | ColC | ColD | ColE | ColF | ColG | ColH | ColI | ColJ | ColK |
|---|---|---|---|---|---|---|---|---|---|---|------------------------------------|\n\
| A001 | data | more | 2026-01-01 | 1.00 | 0.0 | 20.0 | 10.0 | 50.00 | left | 1 |

";
        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        // Must NOT collapse into a single line — each source row should
        // produce its own output line.
        assert!(
            full_output.lines.len() >= 3,
            "malformed table rows must not collapse to 1 line, got {} lines",
            full_output.lines.len()
        );
        // Streaming must match.
        assert_streaming_matches_full_both(text);
    }

    // ----------------------------------------------------------------------
    // Syntect-enabled streaming equivalence (incremental open-code highlighter)
    // ----------------------------------------------------------------------

    /// Build a nested YAML body of at least `num_lines` lines (no fences).
    fn yaml_body(num_lines: usize) -> String {
        let mut out = String::new();
        let mut i = 0usize;
        let mut lines = 0usize;
        while lines < num_lines {
            for line in [
                format!("service_{i}:"),
                format!("  name: \"svc-{i}\""),
                "  enabled: true".to_string(),
                format!("  replicas: {}", i % 7 + 1),
                "  env:".to_string(),
                "    - name: LOG_LEVEL".to_string(),
                format!(
                    "      value: \"{}\"",
                    if i % 2 == 0 { "info" } else { "debug" }
                ),
                "  ports:".to_string(),
                format!("    - {}", 8000 + i),
            ] {
                out.push_str(&line);
                out.push('\n');
                lines += 1;
            }
            i += 1;
        }
        out
    }

    /// Stream `text` in `chunk`-byte pieces (char-boundary aware), rendering
    /// after every chunk so the incremental open-code cache is exercised, then
    /// assert the final view matches a one-shot full render byte-for-byte
    /// (both `lines` and `line_source_map`).
    #[track_caller]
    fn assert_streaming_matches_full_syntect(text: &str, pretty: bool, chunk: usize) {
        let syntect = crate::syntax::test_syntect();
        let (full_output, _) =
            render_markdown_ratatui_full(text, test_style::STYLE, pretty, Some(syntect));

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, pretty);
        let mut pos = 0;
        while pos < text.len() {
            let desired = pos + chunk;
            let end = text[pos..]
                .char_indices()
                .map(|(i, _)| pos + i)
                .find(|&i| i >= desired)
                .unwrap_or(text.len());
            renderer.push_and_render(&text[pos..end], Some(syntect));
            pos = end;
        }
        let streaming_output = renderer.view();

        assert_eq!(
            streaming_output.lines,
            full_output.lines.as_slice(),
            "[chunk={chunk}] syntect streaming lines mismatch",
        );
        assert_eq!(
            streaming_output.line_source_map, full_output.line_source_map,
            "[chunk={chunk}] syntect streaming line_source_map mismatch",
        );
    }

    #[test]
    fn test_open_yaml_block_streaming_matches_full_char_by_char() {
        // An UNCLOSED ```yaml block: the streaming renderer keeps it in the
        // tail and highlights it incrementally; full render highlights it from
        // scratch. They must be byte-identical.
        let text = format!("```yaml\n{}", yaml_body(120));
        assert_streaming_matches_full_syntect(&text, true, 1);
    }

    #[test]
    fn test_open_yaml_block_streaming_matches_full_chunks() {
        let text = format!("```yaml\n{}", yaml_body(120));
        for chunk in [3, 7, 17, 64] {
            assert_streaming_matches_full_syntect(&text, true, chunk);
        }
    }

    /// Like [`assert_streaming_matches_full_syntect`] but compares only
    /// `lines` (the highlighted content). Used where the stream crosses a
    /// checkpoint/freeze boundary: `line_source_map` is tail-relative across
    /// freezes (pre-existing streaming behavior, unrelated to highlighting),
    /// so only the rendered content is asserted equal.
    #[track_caller]
    fn assert_streaming_lines_match_full_syntect(text: &str, pretty: bool, chunk: usize) {
        let syntect = crate::syntax::test_syntect();
        let (full_output, _) =
            render_markdown_ratatui_full(text, test_style::STYLE, pretty, Some(syntect));

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, pretty);
        let mut pos = 0;
        while pos < text.len() {
            let desired = pos + chunk;
            let end = text[pos..]
                .char_indices()
                .map(|(i, _)| pos + i)
                .find(|&i| i >= desired)
                .unwrap_or(text.len());
            renderer.push_and_render(&text[pos..end], Some(syntect));
            pos = end;
        }
        assert_eq!(
            renderer.view().lines,
            full_output.lines.as_slice(),
            "[chunk={chunk}] syntect streaming lines mismatch",
        );
    }

    #[test]
    fn test_closed_yaml_block_streaming_matches_full() {
        // A CLOSED block plus following prose: the closed block never uses the
        // incremental cache (the trailing-open branch requires the body to
        // reach EOF), so its highlighted output must equal the batch path.
        let text = format!("```yaml\n{}```\n\nDone.\n\n", yaml_body(80));
        assert_streaming_lines_match_full_syntect(&text, true, 1);
        assert_streaming_lines_match_full_syntect(&text, true, 11);
    }

    #[test]
    fn test_second_open_block_after_closed_resets_cache() {
        // A closed rust block, then a still-open yaml block. The cache must
        // re-key on the new fence/offset and produce output identical to full.
        let text = format!("```rust\nfn main() {{}}\n```\n\n```yaml\n{}", yaml_body(60));
        assert_streaming_lines_match_full_syntect(&text, true, 1);
        assert_streaming_lines_match_full_syntect(&text, true, 9);
    }

    #[test]
    fn test_closed_fences_inside_open_list_match_full() {
        // Shape: closed fences in a list that keeps streaming. The
        // open list blocks checkpointing, so every push re-parses the fences
        // via `highlight_closed`; output must match a one-shot full render.
        let mut text = String::new();
        for i in 0..2 {
            text.push_str(&format!(
                "- **item {i}**\n  ```rust\n  fn f{i}(x: u64) -> u64 {{\n      x + {i}\n  }}\n  ```\n",
            ));
        }
        for w in 0..30 {
            if w % 6 == 0 {
                text.push_str("\n- more: ");
            }
            text.push_str(&format!("word{w} "));
        }
        text.push('\n');
        for chunk in [1, 7, 23] {
            assert_streaming_lines_match_full_syntect(&text, true, chunk);
        }
    }

    #[test]
    fn test_closed_fence_in_list_then_open_fence_match_full() {
        // Memo path (closed fence in open list) and incremental path
        // (trailing open fence) active simultaneously must not disturb
        // each other.
        let text = format!(
            "- **pinned**\n  ```rust\n  fn pinned() -> u64 {{ 7 }}\n  ```\n- streaming on\n\n```yaml\n{}",
            yaml_body(40),
        );
        for chunk in [1, 9] {
            assert_streaming_lines_match_full_syntect(&text, true, chunk);
        }
    }

    #[test]
    fn test_open_block_utf8_split_across_chunks() {
        // Multibyte chars in the still-streaming last line, split across chunk
        // boundaries, must not panic and must match full render.
        let text = "```yaml\nname: \"café — naïve 日本語 🎉 résumé\"\nother: 1\n".to_string();
        for chunk in [1, 2, 3, 5] {
            assert_streaming_matches_full_syntect(&text, true, chunk);
        }
    }

    #[test]
    fn test_open_block_crlf_line_endings_match_full() {
        // CRLF line endings inside the open block: `LinesWithEndings` keeps the
        // `\r\n` on the committed line, so incremental == batch. (Single open
        // block, no freeze, so line_source_map is asserted too.)
        let mut text = String::from("```yaml\r\n");
        for line in yaml_body(40).lines() {
            text.push_str(line);
            text.push_str("\r\n");
        }
        for chunk in [1, 3, 7, 17] {
            assert_streaming_matches_full_syntect(&text, true, chunk);
        }
    }

    #[test]
    fn test_theme_change_mid_stream_clears_cache() {
        let syntect = crate::syntax::test_syntect();
        // A second, distinct style (changes code_untagged so a difference would
        // be observable if the cache were not cleared).
        let mut style2 = test_style::STYLE;
        style2.code_untagged = anstyle::Style::new().bold();

        let text = format!("```yaml\n{}", yaml_body(40));

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        // Stream the first half with the original style.
        let mid = text.len() / 2;
        let mid = text
            .char_indices()
            .map(|(i, _)| i)
            .find(|&i| i >= mid)
            .unwrap_or(text.len());
        renderer.push_and_render(&text[..mid], Some(syntect));
        assert!(
            renderer.open_code.is_some(),
            "cache should exist after rendering an open block with syntect",
        );

        // Theme change must drop the cache.
        renderer.set_style(style2);
        assert!(
            renderer.open_code.is_none(),
            "set_style must clear the open-code cache",
        );

        // Finish streaming under the new style and compare to a full render
        // under that same style.
        renderer.push_and_render(&text[mid..], Some(syntect));
        let (full_output, _) = render_markdown_ratatui_full(&text, style2, true, Some(syntect));
        assert_eq!(
            renderer.view().lines,
            full_output.lines.as_slice(),
            "post-theme-change streaming must match full render with new style",
        );
    }

    /// Document exercising every math delimiter form, used to verify the
    /// streaming renderer converges to the full render no matter where
    /// chunk boundaries fall.
    const MATH_DOC: &str = concat!(
        "# Math test\n\n",
        "Euler: $e^{i\\pi} + 1 = 0$ inline.\n\n",
        "Display:\n\n",
        "$$\n\\int_0^\\infty e^{-x} dx = 1\n$$\n\n",
        "Paren \\(\\alpha + \\beta\\) inline.\n\n",
        "Padded \\( u + v \\) inline.\n\n",
        "\\[\n\\frac{a+b}{2} \\ge \\sqrt{ab}\n\\]\n\n",
        "| Col | Math |\n|-----|------|\n| a | $x^2$ |\n\n",
        "- item \\(p \\to q\\)\n",
        "- plain\n\n",
        "> quote $$E = mc^2$$\n\n",
        "## Heading \\[h = x^3\\]\n\n",
        "Aligned:\n\n",
        "\\[\n\\begin{aligned}\nf(x) &= x^2 \\\\\ng(x) &= 2x\n\\end{aligned}\n\\]\n\n",
        "The end.\n",
    );
