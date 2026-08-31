    use super::*;
    use theway_transport::feed::{Feed, WireFeedBlock};
    use unicode_width::UnicodeWidthStr;

    fn feed_with(blocks: &[WireFeedBlock]) -> Feed {
        let mut feed = Feed::new();
        feed.replace_blocks(blocks);
        feed
    }

    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn user_block_has_accent_prefix_and_band() {
        let feed = feed_with(&[WireFeedBlock::User {
            text: "hello world".into(),
            timestamp: Some("2026-01-01 12:00".into()),
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 30, &opts);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans[0].content, "\u{276f} ");
        assert_eq!(spans[0].style.fg, Some(ACCENT_USER));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[1].content, "hello world");
        // Trailing band span pads the row to full width.
        let total: usize = spans.iter().map(|s| s.content.width()).sum();
        assert_eq!(total, 30);
        assert_eq!(spans[2].style.bg, Some(BG_HIGHLIGHT));
        // Timestamps are dropped from conversational blocks.
        assert!(!flat(&lines).contains("2026-01-01"));
    }

    #[test]
    fn user_block_wraps_with_indent_and_band() {
        let feed = feed_with(&[WireFeedBlock::User {
            text: "one two three four five six seven".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 12, &opts);
        assert!(lines.len() >= 2, "expected wrap: {lines:?}");
        // Continuation rows keep the band width.
        for line in &lines {
            let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
            assert_eq!(total, 12);
        }
    }

    #[test]
    fn user_block_band_bg_overridable_per_block() {
        let feed = feed_with(&[WireFeedBlock::User {
            text: "hello".into(),
            timestamp: None,
        }]);
        let mut opts = FeedRenderOptions::default();
        let custom = Color::Rgb(36, 40, 59);
        opts.theme.user.bg = Some(custom);
        let lines = super::lines(&feed, 30, &opts);
        let spans = &lines[0].spans;
        assert_eq!(spans[0].content, "\u{276f} ");
        assert_eq!(spans[1].content, "hello");
        // The trailing band span takes the per-block bg, not the role color.
        assert_eq!(spans[2].style.bg, Some(custom));

        // Without the per-block override the role color wins.
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 30, &opts);
        assert_eq!(lines[0].spans[2].style.bg, Some(BG_HIGHLIGHT));
    }

    #[test]
    fn thinking_modes_full_peek_hidden() {
        let blocks = vec![
            WireFeedBlock::User {
                text: "go".into(),
                timestamp: None,
            },
            WireFeedBlock::Thinking {
                text: "deep thoughts about the plan".into(),
                timestamp: None,
            },
        ];
        let opts = |mode| FeedRenderOptions {
            thinking_mode: mode,
            tools_expanded: false,
            ..Default::default()
        };
        let full = flat(&super::lines(
            &feed_with(&blocks),
            80,
            &opts(ThinkingMode::Full),
        ));
        assert!(full.contains("deep thoughts"), "{full}");
        let peek = flat(&super::lines(
            &feed_with(&blocks),
            80,
            &opts(ThinkingMode::Peek),
        ));
        assert!(peek.contains("⏵ thinking · 28 char"), "{peek}");
        assert!(peek.contains("c/s: 0 · in: 0 · out: 0"), "{peek}");
        assert!(peek.contains("deep thoughts"), "{peek}");
        let hidden = flat(&super::lines(
            &feed_with(&blocks),
            80,
            &opts(ThinkingMode::Hidden),
        ));
        assert!(!hidden.contains("deep thoughts"), "{hidden}");
        assert!(hidden.contains("❯ go"), "{hidden}");
    }

    #[test]
    fn thinking_peek_windows_tail_lines() {
        let text = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let feed = feed_with(&[WireFeedBlock::Thinking {
            text,
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Peek,
            tools_expanded: false,
            ..Default::default()
        };
        let lines = super::lines(&feed, 80, &opts);
        // Header + 3 peek rows + mode hint.
        assert!(lines.len() <= 1 + THINKING_PEEK_LINES + 1, "{lines:?}");
        let flat = flat(&lines);
        assert!(flat.contains("line 27"), "{flat}");
        assert!(flat.contains("line 29"), "{flat}");
        assert!(!flat.contains("line 0"), "{flat}");
    }

    #[test]
    fn tool_call_is_single_accent_line_without_timestamp() {
        let feed = feed_with(&[WireFeedBlock::ToolCall {
            name: "read".into(),
            args: "(path=\"x\")".into(),
            metadata: None,
            timestamp: Some("2026-01-01 12:00".into()),
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].content, "\u{23f5} read");
        assert_eq!(lines[0].spans[0].style.fg, Some(ACCENT_TOOL));
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(lines[0].spans[1].content.contains("(path=\"x\")"));
        assert!(!flat(&lines).contains("2026-01-01"));
    }

    #[test]
    fn tool_result_collapses_to_preview_and_expands() {
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines: vec!["line a".into(), "line b".into()],
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        assert!(collapsed.contains("│ line a"), "{collapsed}");
        assert!(collapsed.contains("│ line b"), "{collapsed}");
        assert!(!collapsed.contains("more lines"), "{collapsed}");
        let expanded = flat(&super::lines(
            &feed,
            80,
            &FeedRenderOptions {
                thinking_mode: ThinkingMode::Full,
                tools_expanded: true,
                ..Default::default()
            },
        ));
        assert!(expanded.contains("    line a"), "{expanded}");
        assert!(expanded.contains("    line b"), "{expanded}");
    }

    #[test]
    fn tool_result_preview_folds_to_five_lines_with_elision() {
        let lines: Vec<String> = (0..8).map(|i| format!("row {i}")).collect();
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines,
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        // The first 5 source lines render, each behind the left border.
        for i in 0..5 {
            assert!(collapsed.contains(&format!("│ row {i}")), "{collapsed}");
        }
        // Lines beyond the preview window are folded away.
        assert!(!collapsed.contains("row 5"), "{collapsed}");
        assert!(!collapsed.contains("row 7"), "{collapsed}");
        // The elision row carries the remaining line count.
        assert!(collapsed.contains("…(3 more lines)"), "{collapsed}");
    }

    #[test]
    fn tool_result_expanded_keeps_all_lines() {
        let lines: Vec<String> = (0..8).map(|i| format!("row {i}")).collect();
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines,
            is_error: false,
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            tools_expanded: true,
            ..Default::default()
        };
        let expanded = flat(&super::lines(&feed, 80, &opts));
        for i in 0..8 {
            assert!(expanded.contains(&format!("row {i}")), "{expanded}");
        }
        assert!(!expanded.contains("more lines"), "{expanded}");
    }

    /// Issue #41: an expanded tool result containing a ```mermaid fence
    /// renders the fenced body as box-drawing diagram art; non-fence lines
    /// keep the existing indented text rows.
    #[test]
    fn tool_result_mermaid_fence_renders_box_diagram() {
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines: vec![
                "before".into(),
                "```mermaid".into(),
                "graph TD".into(),
                "  A[Start] --> B[End]".into(),
                "```".into(),
                "after".into(),
            ],
            is_error: false,
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            tools_expanded: true,
            ..Default::default()
        };
        let expanded = flat(&super::lines(&feed, 80, &opts));
        assert!(
            expanded.chars().any(|c| "┌┐─".contains(c)),
            "expected mermaid box art: {expanded}"
        );
        // Non-fence lines stay as today (indented text rows).
        assert!(expanded.contains("    before"), "{expanded}");
        assert!(expanded.contains("    after"), "{expanded}");
        // The fence delimiters are consumed by the diagram.
        assert!(!expanded.contains("```"), "{expanded}");
    }

    /// Only the expanded branch detects fences: the collapsed preview keeps
    /// the classic text rendering (the fence stays visible behind the
    /// preview border).
    #[test]
    fn tool_result_collapsed_preview_keeps_fence_text() {
        let lines = vec![
            "```mermaid".into(),
            "graph TD".into(),
            "  A --> B".into(),
            "```".into(),
        ];
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines,
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        assert!(
            !collapsed.chars().any(|c| "┌┐─".contains(c)),
            "collapsed preview must not render the diagram: {collapsed}"
        );
        let expanded = flat(&super::lines(
            &feed,
            80,
            &FeedRenderOptions {
                tools_expanded: true,
                ..Default::default()
            },
        ));
        assert!(
            expanded.chars().any(|c| "┌┐─".contains(c)),
            "expanded fence must render the diagram: {expanded}"
        );
    }

    #[test]
    fn human_count_formats_raw_and_kilo() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(1000), "1.0k");
        assert_eq!(human_count(1200), "1.2k");
        assert_eq!(human_count(1234), "1.2k");
        assert_eq!(human_count(100_200), "100.2k");
    }

    #[test]
    fn thinking_hidden_renders_no_stats() {
        let feed = feed_with(&[WireFeedBlock::Thinking {
            text: "some thinking text".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Hidden,
            thinking_cps: 84.0,
            thinking_output_tokens: 1200,
            ..Default::default()
        };
        let flat = flat(&super::lines(&feed, 80, &opts));
        assert!(!flat.contains("c/s"), "{flat}");
        assert!(!flat.contains("thinking"), "{flat}");
        assert!(!flat.contains("some thinking text"), "{flat}");
    }

    #[test]
    fn thinking_stats_line_formats_cps_and_in_out_tokens() {
        let feed = feed_with(&[WireFeedBlock::Thinking {
            text: "x".repeat(1200),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Full,
            thinking_cps: 84.0,
            thinking_input_tokens: 57_100,
            thinking_output_tokens: 1_200,
            ..Default::default()
        };
        let flat = flat(&super::lines(&feed, 80, &opts));
        assert!(flat.contains("⏵ thinking · 1.2k char"), "{flat}");
        assert!(flat.contains("c/s: 84 · in: 57.1k · out: 1.2k"), "{flat}");
    }

    #[test]
    fn feed_render_options_defaults() {
        let opts = FeedRenderOptions::default();
        assert_eq!(opts.thinking_mode, ThinkingMode::default());
        assert!(!opts.tools_expanded);
        assert_eq!(opts.color_level, theway_markdown::ColorLevel::TrueColor);
        assert_eq!(opts.thinking_cps, 0.0);
        assert_eq!(opts.thinking_input_tokens, 0);
        assert_eq!(opts.thinking_output_tokens, 0);
        assert_eq!(opts.spinner_phase, 0);
    }

    /// `PartialEq` is hand-implemented (issue #44): the per-frame counters
    /// (cps / in / out / spinner_phase) must NOT participate, otherwise the
    /// feed cache invalidates and fully re-renders every frame; structural
    /// switches (thinking_mode / tools_expanded / color_level / theme) must.
    #[test]
    fn feed_render_options_equality_ignores_per_frame_counters() {
        let structural = FeedRenderOptions::default();
        let mut per_frame = FeedRenderOptions::default();
        per_frame.thinking_cps = 999.5;
        per_frame.thinking_input_tokens = 57_100;
        per_frame.thinking_output_tokens = 1_200;
        per_frame.spinner_phase = 42;
        assert_eq!(
            structural, per_frame,
            "cps/in/out/spinner_phase changes must keep options equal"
        );
        per_frame.thinking_mode = ThinkingMode::Peek;
        assert_ne!(
            structural, per_frame,
            "thinking_mode change must change equality"
        );
        per_frame = FeedRenderOptions::default();
        per_frame.tools_expanded = true;
        assert_ne!(
            structural, per_frame,
            "tools_expanded change must change equality"
        );
        per_frame = FeedRenderOptions::default();
        per_frame.color_level = theway_markdown::ColorLevel::None;
        assert_ne!(
            structural, per_frame,
            "color capability change must change equality"
        );
        per_frame = FeedRenderOptions::default();
        per_frame.theme.tool_title = Color::Rgb(1, 2, 3);
        assert_ne!(
            structural, per_frame,
            "theme change must change equality (issue #49: the cache fingerprints colors/layout)"
        );
    }

    /// Custom theme block layout (issue #49): the tool row carries the
    /// `tool_running_bg` background across the FULL block width with the
    /// configured padding columns, right-aligned.
    #[test]
    fn custom_theme_paints_tool_row_bg_padding_and_right_align() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool_running_bg = Some(Color::Rgb(1, 2, 3));
        theme.tool.padding = 2;
        theme.tool.align = crate::ui::theme::BlockAlign::Right;
        let opts = FeedRenderOptions {
            theme,
            ..Default::default()
        };
        let feed = feed_with(&[WireFeedBlock::ToolCall {
            name: "read".into(),
            args: String::new(),
            metadata: None,
            timestamp: None,
        }]);
        let lines = super::lines(&feed, 20, &opts);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.trim(), "⏵ read", "{text}");
        // Right padding columns: content ends 2 columns before the edge.
        assert!(text.ends_with("  "), "{text}");
        // Every span (content + padding) carries the background.
        for span in &lines[0].spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(1, 2, 3)));
        }
        // The row fills the whole block width.
        let total: usize = lines[0]
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, 20);
    }

    /// Custom theme thinking layout (issue #49): stats line AND body rows get
    /// the `thinking_bg` background at full width; an empty result row renders
    /// as pure background.
    #[test]
    fn custom_theme_paints_thinking_and_empty_result_rows() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.thinking_bg = Some(Color::Rgb(4, 5, 6));
        theme.thinking.padding = 1;
        theme.thinking.align = crate::ui::theme::BlockAlign::Left;
        theme.tool_success_bg = Some(Color::Rgb(7, 8, 9));
        let opts = FeedRenderOptions {
            theme,
            tools_expanded: true,
            ..Default::default()
        };
        let feed = feed_with(&[
            WireFeedBlock::Thinking {
                text: "ponder".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["row".into(), String::new()],
                is_error: false,
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 20, &opts);
        let flat = flat(&lines);
        // Thinking stats row + body row both painted at full width.
        let stats = &lines[0];
        assert!(flat.contains("⏵ thinking"), "{flat}");
        for span in &stats.spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(4, 5, 6)));
        }
        let total: usize = stats
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, 20, "stats row must span the full block width");
        let body = &lines[1];
        for span in &body.spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(4, 5, 6)));
        }
        // The expanded empty result line renders as a pure-background row.
        // Layout: [separator? no — blocks 0..1 thinking, then result rows]
        // Find the empty result row: after the thinking rows + separator.
        let empty = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .all(|s| s.content.chars().all(|c| c == ' '))
                    && !line.spans.is_empty()
            })
            .expect("empty result row missing");
        for span in &empty.spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(7, 8, 9)));
        }
        let total: usize = empty
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, 20, "empty row must be pure background at full width");
    }

    /// Theme role colors (issue #43): tool result / error colors flow from
    /// the theme; the default theme equals the pre-theme consts.
    #[test]
    fn custom_theme_recolors_tool_result_and_error() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool_result = Color::Rgb(1, 2, 3);
        theme.tool_error = Color::Rgb(4, 5, 6);
        let opts = FeedRenderOptions {
            theme,
            tools_expanded: true,
            ..Default::default()
        };
        let feed = feed_with(&[
            WireFeedBlock::ToolResult {
                lines: vec!["ok".into()],
                is_error: false,
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["bad".into()],
                is_error: true,
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 80, &opts);
        let ok = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("ok")))
            .unwrap();
        let bad = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("bad")))
            .unwrap();
        // `Line::styled` puts the fg on the line itself (span styles stay
        // default) — assert the effective line style.
        assert_eq!(ok.style.fg, Some(Color::Rgb(1, 2, 3)), "{ok:?}");
        assert_eq!(bad.style.fg, Some(Color::Rgb(4, 5, 6)), "{bad:?}");
    }

    /// Default theme renders byte-identical rows to the pre-theme consts:
    /// no background, no padding columns, flush left (issue #49).
    #[test]
    fn default_theme_keeps_classic_tool_and_thinking_rows() {
        let feed = feed_with(&[
            WireFeedBlock::ToolCall {
                name: "read".into(),
                args: "(path=\"x\")".into(),
            metadata: None,
                timestamp: None,
            },
            WireFeedBlock::Thinking {
                text: "ponder".into(),
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 20, &FeedRenderOptions::default());
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "⏵ read (path=\"x\")", "tool row must stay flush");
        assert!(lines[0].spans.iter().all(|s| s.style.bg.is_none()));
        let stats: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(stats.contains("⏵ thinking"), "{stats}");
        assert!(lines[1].spans.iter().all(|s| s.style.bg.is_none()));
    }

    #[test]
    fn mermaid_fence_renders_diagram_not_source() {
        let feed = feed_with(&[WireFeedBlock::Assistant {
            text: "```mermaid\ngraph TD\n  A[Start] --> B[End]\n```\n".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        let flat = flat(&lines);
        assert!(
            flat.chars().any(|c| "─│┌┐└┘├┤┬┴┼".contains(c)),
            "expected diagram art: {flat}"
        );
        assert!(!flat.contains("graph TD"), "{flat}");
    }

    #[test]
    fn assistant_response_has_no_role_prefix() {
        let feed = feed_with(&[WireFeedBlock::Assistant {
            text: "plain answer".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        assert_eq!(flat(&lines), "plain answer");
    }

    #[test]
    fn syntax_colors_follow_injected_capability() {
        let feed = feed_with(&[WireFeedBlock::Assistant {
            text: "```rust\nfn main() { let value = true; }\n```\n".into(),
            timestamp: None,
        }]);
        let mut opts = FeedRenderOptions::default();
        opts.theme.assistant_text = None;

        opts.color_level = theway_markdown::ColorLevel::None;
        let plain = super::lines(&feed, 80, &opts);
        assert!(
            plain
                .iter()
                .flat_map(|line| &line.spans)
                .all(|span| span.style.fg.is_none()),
            "no-color capability must remove syntax foregrounds: {plain:?}"
        );

        opts.color_level = theway_markdown::ColorLevel::TrueColor;
        let colored = super::lines(&feed, 80, &opts);
        assert!(
            colored
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.style.fg.is_some()),
            "truecolor capability must retain syntax foregrounds: {colored:?}"
        );
    }

    /// `wrap_str_ranges` must produce rows identical to the transport
    /// `wrap_str` (byte ranges re-slice the source to the same rows).
    #[test]
    fn wrap_str_ranges_matches_wrap_str() {
        let cases: Vec<String> = vec![
            String::new(),
            "hello".to_string(),
            "hello world".to_string(),
            "aa bb cc dd ee".to_string(),
            "word ".repeat(30),
            "https://example.com/very/long/path".to_string(),
            "  leading spaces preserved".to_string(),
            "mix of 中文 and ascii text".to_string(),
        ];
        for text in &cases {
            for width in [1usize, 2, 5, 8, 20, 80] {
                let expected = theway_transport::feed::wrap_str(text, width);
                let got: Vec<String> = wrap_str_ranges(text, width)
                    .into_iter()
                    .map(|row| text[row.range].to_string())
                    .collect();
                assert_eq!(got, expected, "text={text:?} width={width}");
            }
        }
    }

    // ── v2 feed rhythm (#30): [feed] gap / separator ──────────────────────

    #[test]
    fn feed_gap_controls_inter_block_spacing() {
        let feed = feed_with(&[
            WireFeedBlock::User {
                text: "hello".into(),
                timestamp: None,
            },
            WireFeedBlock::Assistant {
                text: "world".into(),
                timestamp: None,
            },
        ]);
        let opts = FeedRenderOptions::default();
        let text = flat(&super::lines(&feed, 30, &opts));
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[0].trim_end(), "\u{276f} hello");
        assert_eq!(rows[1], "", "default gap is one blank line");
        assert_eq!(rows[2], "world");

        let mut opts = FeedRenderOptions::default();
        opts.theme.feed.gap = 3;
        let text = flat(&super::lines(&feed, 30, &opts));
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "");
        assert_eq!(rows[3], "");
        assert_eq!(rows[4], "world");

        let mut opts = FeedRenderOptions::default();
        opts.theme.feed.gap = 0;
        let text = flat(&super::lines(&feed, 30, &opts));
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], "world", "gap 0 renders flush");
    }

    #[test]
    fn feed_separator_renders_full_width_styled_line() {
        let feed = feed_with(&[
            WireFeedBlock::User {
                text: "hello".into(),
                timestamp: None,
            },
            WireFeedBlock::Assistant {
                text: "world".into(),
                timestamp: None,
            },
        ]);
        let mut opts = FeedRenderOptions::default();
        opts.theme.feed.gap = 0;
        opts.theme.feed.separator = Some('─');
        opts.theme.feed.separator_style = ratatui::style::Color::Rgb(0x56, 0x5F, 0x89);
        let lines = super::lines(&feed, 30, &opts);
        let text = flat(&lines);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], "─".repeat(30), "separator spans the full width");
        assert_eq!(rows[1].chars().count(), 30);
        // The separator line carries the configured style.
        let sep_line = &lines[1];
        assert_eq!(sep_line.spans[0].style.fg, Some(ratatui::style::Color::Rgb(0x56, 0x5F, 0x89)));
    }

    // ── v2 block frame (#31): margins + borders ──────────────────────────

    fn tool_block() -> Feed {
        feed_with(&[WireFeedBlock::ToolCall {
            name: "bash".into(),
            args: " ls".into(),
            metadata: None,
            timestamp: None,
        }])
    }

    #[test]
    fn block_margins_add_blank_rows_around_content() {
        let feed = tool_block();
        let mut opts = FeedRenderOptions::default();
        opts.theme.tool.margin_top = 1;
        opts.theme.tool.margin_bottom = 2;
        let text = flat(&super::lines(&feed, 30, &opts));
        // split (not lines): lines() drops trailing empty rows.
        let rows: Vec<&str> = text.split('\n').collect();
        assert_eq!(rows[0], "", "margin_top blank row");
        assert!(rows[1].contains("bash"), "content row");
        assert_eq!(rows[2], "", "margin_bottom row 1");
        assert_eq!(rows[3], "", "margin_bottom row 2");
    }

    #[test]
    fn block_borders_render_thin_and_thick_lines() {
        let feed = tool_block();
        let mut opts = FeedRenderOptions::default();
        opts.theme.tool.margin_top = 1;
        opts.theme.tool.border_top = crate::ui::theme::BlockBorder::Thin;
        opts.theme.tool.border_bottom = crate::ui::theme::BlockBorder::Thick;
        opts.theme.tool.border_style = ratatui::style::Color::Rgb(1, 2, 3);
        let lines = super::lines(&feed, 30, &opts);
        let text = flat(&lines);
        let rows: Vec<&str> = text.lines().collect();
        // Order: margin, border-top, content, border-bottom.
        assert_eq!(rows[0], "", "margin row");
        assert_eq!(rows[1], "─".repeat(30), "thin top border");
        assert!(rows[2].contains("bash"), "content");
        assert_eq!(rows[3], "━".repeat(30), "thick bottom border");
        // Border lines carry border_style.
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(ratatui::style::Color::Rgb(1, 2, 3))
        );
    }

    #[test]
    fn block_frame_composes_with_feed_gap() {
        let feed = feed_with(&[
            WireFeedBlock::User {
                text: "hello".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolCall {
                name: "bash".into(),
                args: " ls".into(),
            metadata: None,
                timestamp: None,
            },
        ]);
        let mut opts = FeedRenderOptions::default();
        opts.theme.tool.margin_top = 1;
        opts.theme.tool.border_top = crate::ui::theme::BlockBorder::Thin;
        let text = flat(&super::lines(&feed, 30, &opts));
        let rows: Vec<&str> = text.lines().collect();
        // User row, [feed] gap (1 blank), then the tool frame: margin, border, content.
        assert!(rows[0].contains("hello"));
        assert_eq!(rows[1], "", "feed gap");
        assert_eq!(rows[2], "", "tool margin_top");
        assert_eq!(rows[3], "─".repeat(30), "tool border_top");
        assert!(rows[4].contains("bash"));
    }

    /// A tool call followed by its result renders as ONE tool area (issue
    /// #69): no internal feed gap between them even under `separate_all`,
    /// and the call row + result body share one background band with an
    /// optional internal divider (the call block's `border_bottom`).
    #[test]
    fn tool_pair_renders_as_one_area_with_internal_divider() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool.bg = Some(Color::Rgb(40, 40, 46)); // gray band
        theme.tool.border_bottom = crate::ui::theme::BlockBorder::Thin;
        let opts = FeedRenderOptions {
            theme,
            ..Default::default()
        };
        let feed = feed_with(&[
            WireFeedBlock::ToolCall {
                name: "read".into(),
                args: " /tmp/x.md".into(),
                metadata: None,
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["one".into(), "two".into()],
                is_error: false,
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 24, &opts);
        let text = flat(&lines);
        let rows: Vec<&str> = text.lines().collect();
        // call row, divider, then the two result rows — no blank row inside.
        assert!(rows[0].contains("⏵ read"), "{rows:?}");
        assert_eq!(rows[1], "─".repeat(24), "internal divider: {rows:?}");
        assert!(rows[2].contains("one"), "{rows:?}");
        assert!(rows[3].contains("two"), "{rows:?}");
        assert_eq!(rows.len(), 4, "no gap rows inside the area: {rows:?}");
        // The divider and every content row carry the shared band background.
        for line in &lines {
            for span in &line.spans {
                assert_eq!(span.style.bg, Some(Color::Rgb(40, 40, 46)));
            }
        }
    }

    /// Standalone tool call (result not yet arrived) renders alone with its
    /// own frame — the pair merge must not kick in.
    #[test]
    fn standalone_tool_call_keeps_single_block_frame() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool.bg = Some(Color::Rgb(40, 40, 46));
        theme.tool.border_bottom = crate::ui::theme::BlockBorder::Thin;
        let opts = FeedRenderOptions {
            theme,
            ..Default::default()
        };
        let feed = feed_with(&[WireFeedBlock::ToolCall {
            name: "read".into(),
            args: String::new(),
            metadata: None,
            timestamp: None,
        }]);
        let text = flat(&super::lines(&feed, 20, &opts));
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows.len(), 2, "call row + closing border: {rows:?}");
        assert!(rows[0].contains("⏵ read"));
        assert_eq!(rows[1], "─".repeat(20));
    }
