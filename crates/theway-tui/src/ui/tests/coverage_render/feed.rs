use super::*;

#[test]
fn feed_window_selection_and_selection_text_edges() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    let lines = vec![Line::raw("hello"), Line::raw(""), Line::raw("world")];
    let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
    render_lines_window(
        &mut buf,
        Rect::new(0, 0, 20, 5),
        &lines,
        0,
        Some(crate::feed_render::TextSelection {
            start_line: 0,
            start_col: 1,
            end_line: 2,
            end_col: 3,
        }),
    );
    assert_eq!(
        buf.cell((1, 0)).unwrap().bg,
        Color::Rgb(75, 92, 140),
        "selection background"
    );
    assert_eq!(
        buf.cell((0, 2)).unwrap().bg,
        Color::Rgb(75, 92, 140),
        "selection background"
    );

    let text = selection_text(
        &lines,
        crate::feed_render::TextSelection {
            start_line: 0,
            start_col: 1,
            end_line: 2,
            end_col: 3,
        },
    );
    assert_eq!(text, "ello\n\nwor");

    assert_eq!(
        selection_text(
            &lines,
            crate::feed_render::TextSelection {
                start_line: 99,
                start_col: 0,
                end_line: 99,
                end_col: 0,
            },
        ),
        ""
    );
    assert_eq!(
        selection_text(
            &[],
            crate::feed_render::TextSelection {
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 0,
            },
        ),
        ""
    );
}

#[test]
fn feed_render_blocks_cover_plain_error_tool_thinking_assistant() {
    use theway_transport::feed::{Block, Level};

    let opts = FeedRenderOptions::default();
    let block = |text: &str| {
        render_block(
            &Block::Plain {
                text: text.into(),
                level: Level::Note,
                timestamp: Some("2026-01-01 12:00".into()),
            },
            40,
            &opts,
        )
    };
    assert!(!block("plain body").is_empty());

    let error = render_block(
        &Block::Error {
            message: "bad".into(),
            code: Some("E1".into()),
            recoverable: true,
            timestamp: Some("t".into()),
        },
        40,
        &opts,
    );
    assert!(
        error
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("bad")))
    );

    let tool = render_block(
        &Block::ToolCall {
            name: "read".into(),
            args: "path".into(),
            metadata: Some("meta".into()),
            timestamp: None,
        },
        40,
        &opts,
    );
    assert!(
        tool.iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("read")))
    );

    let mut peek_opts = opts;
    peek_opts.thinking_mode = ThinkingMode::Peek;
    let thinking = render_block(
        &Block::Thinking {
            text: "one\ntwo\nthree\nfour\nfive".into(),
            timestamp: None,
        },
        20,
        &peek_opts,
    );
    assert!(!thinking.is_empty());

    let hidden = render_block(
        &Block::Thinking {
            text: "hidden".into(),
            timestamp: None,
        },
        20,
        &FeedRenderOptions {
            thinking_mode: ThinkingMode::Hidden,
            ..Default::default()
        },
    );
    assert!(hidden.is_empty());

    let assistant = render_block(
        &Block::Assistant {
            text: "**bold** [link](https://example.com)".into(),
            timestamp: None,
        },
        30,
        &opts,
    );
    assert!(!assistant.is_empty());
}

#[test]
fn feed_render_tool_pair_and_incremental_wrap() {
    use theway_transport::feed::Block;

    let _opts = FeedRenderOptions::default();
    assert_eq!(
        crate::feed_render::tool_pair_len(
            &[Block::User {
                text: "x".into(),
                timestamp: None,
                attachments: Vec::new(),
                source: None,
            }],
            0
        ),
        1
    );
    assert_eq!(
        crate::feed_render::tool_pair_len(
            &[
                Block::ToolCall {
                    name: "x".into(),
                    args: String::new(),
                    metadata: None,
                    timestamp: None
                },
                Block::ToolResult {
                    tool_call_id: "".into(),
                    lines: vec![],
                    is_error: false,
                    timestamp: None
                },
            ],
            0,
        ),
        2
    );
    assert_eq!(
        crate::feed_render::unit_count(&[
            Block::ToolCall {
                name: "x".into(),
                args: String::new(),
                metadata: None,
                timestamp: None
            },
            Block::ToolResult {
                tool_call_id: "".into(),
                lines: vec![],
                is_error: false,
                timestamp: None
            },
            Block::User {
                text: "y".into(),
                timestamp: None,
                attachments: Vec::new(),
                source: None,
            },
        ]),
        2
    );
    assert_ne!(
        crate::feed_render::unit_fingerprint(
            &[
                Block::ToolCall {
                    name: "a".into(),
                    args: String::new(),
                    metadata: None,
                    timestamp: None
                },
                Block::ToolResult {
                    tool_call_id: "".into(),
                    lines: vec![],
                    is_error: false,
                    timestamp: None
                },
            ],
            0
        ),
        0
    );

    let mut wrap = crate::feed_render::IncrementalWrap::new(5);
    wrap.push_str("hello world");
    assert!(!wrap.rows.is_empty());
    wrap.push_str("\nnext");
    assert_eq!(wrap.tail, "next");
}

#[test]
fn feed_render_block_edge_variants() {
    use theway_transport::feed::Block;

    let opts = FeedRenderOptions::default();
    // User block with a newline and extremely narrow width.
    let user = render_block(
        &Block::User {
            text: "hello\nworld".into(),
            timestamp: None,
            attachments: Vec::new(),
            source: None,
        },
        1,
        &opts,
    );
    assert!(!user.is_empty());

    // Error without code / non-recoverable / no timestamp.
    let err = render_block(
        &Block::Error {
            message: "boom".into(),
            code: None,
            recoverable: false,
            timestamp: None,
        },
        20,
        &opts,
    );
    assert!(
        err.iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("boom")))
    );

    // Tool call with no args/metadata.
    let call = render_block(
        &Block::ToolCall {
            name: "run".into(),
            args: String::new(),
            metadata: None,
            timestamp: None,
        },
        20,
        &opts,
    );
    assert!(
        call.iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("run")))
    );

    // Expanded result with an unclosed mermaid fence.
    let expanded = render_block(
        &Block::ToolResult {
            tool_call_id: "".into(),
            lines: vec!["```mermaid".into(), "graph TD".into(), "  A --> B".into()],
            is_error: false,
            timestamp: None,
        },
        40,
        &FeedRenderOptions {
            tools_expanded: true,
            ..Default::default()
        },
    );
    assert!(!expanded.is_empty());

    // Plain rows go through the URL underline scanner.
    let url = render_block(
        &Block::Plain {
            text: "see https://example.com/path".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
        40,
        &opts,
    );
    assert!(
        url.iter().flat_map(|l| &l.spans).any(|s| s
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::UNDERLINED)),
        "plain URL rows should receive underline affordance: {url:?}"
    );

    // Tool-pair fallback: non-matching blocks just render independently.
    let pair = crate::feed_render::render_tool_pair(
        &Block::User {
            text: "u".into(),
            timestamp: None,
            attachments: Vec::new(),
            source: None,
        },
        &Block::Plain {
            text: "p".into(),
            level: theway_transport::feed::Level::Output,
            timestamp: None,
        },
        20,
        &opts,
    );
    assert!(!pair.is_empty());
}

#[test]
fn feed_render_markdown_line_hyperlinks_wrap() {
    use ratatui::style::Style;
    use ratatui::text::{Line, Span};
    use theway_markdown::{CodeBlockSpan, HyperlinkTarget};

    let mut out = Vec::new();
    let long = "x".repeat(50);
    let line = Line::from(vec![Span::raw(long.clone())]);
    let link = HyperlinkTarget {
        line_index: 0,
        column_range: 0..10,
        url: "https://example.com".into(),
        id: 1,
    };
    crate::feed_render::push_rendered_markdown_line(
        &mut out,
        0,
        line,
        "",
        Style::default(),
        10,
        &[] as &[CodeBlockSpan],
        &[link],
    );
    assert!(out.len() > 1, "long line should wrap into multiple rows");
    assert!(
        out.iter().flat_map(|l| &l.spans).any(|s| s
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::UNDERLINED)),
        "hyperlink underline should be projected onto wrapped rows"
    );

    // A table-shaped line is kept verbatim even beyond width.
    let mut out = Vec::new();
    let table = "┌───┐".to_string();
    let line = Line::from(vec![Span::raw(table.clone())]);
    crate::feed_render::push_rendered_markdown_line(
        &mut out,
        0,
        line,
        "",
        Style::default(),
        2,
        &[],
        &[],
    );
    assert_eq!(out.len(), 1, "table rows must stay verbatim");
}

#[test]
fn feed_render_incremental_wrap_more_boundaries() {
    let mut wrap = crate::feed_render::IncrementalWrap::new(3);
    wrap.push_str("ab");
    assert_eq!(wrap.tail, "ab");
    wrap.push_str("cd");
    assert_eq!(wrap.rows, vec!["abc".to_string()]);
    assert_eq!(wrap.tail, "d");
    wrap.push_str("\n");
    assert!(wrap.rows.len() >= 2);
    wrap.push_str("");
    assert_eq!(wrap.tail, "");
}
