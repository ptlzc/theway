use super::*;

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
    let text = flat(&lines(&feed, 30, &opts));
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows[0].trim_end(), "\u{276f} hello");
    assert_eq!(rows[1], "", "default gap is one blank line");
    assert_eq!(rows[2], "world");

    let mut opts = FeedRenderOptions::default();
    opts.theme.feed.gap = 3;
    let text = flat(&lines(&feed, 30, &opts));
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows[1], "");
    assert_eq!(rows[2], "");
    assert_eq!(rows[3], "");
    assert_eq!(rows[4], "world");

    let mut opts = FeedRenderOptions::default();
    opts.theme.feed.gap = 0;
    let text = flat(&lines(&feed, 30, &opts));
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
    let lines = lines(&feed, 30, &opts);
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
    let text = flat(&lines(&feed, 30, &opts));
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
    let lines = lines(&feed, 30, &opts);
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
    let text = flat(&lines(&feed, 30, &opts));
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
    let lines = lines(&feed, 24, &opts);
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
    let text = flat(&lines(&feed, 20, &opts));
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 2, "call row + closing border: {rows:?}");
    assert!(rows[0].contains("⏵ read"));
    assert_eq!(rows[1], "─".repeat(20));
}
