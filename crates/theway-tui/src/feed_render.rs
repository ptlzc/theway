//! Ratatui rendering for the conversation feed (daemon-kernel-layers: the
//! terminal rendering moved from the SDK into the TUI; the UI-agnostic model
//! lives in `theway_transport::feed`).
//!
//! [`lines`] renders a [`Feed`] to width-wrapped, styled `ratatui` lines,
//! ready to scroll/draw — the terminal counterpart of `Feed::plain_lines`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use theway_transport::feed::{Block, Level, display_prefix, should_separate, wrap_str};

const USER_STYLE: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const THINKING_STYLE: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
const TOOL_STYLE: Style = Style::new().fg(Color::Yellow);

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
/// `prefix` (e.g. `ai ▸ `) is prepended to the first rendered line only.
/// Fenced-code body lines (per the renderer's `code_blocks` output ranges)
/// and table rows stay verbatim; every other line wraps with `wrap_str`.
/// Link underlines come from the renderer's `hyperlinks` output — the
/// assistant block does not go through the regex URL scan.
pub fn push_markdown(out: &mut Vec<Line<'static>>, text: &str, prefix: &str, width: usize) {
    let (rendered, _checkpoint) = theway_markdown::render_markdown_ratatui_full(
        text,
        markdown_style(),
        true,
        Some(theway_markdown::default_syntect()),
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
                    .insert(0, Span::styled(prefix.to_string(), Style::default()));
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
                pieces.push((0..prefix_len, Style::default()));
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
pub fn lines(feed: &theway_transport::feed::Feed, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut assistant_rows: Vec<std::ops::Range<usize>> = Vec::new();
    let mut previous: Option<&Block> = None;
    for block in feed.blocks() {
        if should_separate(previous, block, !out.is_empty()) {
            out.push(Line::raw(""));
        }
        match block {
            Block::User { text, timestamp } => {
                let prefix = display_prefix(timestamp.as_deref(), "you ▸ ");
                push_paragraphs(&mut out, text, USER_STYLE, Some(&prefix), width);
            }
            Block::Assistant { text, timestamp } => {
                let prefix = display_prefix(timestamp.as_deref(), "ai ▸ ");
                // Assistant blocks are markdown: one-shot pretty render via
                // theway-markdown, link underlines from the renderer's
                // hyperlinks, verbatim code/table rows, wrapped prose.
                let start = out.len();
                push_markdown(&mut out, text, &prefix, width);
                assistant_rows.push(start..out.len());
            }
            Block::Thinking { text, timestamp } => {
                let prefix = display_prefix(timestamp.as_deref(), "[thinking] ");
                push_paragraphs(&mut out, text, THINKING_STYLE, Some(&prefix), width);
            }
            Block::Tool {
                name,
                args,
                timestamp,
            } => {
                let text = format!("⚙ {name}{args}");
                let prefix = display_prefix(timestamp.as_deref(), "");
                push_paragraphs(&mut out, &text, TOOL_STYLE, Some(&prefix), width);
            }
            Block::ToolResult {
                lines,
                is_error,
                timestamp,
                ..
            } => {
                let style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                };
                let mut first = true;
                for line in lines {
                    let indented = if first {
                        first = false;
                        format!("{}    {line}", display_prefix(timestamp.as_deref(), ""))
                    } else {
                        format!("    {line}")
                    };
                    for row in wrap_str(&indented, width) {
                        out.push(Line::styled(row, style));
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
        previous = Some(block);
    }
    underline_links(&mut out, &assistant_rows);
    out
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
    use super::wrap_str_ranges;

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
