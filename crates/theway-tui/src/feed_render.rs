//! Ratatui rendering for the conversation feed (daemon-kernel-layers: the
//! terminal rendering moved from the SDK into the TUI; the UI-agnostic model
//! lives in `theway_transport::feed`).
//!
//! [`lines`] renders a [`Feed`] to width-wrapped, styled `ratatui` lines,
//! ready to scroll/draw — the terminal counterpart of `Feed::plain_lines`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use theway_transport::feed::{Block, Level, display_prefix, wrap_str};

/// Grok tokyonight palette values (xai-grok-pager-render theme/tokyonight.rs).
const ACCENT_USER: Color = Color::Rgb(122, 162, 247); // BLUE — user `❯` prefix
const ACCENT_ASSISTANT: Color = Color::Rgb(187, 154, 247); // MAGENTA — `ai ▸` prefix
const ACCENT_TOOL: Color = Color::Rgb(115, 122, 162); // DARK5 — tool name
const TEXT_PRIMARY: Color = Color::Rgb(192, 202, 245); // FG — body text
const BG_HIGHLIGHT: Color = Color::Rgb(41, 46, 66); // BG_HIGHLIGHT — user band / selection

const USER_PREFIX: &str = "\u{276F} "; // ❯ (2 cols, grok prompt_arrow)
const AI_PREFIX: &str = "ai \u{25b8} "; // ai ▸
const TOOL_PREFIX: &str = "\u{23f5} "; // ⏵
const USER_BAND_INDENT: &str = "  ";

const USER_STYLE: Style = Style::new().fg(ACCENT_USER).add_modifier(Modifier::BOLD);
const THINKING_STYLE: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
const USER_BODY_STYLE: Style = Style::new().fg(TEXT_PRIMARY);
const AI_PREFIX_STYLE: Style = Style::new()
    .fg(ACCENT_ASSISTANT)
    .add_modifier(Modifier::BOLD);
const TOOL_NAME_STYLE: Style = Style::new().fg(ACCENT_TOOL).add_modifier(Modifier::BOLD);
const TOOL_ARGS_STYLE: Style = Style::new().fg(Color::DarkGray);
const RESULT_SUMMARY_STYLE: Style = Style::new().fg(Color::DarkGray);
const BAND_STYLE: Style = Style::new().bg(BG_HIGHLIGHT);

/// How `Block::Thinking` renders in the feed (Ctrl+O cycles).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThinkingMode {
    /// Full thinking text (default).
    #[default]
    Full,
    /// Peek window: header + the last few lines only.
    Peek,
    /// Skipped entirely.
    Hidden,
}

/// Renderer switches owned by the TUI app state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedRenderOptions {
    pub thinking_mode: ThinkingMode,
    /// Tool results: collapsed to a one-line summary unless expanded (Ctrl+T).
    pub tools_expanded: bool,
}

/// Lines shown in the thinking peek window.
const THINKING_PEEK_LINES: usize = 3;

pub fn style_for_level(level: Level) -> Style {
    match level {
        Level::Output => Style::default(),
        Level::System => Style::default().fg(Color::DarkGray),
        Level::Error => Style::default().fg(Color::Red),
        Level::Note => Style::default().fg(Color::Green),
        Level::Header => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        Level::Qr => Style::default(),
    }
}

/// Split `text` on newlines, word-wrap each paragraph to `width`, and push styled lines. An
/// optional `prefix` is prepended to the very first paragraph (e.g. `you ▸ `).
pub fn push_paragraphs(
    out: &mut Vec<Line<'static>>,
    text: &str,
    style: Style,
    prefix: Option<&str>,
    width: usize,
) {
    for (i, para) in text.split('\n').enumerate() {
        let owned;
        let para = if i == 0 {
            if let Some(p) = prefix {
                owned = format!("{p}{para}");
                owned.as_str()
            } else {
                para
            }
        } else {
            para
        };
        for row in wrap_str(para, width) {
            out.push(Line::styled(row, style));
        }
    }
}

/// Markdown style for assistant feed blocks.
///
/// `theway-markdown` derives an all-plain `MarkdownStyle::default()` (it is a
/// customization point for full-palette consumers such as the pager theme);
/// the feed needs effect-based styles so bold/italic/headings/inline code
/// actually render and pretty mode hides the syntax markers. Effects-only
/// (no foreground colors) keeps the feed legible on every terminal; colors
/// are then adapted to the terminal's color capabilities via [`adapt`].
///
/// [`adapt`]: theway_markdown::MarkdownStyle::adapt
fn markdown_style() -> theway_markdown::MarkdownStyle {
    use anstyle::Style as AStyle;
    theway_markdown::MarkdownStyle {
        heading_inner: [AStyle::new().bold(); 6],
        heading_outer: [AStyle::new().hidden(); 6],
        strong_inner: AStyle::new().bold(),
        strong_outer: AStyle::new().hidden(),
        emphasis_inner: AStyle::new().italic(),
        emphasis_outer: AStyle::new().hidden(),
        strikethrough_inner: AStyle::new().strikethrough(),
        strikethrough_outer: AStyle::new().hidden(),
        inline_code_inner: AStyle::new().bold(),
        inline_code_outer: AStyle::new().hidden(),
        blockquote_outer: AStyle::new().dimmed(),
        task_checked: AStyle::new(),
        task_unchecked: AStyle::new().dimmed(),
        list_item: AStyle::new().dimmed(),
        rule: AStyle::new(),
        link_outer: AStyle::new(),
        link_text: AStyle::new().bold(),
        link_url: AStyle::new().dimmed(),
        link_title: AStyle::new(),
        code_outer: AStyle::new().hidden(),
        code_language: AStyle::new().hidden(),
        code_untagged: AStyle::new(),
        code_background: AStyle::new(),
        table_outer: AStyle::new().bold(),
        text: AStyle::new(),
        math: AStyle::new().italic(),
    }
    .adapt()
}

/// Table border glyphs: pretty-mode tables render with box-drawing borders
/// (`TableBorders` BOX default, DOUBLE included for safety). Lines containing
/// these glyphs are table rows and must stay verbatim (no re-wrapping).
const TABLE_BORDER_CHARS: &[char] = &[
    '─', '│', '┌', '┐', '└', '┘', '┬', '┴', '├', '┤', '┼', '═', '║', '╔', '╗', '╚', '╝', '╦', '╩',
    '╠', '╣', '╬',
];

fn is_table_line(line: &Line<'_>) -> bool {
    line.spans.iter().any(|span| {
        span.content
            .chars()
            .any(|c| TABLE_BORDER_CHARS.contains(&c))
    })
}

/// One row produced by [`wrap_str_ranges`]: the source byte range it covers
/// (rows are trimmed at break points, so ranges exclude the dropped boundary
/// whitespace).
struct WrappedRow {
    range: std::ops::Range<usize>,
}

/// [`wrap_str`] equivalent that also reports each row's byte range in `text`.
/// Kept byte-for-byte in sync with `wrap_str` so hyperlink column ranges and
/// span styles can be projected onto the wrapped rows.
fn wrap_str_ranges(text: &str, width: usize) -> Vec<WrappedRow> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![WrappedRow { range: 0..0 }];
    }
    let mut rows: Vec<WrappedRow> = Vec::new();
    let mut cur = String::new();
    let mut chunk_start = 0usize;
    let mut chunk_end = 0usize;
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            if let Some(bp) = last_space.take() {
                let rest_orig = cur.split_off(bp);
                let rest = rest_orig.trim_start_matches(' ').to_string();
                let trimmed_front = rest_orig.len() - rest.len();
                let done_len = cur.trim_end().len();
                rows.push(WrappedRow {
                    range: chunk_start..chunk_start + done_len,
                });
                cur = rest;
                chunk_start += bp + trimmed_front;
                cur_w = unicode_width::UnicodeWidthStr::width(cur.as_str());
            } else {
                rows.push(WrappedRow {
                    range: chunk_start..chunk_end,
                });
                cur.clear();
                chunk_start = chunk_end;
                cur_w = 0;
            }
        }
        cur.push(ch);
        chunk_end += ch.len_utf8();
        cur_w += cw;
        if ch == ' ' {
            last_space = Some(cur.len());
        }
    }
    rows.push(WrappedRow {
        range: chunk_start..chunk_end,
    });
    rows
}

/// Where a pre-wrap rendered line landed in the output rows.
struct MappedLine {
    /// First output row index for this pre-wrap line.
    start: usize,
    /// Pre-wrap source text (prefix included for the first line) when the
    /// line was re-wrapped; `None` when the line was pushed verbatim.
    source: Option<String>,
    /// Wrapped rows with their byte ranges in `source` (empty when verbatim).
    rows: Vec<WrappedRow>,
}

/// Render assistant markdown through `theway-markdown` (one-shot full render,
/// pretty mode) and push width-wrapped `ratatui` lines.
///
/// `prefix` (e.g. `ai ▸ `) is prepended to the first rendered line only, in
/// `prefix_style`. Fenced-code body lines (per the renderer's `code_blocks`
/// output ranges) and table rows stay verbatim; every other line wraps with
/// `wrap_str`. The width-aware render entry passes `max_table_width` so fenced
/// `mermaid` diagrams come out sized to the feed (over-wide graphs fall back
/// to a framed source box). Link underlines come from the renderer's
/// `hyperlinks` output — the assistant block does not go through the regex
/// URL scan.
pub fn push_markdown(
    out: &mut Vec<Line<'static>>,
    text: &str,
    prefix: &str,
    prefix_style: Style,
    width: usize,
) {
    let (rendered, _checkpoint) = theway_markdown::render_markdown_ratatui_full_width(
        text,
        markdown_style(),
        true,
        Some(theway_markdown::default_syntect()),
        Some(width),
    );
    let width = width.max(1);
    let prefix_width = unicode_width::UnicodeWidthStr::width(prefix);

    let mut mapped: Vec<MappedLine> = Vec::with_capacity(rendered.lines.len());
    for (i, line) in rendered.lines.into_iter().enumerate() {
        let first = i == 0;
        let in_code = rendered
            .code_blocks
            .iter()
            .any(|cb| cb.output_line_range.contains(&i));
        let keep = in_code || is_table_line(&line);

        let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let source = if first {
            format!("{prefix}{line_text}")
        } else {
            line_text
        };

        if keep || unicode_width::UnicodeWidthStr::width(source.as_str()) <= width {
            let mut line = line;
            if first {
                line.spans
                    .insert(0, Span::styled(prefix.to_string(), prefix_style));
            }
            mapped.push(MappedLine {
                start: out.len(),
                source: None,
                rows: Vec::new(),
            });
            out.push(line);
        } else {
            // Re-wrap the line with `wrap_str` semantics but re-apply each
            // span's style through the row byte ranges, so inline styles
            // (bold/italic/code/link) survive the wrap instead of degrading
            // to a single base style.
            let prefix_len = if first { prefix.len() } else { 0 };
            let mut pieces: Vec<(std::ops::Range<usize>, Style)> =
                Vec::with_capacity(line.spans.len() + 1);
            if first {
                pieces.push((0..prefix_len, prefix_style));
            }
            let mut offset = prefix_len;
            for span in &line.spans {
                let end = offset + span.content.len();
                pieces.push((offset..end, span.style));
                offset = end;
            }
            let rows = wrap_str_ranges(&source, width);
            let start = out.len();
            for row in &rows {
                let mut spans: Vec<Span<'static>> = Vec::new();
                for (range, style) in &pieces {
                    let overlap = range.start.max(row.range.start)..range.end.min(row.range.end);
                    if overlap.start < overlap.end {
                        spans.push(Span::styled(source[overlap].to_string(), *style));
                    }
                }
                out.push(Line::from(spans));
            }
            mapped.push(MappedLine {
                start,
                source: Some(source),
                rows,
            });
        }
    }

    // Underline hyperlinks on the final rows: the renderer reports each link
    // as (pre-wrap line index, display-column range), which maps directly
    // onto verbatim rows and through the byte ranges onto wrapped rows.
    for link in &rendered.hyperlinks {
        let Some(mapped_line) = mapped.get(link.line_index) else {
            continue;
        };
        let shift = if link.line_index == 0 {
            prefix_width
        } else {
            0
        };
        let start_col = link.column_range.start + shift;
        let end_col = link.column_range.end + shift;
        match &mapped_line.source {
            Some(source) => {
                let byte_start =
                    theway_pager_render::line_utils::byte_offset_at_width(source, start_col);
                let byte_end =
                    theway_pager_render::line_utils::byte_offset_at_width(source, end_col);
                for (j, row) in mapped_line.rows.iter().enumerate() {
                    let overlap = byte_start.max(row.range.start)..byte_end.min(row.range.end);
                    if overlap.start >= overlap.end {
                        continue;
                    }
                    let row_idx = mapped_line.start + j;
                    let row_start_width =
                        unicode_width::UnicodeWidthStr::width(&source[..row.range.start]);
                    let c0 = unicode_width::UnicodeWidthStr::width(&source[..overlap.start])
                        - row_start_width;
                    let c1 = unicode_width::UnicodeWidthStr::width(&source[..overlap.end])
                        - row_start_width;
                    underline_range(&mut out[row_idx], c0, c1);
                }
            }
            None => underline_range(&mut out[mapped_line.start], start_col, end_col),
        }
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
    for block in feed.blocks() {
        if theway_transport::feed::should_separate(previous, block, !out.is_empty()) {
            out.push(Line::raw(""));
        }
        out.extend(render_block(block, width, opts));
        previous = Some(block);
    }
    out
}

/// Render ONE feed block to width-wrapped lines (no inter-block separator).
/// The feed render cache calls this per dirty block; [`lines`] composes it
/// with separators. Assistant blocks skip the URL regex scan (their underlines
/// come from the markdown renderer's hyperlinks); every other block scans its
/// own lines once, so the per-block result is stable and cacheable.
pub(crate) fn render_block(
    block: &Block,
    width: usize,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut assistant_rows: Vec<std::ops::Range<usize>> = Vec::new();
    match block {
        Block::User { text, .. } => push_user_block(&mut out, text, width),
        Block::Assistant { text, .. } => {
            // Assistant blocks are markdown: one-shot pretty render via
            // theway-markdown, link underlines from the renderer's
            // hyperlinks, verbatim code/table/mermaid rows, wrapped prose.
            let start = out.len();
            push_markdown(&mut out, text, AI_PREFIX, AI_PREFIX_STYLE, width);
            assistant_rows.push(start..out.len());
        }
        Block::Thinking { text, .. } => match opts.thinking_mode {
            ThinkingMode::Hidden => {}
            ThinkingMode::Peek => push_thinking_peek(&mut out, text, width),
            ThinkingMode::Full => {
                push_paragraphs(&mut out, text, THINKING_STYLE, Some(TOOL_PREFIX), width)
            }
        },
        Block::Tool { name, args, .. } => {
            let mut spans = vec![Span::styled(
                format!("{TOOL_PREFIX}{name}"),
                TOOL_NAME_STYLE,
            )];
            if !args.is_empty() {
                spans.push(Span::styled(format!(" {args}"), TOOL_ARGS_STYLE));
            }
            let mut line = Line::from(spans);
            truncate_line(&mut line, width);
            out.push(line);
        }
        Block::ToolResult {
            lines, is_error, ..
        } => {
            if !opts.tools_expanded {
                let bytes: usize = lines.iter().map(String::len).sum();
                out.push(Line::styled(
                    format!(
                        "    result · {} lines · {} B · Ctrl+T expand",
                        lines.len(),
                        bytes
                    ),
                    RESULT_SUMMARY_STYLE,
                ));
            } else {
                let style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                };
                for line in lines {
                    for row in wrap_str(&format!("    {line}"), width) {
                        out.push(Line::styled(row, style));
                    }
                }
            }
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

/// Content fingerprint of one feed block (fnv-1a over kind + fields). Two
/// blocks with identical fingerprints render identically for the same
/// width/options, so the render cache reuses their rendered lines.
pub(crate) fn block_fingerprint(block: &Block) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    match block {
        Block::User { text, timestamp } => {
            mix(b"user\x00");
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Assistant { text, timestamp } => {
            mix(b"assistant\x00");
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Thinking { text, timestamp } => {
            mix(b"thinking\x00");
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Tool {
            name,
            args,
            timestamp,
        } => {
            mix(b"tool\x00");
            mix(name.as_bytes());
            mix(args.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::ToolResult {
            lines,
            is_error,
            timestamp,
            ..
        } => {
            mix(b"toolresult\x00");
            mix(if *is_error { b"1" } else { b"0" });
            for line in lines {
                mix(line.as_bytes());
                mix(b"\x00");
            }
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Plain {
            text,
            level,
            timestamp,
        } => {
            mix(b"plain\x00");
            mix(format!("{level:?}").as_bytes());
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
    }
    hash
}

/// Draw pre-wrapped lines into the visible window only (O(viewport)) — the
/// cache-friendly replacement for `Paragraph::new(lines).scroll(...)` (issue
/// #34). Rows outside the window are never touched, and the area is cleared
/// first so a shrinking feed cannot leave stale cells behind. `selection` is
/// an inclusive range of *capped* line indices rendered as highlighted rows.
pub fn render_lines_window(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    lines: &[Line<'static>],
    offset: usize,
    selection: Option<std::ops::RangeInclusive<usize>>,
) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
            }
        }
    }
    for (i, line) in lines.iter().enumerate().skip(offset) {
        let row = i - offset;
        if row >= area.height as usize {
            break;
        }
        let y = area.y + row as u16;
        if selection.as_ref().is_some_and(|sel| sel.contains(&i)) {
            let mut selected = line.clone();
            highlight_line(&mut selected);
            set_line_safe(buf, area.x, y, &selected, area.width);
        } else {
            set_line_safe(buf, area.x, y, line, area.width);
        }
    }
}

/// Bounds-checked `Buffer::set_line`: resize races can leave a momentarily
/// out-of-bounds rect — skip the write instead of panicking.
fn set_line_safe(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, line: &Line<'_>, width: u16) {
    if y < buf.area.bottom() && x < buf.area.right() {
        buf.set_line(x, y, line, width);
    }
}

/// User rows, grok style: `❯ ` accent prefix + primary-colored body on a
/// full-width elevated band; continuation lines keep a 2-col indent.
fn push_user_block(out: &mut Vec<Line<'static>>, text: &str, width: usize) {
    for (i, para) in text.split('\n').enumerate() {
        let owned;
        let (prefix, body) = if i == 0 {
            (Some(USER_PREFIX), para)
        } else {
            (None, {
                owned = format!("{USER_BAND_INDENT}{para}");
                owned.as_str()
            })
        };
        let prefix_width = prefix
            .map(unicode_width::UnicodeWidthStr::width)
            .unwrap_or(0);
        let mut first = true;
        for row in wrap_str(body, width.saturating_sub(prefix_width).max(1)) {
            let mut spans = Vec::with_capacity(3);
            if first && let Some(prefix) = prefix {
                spans.push(Span::styled(prefix.to_string(), USER_STYLE));
            }
            spans.push(Span::styled(row.clone(), USER_BODY_STYLE));
            first = false;
            let row_width = if prefix.is_some() && spans.len() > 1 {
                prefix_width
            } else {
                0
            } + unicode_width::UnicodeWidthStr::width(row.as_str());
            let pad = width.saturating_sub(row_width);
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), BAND_STYLE));
            }
            out.push(Line::from(spans));
        }
    }
}

/// Restyle a rendered feed line as selected (issue #33): the line's spans are
/// flattened and the whole row carries the selection background. Highlight
/// only — no copy yet.
pub fn highlight_line(line: &mut Line<'static>) {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    line.spans = vec![Span::styled(text, BAND_STYLE)];
}

/// Truncate a styled line to `max` display columns, dropping trailing spans
/// and slicing the final span (single-line tool rows never wrap).
fn truncate_line(line: &mut Line<'static>, max: usize) {
    let mut total = 0usize;
    let mut keep: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    for span in std::mem::take(&mut line.spans) {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        if total + w <= max {
            total += w;
            keep.push(span);
        } else {
            let budget = max.saturating_sub(total);
            if budget == 0 {
                break;
            }
            let cut = theway_pager_render::line_utils::byte_offset_at_width(&span.content, budget);
            if let Some(s) = span.content.get(..cut).filter(|s| !s.is_empty()) {
                keep.push(Span::styled(s.to_string(), span.style));
            }
            break;
        }
    }
    line.spans = keep;
}

/// Thinking peek window: dim header with the char count, the last few
/// wrapped lines, and a mode hint.
fn push_thinking_peek(out: &mut Vec<Line<'static>>, text: &str, width: usize) {
    let chars = text.chars().count();
    out.push(Line::styled(
        format!("{TOOL_PREFIX}thinking · {chars} chars"),
        RESULT_SUMMARY_STYLE,
    ));
    let wrapped: Vec<String> = text
        .split('\n')
        .flat_map(|para| wrap_str(para, width))
        .collect();
    let shown = wrapped.iter().rev().take(THINKING_PEEK_LINES).rev();
    for row in shown {
        out.push(Line::styled(format!("  {row}"), THINKING_STYLE));
    }
    if wrapped.len() > THINKING_PEEK_LINES {
        out.push(Line::styled(
            "  … Ctrl+O cycles: hidden/peek/full",
            RESULT_SUMMARY_STYLE,
        ));
    }
}

/// Underline URLs detected in the rendered lines (theway-pager-render osc8
/// detection). Actual OSC 8 hyperlink sequence output needs the inline
/// terminal (Phase 4, `theway-ratatui-inline`); until then URLs get the
/// underline affordance only.
///
/// Assistant rows are skipped: their underlines come from the markdown
/// renderer's `hyperlinks` output instead of this regex scan.
fn underline_links(lines: &mut [Line<'static>], skip: &[std::ops::Range<usize>]) {
    use theway_pager_render::osc8::{LinkOverlay, scan_lines_for_url_overlays};
    let mut overlay = LinkOverlay::new();
    scan_lines_for_url_overlays(
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| (i as u16, line, None)),
        0,
        &[],
        &mut overlay,
    );
    for link in overlay.links() {
        let row = link.screen_row as usize;
        if skip.iter().any(|range| range.contains(&row)) {
            continue;
        }
        if let Some(line) = lines.get_mut(row) {
            underline_range(line, link.col_start as usize, link.col_end as usize);
        }
    }
}

/// Split a line's spans so the display-column range `[start_col, end_col)`
/// carries the underline modifier; the rest of the line is untouched.
fn underline_range(line: &mut Line<'static>, start_col: usize, end_col: usize) {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    for span in std::mem::take(&mut line.spans) {
        let width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_end = col + width;
        if span_end <= start_col || col >= end_col {
            out.push(span);
        } else {
            let cut_start = start_col.max(col) - col;
            let cut_end = end_col.min(span_end) - col;
            let pre = span
                .content
                .get(
                    ..theway_pager_render::line_utils::byte_offset_at_width(
                        &span.content,
                        cut_start,
                    ),
                )
                .filter(|s| !s.is_empty())
                .map(|s| Span::styled(s.to_string(), span.style));
            let mid = span
                .content
                .get(
                    theway_pager_render::line_utils::byte_offset_at_width(&span.content, cut_start)
                        ..theway_pager_render::line_utils::byte_offset_at_width(
                            &span.content,
                            cut_end,
                        ),
                )
                .map(|s| {
                    Span::styled(s.to_string(), span.style.add_modifier(Modifier::UNDERLINED))
                });
            let post = span
                .content
                .get(
                    theway_pager_render::line_utils::byte_offset_at_width(&span.content, cut_end)..,
                )
                .filter(|s| !s.is_empty())
                .map(|s| Span::styled(s.to_string(), span.style));
            if let Some(p) = pre {
                out.push(p);
            }
            if let Some(m) = mid {
                out.push(m);
            }
            if let Some(p) = post {
                out.push(p);
            }
        }
        col = span_end;
    }
    line.spans = out;
}

#[cfg(test)]
mod tests {
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
        assert!(peek.contains("⏵ thinking · 28 chars"), "{peek}");
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
        let feed = feed_with(&[WireFeedBlock::Tool {
            name: "read".into(),
            args: "(path=\"x\")".into(),
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
    fn tool_result_collapses_by_default_and_expands() {
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines: vec!["line a".into(), "line b".into()],
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        assert!(collapsed.contains("result · 2 lines"), "{collapsed}");
        assert!(!collapsed.contains("line a"), "{collapsed}");
        let expanded = flat(&super::lines(
            &feed,
            80,
            &FeedRenderOptions {
                thinking_mode: ThinkingMode::Full,
                tools_expanded: true,
            },
        ));
        assert!(expanded.contains("    line a"), "{expanded}");
        assert!(expanded.contains("    line b"), "{expanded}");
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
    fn highlight_line_flattens_spans_and_sets_band_bg() {
        let mut line = Line::from(vec![
            Span::styled("abc", Style::default().fg(Color::Red)),
            Span::styled("def", Style::default().fg(Color::Green)),
        ]);
        highlight_line(&mut line);
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content, "abcdef");
        assert_eq!(line.spans[0].style.bg, Some(BG_HIGHLIGHT));
    }

    #[test]
    fn assistant_prefix_is_magenta() {
        let feed = feed_with(&[WireFeedBlock::Assistant {
            text: "plain answer".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        assert!(lines[0].spans[0].content.starts_with("ai ▸ "));
        assert_eq!(lines[0].spans[0].style.fg, Some(ACCENT_ASSISTANT));
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
}
