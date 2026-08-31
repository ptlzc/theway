/// Push the inter-block separator rows (issue #30): `gap` blank lines, or a
/// full-width styled separator line when `[feed] separator` is set (the gap
/// still applies above it). Callers decide WHERE a gap goes via the transport
/// `should_separate`; this decides HOW MUCH.
pub(crate) fn push_feed_gap(
    out: &mut Vec<Line<'static>>,
    width: usize,
    feed: &crate::ui::theme::FeedTheme,
) {
    for _ in 0..feed.gap {
        out.push(Line::raw(""));
    }
    if let Some(glyph) = feed.separator {
        let line = glyph.to_string().repeat(width.max(1));
        out.push(Line::from(Span::styled(
            line,
            Style::default().fg(feed.separator_style),
        )));
    }
}

/// Render the whole feed to width-wrapped `ratatui` lines, ready to scroll/draw.
///
/// The production render path uses the block cache (`feed_cache`) instead;
/// this whole-feed renderer stays for tests (reference composition with
/// separators) and diagnostics.
///
/// Grok-style block styling (issue #33): user rows carry a `❯` accent prefix on
/// an elevated band, tool calls are `⏵ name` + dim args single-liners, tool
/// results collapse to a one-line summary unless `tools_expanded`, and
/// thinking blocks honor `thinking_mode`. Timestamps are dropped for
/// conversational blocks (grok shows none); plain status lines keep theirs.
#[cfg(test)]
pub fn lines(
    feed: &theway_transport::feed::Feed,
    width: usize,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut previous: Option<&Block> = None;
    let blocks = feed.blocks();
    let mut i = 0;
    while i < blocks.len() {
        let n = tool_pair_len(blocks, i);
        if theway_transport::feed::should_separate_with(
            previous,
            &blocks[i],
            !out.is_empty(),
            opts.theme.feed.separate_all,
        ) {
            push_feed_gap(&mut out, width, &opts.theme.feed);
        }
        if n == 2 {
            out.extend(render_tool_pair(&blocks[i], &blocks[i + 1], width, opts));
        } else {
            out.extend(render_block(&blocks[i], width, opts));
        }
        previous = Some(&blocks[i + n - 1]);
        i += n;
    }
    out
}

/// Render ONE feed block to width-wrapped lines (no inter-block separator).
/// The feed render cache calls this per dirty block; [`lines`] composes it
/// with separators. Assistant blocks skip the URL regex scan (their underlines
/// come from the markdown renderer's hyperlinks); every other block scans its
/// own lines once, so the per-block result is stable and cacheable.
///
/// Tool / ToolResult / Thinking blocks carry the theme block layout (issue
/// #49): background, padding columns and left/right alignment, with the
/// background spanning every block row at full width. Colors come from the
/// theme roles in `opts.theme` (issue #43).
pub(crate) fn render_block(
    block: &Block,
    width: usize,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let theme = &opts.theme;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut assistant_rows: Vec<std::ops::Range<usize>> = Vec::new();
    match block {
        Block::User { text, .. } => push_user_block(&mut out, text, width, theme),
        Block::Assistant { text, .. } => {
            // Assistant blocks are markdown: one-shot pretty render via
            // theway-markdown, link underlines from the renderer's
            // hyperlinks, verbatim code/table/mermaid rows, wrapped prose.
            let start = out.len();
            push_markdown(
                &mut out,
                text,
                "",
                Style::default(),
                width,
                opts.color_level,
            );
            // `assistant_text` role: fallback foreground for spans the
            // markdown renderer left uncolored (syntax colors win).
            if let Some(fg) = theme.assistant_text {
                for line in &mut out[start..] {
                    for span in &mut line.spans {
                        if span.style.fg.is_none() {
                            span.style = span.style.fg(fg);
                        }
                    }
                }
            }
            assistant_rows.push(start..out.len());
        }
        Block::Thinking { text, .. } => {
            let bg = theme.thinking.bg.or(theme.thinking_bg);
            let content_w = block_content_width(width, bg, theme.thinking.padding);
            let mut rows: Vec<Line<'static>> = Vec::new();
            match opts.thinking_mode {
                ThinkingMode::Hidden => {}
                ThinkingMode::Peek => push_thinking_peek(&mut rows, text, opts, content_w),
                ThinkingMode::Full => {
                    push_thinking_stats_line(&mut rows, text, opts, content_w);
                    push_paragraphs(
                        &mut rows,
                        text,
                        thinking_style(theme),
                        Some(TOOL_PREFIX),
                        content_w,
                    )
                }
            }
            apply_block_layout(&mut rows, width, bg, &theme.thinking);
            apply_block_frame(&mut rows, width, &theme.thinking);
            out.extend(rows);
        }
        Block::ToolCall {
            name,
            args,
            metadata,
            ..
        } => {
            let bg = theme.tool.bg.or(theme.tool_running_bg);
            let mut rows = vec![tool_call_row(name, args, metadata, width, bg, theme)];
            apply_block_layout(&mut rows, width, bg, &theme.tool);
            apply_block_frame(&mut rows, width, &theme.tool);
            out.extend(rows);
        }
        Block::Error {
            message,
            code,
            recoverable,
            timestamp,
        } => {
            let mut text = message.clone();
            if let Some(code) = code {
                text.push_str(&format!(" ({code})"));
            }
            if *recoverable {
                text.push_str(" [recoverable]");
            }
            let prefix = timestamp.as_deref().map(|ts| display_prefix(Some(ts), ""));
            push_paragraphs(
                &mut out,
                &text,
                style_for_level(Level::Error),
                prefix.as_deref(),
                width,
            );
        }
        Block::ToolResult {
            lines, is_error, ..
        } => {
            let bg = theme.tool.bg.or(if *is_error {
                theme.tool_error_bg
            } else {
                theme.tool_success_bg
            });
            let mut rows = tool_result_rows(lines, *is_error, width, bg, opts);
            apply_block_layout(&mut rows, width, bg, &theme.tool);
            apply_block_frame(&mut rows, width, &theme.tool);
            out.extend(rows);
        }
        Block::Plain {
            text,
            level,
            timestamp,
        } => {
            let prefix = timestamp.as_deref().map(|ts| display_prefix(Some(ts), ""));
            push_paragraphs(
                &mut out,
                text,
                style_for_level(*level),
                prefix.as_deref(),
                width,
            );
        }
    }
    underline_links(&mut out, &assistant_rows);
    out
}

/// One tool-call row: `⏵ name args · metadata`, truncated to the block
/// content width (single-line tool rows never wrap).
fn tool_call_row(
    name: &str,
    args: &str,
    metadata: &Option<String>,
    width: usize,
    bg: Option<Color>,
    theme: &Theme,
) -> Line<'static> {
    let content_w = block_content_width(width, bg, theme.tool.padding);
    let mut spans = vec![Span::styled(
        format!("{TOOL_PREFIX}{name}"),
        tool_name_style(theme),
    )];
    if !args.is_empty() {
        spans.push(Span::styled(format!(" {args}"), tool_args_style(theme)));
    }
    if let Some(metadata) = metadata {
        spans.push(Span::styled(
            format!(" · {metadata}"),
            tool_args_style(theme),
        ));
    }
    let mut line = Line::from(spans);
    truncate_line(&mut line, content_w);
    line
}

/// Tool-result body rows (preview or full expansion, mermaid-aware),
/// pre-layout: callers own the block layout/frame application.
fn tool_result_rows(
    lines: &[String],
    is_error: bool,
    width: usize,
    bg: Option<Color>,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let theme = &opts.theme;
    let content_w = block_content_width(width, bg, theme.tool.padding);
    let style = if is_error {
        Style::default().fg(theme.tool_error)
    } else {
        Style::default().fg(theme.tool_result)
    };
    let mut rows: Vec<Line<'static>> = Vec::new();
    if opts.tools_expanded {
        push_tool_result_expanded(&mut rows, lines, content_w, style);
    } else {
        push_tool_result_preview(&mut rows, lines, is_error, content_w, theme);
    }
    rows
}

/// Render a tool-call + its result as ONE tool area (issue #69): the call
/// row and the result body share the block layout and a single background
/// band — no feed gap between them, so they read as one container. A
/// configured `border_bottom` draws the internal divider between the call
/// row and the result body (instead of closing the call block); margins and
/// `border_top` still frame the whole area. Falls back to two independent
/// blocks when the pair doesn't line up (defensive).
pub(crate) fn render_tool_pair(
    call: &Block,
    result: &Block,
    width: usize,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let theme = &opts.theme;
    let (Block::ToolCall {
        name,
        args,
        metadata,
        ..
    }, Block::ToolResult {
        lines, is_error, ..
    }) = (call, result)
    else {
        let mut out = render_block(call, width, opts);
        out.extend(render_block(result, width, opts));
        return out;
    };
    let call_bg = theme.tool.bg.or(theme.tool_running_bg);
    let result_bg = theme.tool.bg.or(if *is_error {
        theme.tool_error_bg
    } else {
        theme.tool_success_bg
    });
    let mut out: Vec<Line<'static>> = Vec::new();
    for _ in 0..theme.tool.margin_top {
        out.push(Line::raw(""));
    }
    if let Some(glyph) = border_glyph(theme.tool.border_top) {
        out.push(border_line(width, glyph, theme.tool.border_style));
    }
    let mut rows = vec![tool_call_row(name, args, metadata, width, call_bg, theme)];
    apply_block_layout(&mut rows, width, call_bg, &theme.tool);
    out.extend(rows);
    // Internal divider: the call block's bottom border separates the call
    // row from the result body inside the shared band.
    if let Some(glyph) = border_glyph(theme.tool.border_bottom) {
        out.push(border_line_filled(
            width,
            glyph,
            theme.tool.border_style,
            result_bg,
        ));
    }
    let mut rows = tool_result_rows(lines, *is_error, width, result_bg, opts);
    apply_block_layout(&mut rows, width, result_bg, &theme.tool);
    out.extend(rows);
    for _ in 0..theme.tool.margin_bottom {
        out.push(Line::raw(""));
    }
    out
}

/// Number of feed blocks covered by the render unit starting at `i`: a
/// `ToolCall` immediately followed by its `ToolResult` renders as ONE tool
/// area, so the pair is a single cache unit.
pub(crate) fn tool_pair_len(blocks: &[Block], i: usize) -> usize {
    if i + 1 < blocks.len()
        && matches!(blocks[i], Block::ToolCall { .. })
        && matches!(blocks[i + 1], Block::ToolResult { .. })
    {
        2
    } else {
        1
    }
}

/// Total number of render units across `blocks` (pairs count once).
pub(crate) fn unit_count(blocks: &[Block]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < blocks.len() {
        i += tool_pair_len(blocks, i);
        n += 1;
    }
    n
}

/// Content fingerprint of the render unit starting at `i` (single block, or
/// a tool-call + result pair hashed as one). Two units with equal
/// fingerprints render identically at the same width.
pub(crate) fn unit_fingerprint(blocks: &[Block], i: usize) -> u64 {
    let n = tool_pair_len(blocks, i);
    if n == 1 {
        return block_fingerprint(&blocks[i]);
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for block in &blocks[i..i + n] {
        for byte in block_fingerprint(block).to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Content width for a tool/thinking block row: with a background active the
/// padding columns are reserved on both sides; without one the block keeps
/// the classic full-width layout (theme padding is part of the background
/// fill and renders nothing on its own).
fn block_content_width(width: usize, bg: Option<Color>, padding: u16) -> usize {
    if bg.is_some() {
        width
            .saturating_sub(usize::from(padding).saturating_mul(2))
            .max(1)
    } else {
        width
    }
}

/// Paint the block layout over already-wrapped rows (issue #49): every row
/// becomes exactly `width` columns — `padding` background columns on each
/// side, content between them (hugging the right padding under
/// [`BlockAlign::Right`]) — and every span carries the background, so the
/// block reads as one solid bar. Content rows shorter than the block width
/// are filled with pure background spans; empty content rows render as pure
/// background. No background → no-op (classic flush layout).
fn apply_block_layout(
    rows: &mut [Line<'static>],
    width: usize,
    bg: Option<Color>,
    layout: &BlockTheme,
) {
    let Some(bg) = bg else { return };
    let width = width.max(1);
    let pad = usize::from(layout.padding).min(width / 2);
    let inner = width - pad * 2;
    for line in rows {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let content_w = unicode_width::UnicodeWidthStr::width(text.as_str());
        if content_w > inner {
            truncate_line(line, inner);
        }
        let gap = inner.saturating_sub(content_w.min(inner));
        let (lead, trail) = match layout.align {
            BlockAlign::Left => (pad, pad + gap),
            BlockAlign::Right => (pad + gap, pad),
        };
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
        if lead > 0 {
            spans.push(Span::styled(" ".repeat(lead), Style::new().bg(bg)));
        }
        for span in std::mem::take(&mut line.spans) {
            spans.push(Span::styled(span.content, span.style.bg(bg)));
        }
        if trail > 0 {
            spans.push(Span::styled(" ".repeat(trail), Style::new().bg(bg)));
        }
        line.spans = spans;
    }
}

/// Apply the block frame (issue #31): `margin_top` blank rows, a top border
/// line, the content, a bottom border line, then `margin_bottom` blank rows.
/// Margins accumulate with `[feed] gap` (emitted separately by
/// `push_feed_gap`); borders draw inside the margins. A no-op when every
/// frame knob is at its default, so default rendering stays byte-identical.
fn apply_block_frame(rows: &mut Vec<Line<'static>>, width: usize, layout: &BlockTheme) {
    if layout.margin_top == 0
        && layout.margin_bottom == 0
        && layout.border_top == BlockBorder::None
        && layout.border_bottom == BlockBorder::None
    {
        return;
    }
    let mut framed: Vec<Line<'static>> = Vec::with_capacity(rows.len() + 4);
    for _ in 0..layout.margin_top {
        framed.push(Line::raw(""));
    }
    if let Some(glyph) = border_glyph(layout.border_top) {
        framed.push(border_line(width, glyph, layout.border_style));
    }
    framed.extend(std::mem::take(rows));
    if let Some(glyph) = border_glyph(layout.border_bottom) {
        framed.push(border_line(width, glyph, layout.border_style));
    }
    for _ in 0..layout.margin_bottom {
        framed.push(Line::raw(""));
    }
    *rows = framed;
}

fn border_glyph(border: BlockBorder) -> Option<char> {
    match border {
        BlockBorder::None => None,
        BlockBorder::Thin => Some('─'),
        BlockBorder::Thick => Some('━'),
    }
}

fn border_line(width: usize, glyph: char, style: Color) -> Line<'static> {
    Line::from(Span::styled(
        glyph.to_string().repeat(width.max(1)),
        Style::default().fg(style),
    ))
}

/// Full-width divider line carrying the band background: used for the
/// internal call/result separator inside a tool area, so the line reads as
/// part of the shared background instead of punching through it.
fn border_line_filled(
    width: usize,
    glyph: char,
    style: Color,
    bg: Option<Color>,
) -> Line<'static> {
    let mut span_style = Style::default().fg(style);
    if let Some(bg) = bg {
        span_style = span_style.bg(bg);
    }
    Line::from(Span::styled(glyph.to_string().repeat(width.max(1)), span_style))
}
