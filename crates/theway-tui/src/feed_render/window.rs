/// Character-level text selection over the rendered feed. Columns are
/// 0-based display columns inside the feed pane (`area.x`-relative); `end_col`
/// is exclusive. Use [`usize::MAX`] for an open-ended selection to the end of
/// a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextSelection {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

/// Draw pre-wrapped lines into the visible window only (O(viewport)) — the
/// cache-friendly replacement for `Paragraph::new(lines).scroll(...)` (issue
/// #34). Rows outside the window are never touched, and the area is cleared
/// first so a shrinking feed cannot leave stale cells behind.
///
/// `selection` paints the selected character columns with [`SELECTION_BG`] —
/// the fg/modifiers of each cell are preserved, only the background is
/// overlaid, so the selection reads as a highlight band over any block
/// styling.
pub fn render_lines_window(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    lines: &[Line<'static>],
    offset: usize,
    selection: Option<TextSelection>,
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
        set_line_safe(buf, area.x, y, line, area.width);
        if let Some(sel) = selection
            && sel.start_line <= i
            && i <= sel.end_line
        {
            let (start_col, end_col) = if sel.start_line == sel.end_line {
                (sel.start_col, sel.end_col)
            } else if i == sel.start_line {
                (sel.start_col, usize::MAX)
            } else if i == sel.end_line {
                (0, sel.end_col)
            } else {
                (0, usize::MAX)
            };
            let width = line_width(line);
            let start_x = start_col.min(width).min(area.width as usize);
            let end_x = end_col.min(width).min(area.width as usize);
            if end_x > start_x {
                for x in area.x + start_x as u16..area.x + end_x as u16 {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.bg = SELECTION_BG;
                    }
                }
            }
        }
    }
}

/// Text of a character-level selection, joining selected rows with `\n` and
/// trimming each row's trailing padding.
pub(crate) fn selection_text(
    lines: &[Line<'static>],
    selection: TextSelection,
) -> String {
    if lines.is_empty() || selection.start_line >= lines.len() {
        return String::new();
    }
    let end_line = selection.end_line.min(lines.len() - 1);
    let mut parts = Vec::new();
    for (i, line) in lines
        .iter()
        .enumerate()
        .take(end_line.saturating_add(1))
        .skip(selection.start_line)
    {
        let part = if i == selection.start_line {
            if selection.start_line == end_line {
                slice_line_by_columns(line, selection.start_col, selection.end_col)
            } else {
                slice_line_by_columns(line, selection.start_col, usize::MAX)
            }
        } else if i == end_line {
            slice_line_by_columns(line, 0, selection.end_col)
        } else {
            slice_line_by_columns(line, 0, usize::MAX)
        };
        parts.push(part.trim_end().to_string());
    }
    parts.join("\n")
}

/// Display width of a rendered line (sum of every span's Unicode width).
fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Slice a rendered line by display column range, keeping whole Unicode
/// characters (wide chars are never cut in half). `start_col` is inclusive,
/// `end_col` exclusive; `usize::MAX` means “to the end of the line”.
fn slice_line_by_columns(line: &Line<'_>, start_col: usize, end_col: usize) -> String {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let mut chars: Vec<(char, usize, usize)> = Vec::new();
    let mut col = 0usize;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        let start = col;
        col += w;
        chars.push((ch, start, col));
    }
    let start_idx = chars
        .iter()
        .position(|(_, _, end)| *end > start_col)
        .unwrap_or(chars.len());
    let end_idx = chars
        .iter()
        .position(|(_, start, _)| *start >= end_col)
        .unwrap_or(chars.len());
    chars[start_idx..end_idx]
        .iter()
        .map(|(ch, _, _)| *ch)
        .collect()
}

/// Background color for the mouse-selected feed rows (grok tokyonight
/// accent-blue, `prompt_chrome::BORDER_FOCUSED`).
const SELECTION_BG: Color = Color::Rgb(75, 92, 140);

/// Bounds-checked `Buffer::set_line`: resize races can leave a momentarily
/// out-of-bounds rect — skip the write instead of panicking.
fn set_line_safe(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, line: &Line<'_>, width: u16) {
    if y < buf.area.bottom() && x < buf.area.right() {
        buf.set_line(x, y, line, width);
    }
}

/// User rows, grok style: `❯ ` accent prefix + primary-colored body on a
/// full-width elevated band (the band color is the `user_bg` theme role,
/// overridable per-block via `[blocks.user] bg`); continuation lines keep a
/// 2-col indent.
fn push_user_block(out: &mut Vec<Line<'static>>, text: &str, width: usize, theme: &Theme) {
    let band_bg = theme.user.bg.unwrap_or(theme.user_bg);
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
                // The whole user row is one colored band: prefix and body
                // spans carry the same background, not just the padding.
                spans.push(Span::styled(prefix.to_string(), USER_STYLE.bg(band_bg)));
            }
            spans.push(Span::styled(
                row.clone(),
                user_body_style(theme, band_bg),
            ));
            first = false;
            let row_width = if prefix.is_some() && spans.len() > 1 {
                prefix_width
            } else {
                0
            } + unicode_width::UnicodeWidthStr::width(row.as_str());
            let pad = width.saturating_sub(row_width);
            if pad > 0 {
                spans.push(Span::styled(
                    " ".repeat(pad),
                    Style::new().bg(band_bg),
                ));
            }
            out.push(Line::from(spans));
        }
    }
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

/// Human-format a count for stats lines: raw below 1000, otherwise `k` with
/// one decimal (`1.2k`, `100.2k`).
fn human_count(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

/// Build the thinking stats line for `chars` characters of thinking text:
/// char count (human format) on the left, `c/s` throughput + last-turn
/// input/output tokens on the right, right aligned to the content width.
/// Shared by the one-shot renderer and the streaming cache so both stay
/// byte-identical.
pub(crate) fn thinking_stats_line(
    chars: usize,
    opts: &FeedRenderOptions,
    width: usize,
) -> Line<'static> {
    let left = format!("{TOOL_PREFIX}thinking · {} char", human_count(chars as u64));
    let cps = opts.thinking_cps.round() as u64;
    let right = format!(
        "c/s: {} · in: {} · out: {}",
        cps,
        human_count(opts.thinking_input_tokens),
        human_count(opts.thinking_output_tokens)
    );
    let left_w = unicode_width::UnicodeWidthStr::width(left.as_str());
    let right_w = unicode_width::UnicodeWidthStr::width(right.as_str());
    let pad = width.saturating_sub(left_w + right_w).max(1);
    let mut line = Line::from(vec![
        Span::styled(left, RESULT_SUMMARY_STYLE),
        Span::styled(" ".repeat(pad), RESULT_SUMMARY_STYLE),
        Span::styled(right, RESULT_SUMMARY_STYLE),
    ]);
    truncate_line(&mut line, width);
    line
}

/// Push the thinking stats line for `text`. Hidden mode never renders it.
fn push_thinking_stats_line(
    out: &mut Vec<Line<'static>>,
    text: &str,
    opts: &FeedRenderOptions,
    width: usize,
) {
    out.push(thinking_stats_line(text.chars().count(), opts, width));
}

/// Thinking peek window: stats-line header, the last few wrapped lines, and a
/// mode hint.
fn push_thinking_peek(
    out: &mut Vec<Line<'static>>,
    text: &str,
    opts: &FeedRenderOptions,
    width: usize,
) {
    push_thinking_stats_line(out, text, opts, width);
    let wrapped: Vec<String> = text
        .split('\n')
        .flat_map(|para| wrap_str(para, width))
        .collect();
    let shown = wrapped.iter().rev().take(THINKING_PEEK_LINES).rev();
    for row in shown {
        out.push(Line::styled(
            format!("  {row}"),
            thinking_style(&opts.theme),
        ));
    }
    if wrapped.len() > THINKING_PEEK_LINES {
        out.push(Line::styled(
            "  … Ctrl+O cycles: hidden/peek/full",
            RESULT_SUMMARY_STYLE,
        ));
    }
}

/// Collapsed tool result: a bordered preview of the first
/// [`TOOL_RESULT_PREVIEW_LINES`] lines plus an `…(N more lines)` elision row
/// when the result is taller. Full expansion (Ctrl+T) keeps the whole body.
/// The left `│` bar is drawn in the block's border color (dimmer than the
/// body text) and sits flush at the content edge.
fn push_tool_result_preview(
    out: &mut Vec<Line<'static>>,
    lines: &[String],
    is_error: bool,
    width: usize,
    theme: &Theme,
) {
    let style = if is_error {
        Style::default().fg(theme.tool_error)
    } else {
        Style::default().fg(theme.tool_result)
    };
    let border_style = Style::default().fg(theme.tool.border_style);
    let border_w = unicode_width::UnicodeWidthStr::width(TOOL_RESULT_BORDER);
    let content_w = width.saturating_sub(border_w).max(1);
    let preview_n = lines.len().min(TOOL_RESULT_PREVIEW_LINES);
    for line in &lines[..preview_n] {
        for row in wrap_str(line, content_w) {
            out.push(Line::from(vec![
                Span::styled(TOOL_RESULT_BORDER, border_style),
                Span::styled(row, style),
            ]));
        }
    }
    if lines.len() > TOOL_RESULT_PREVIEW_LINES {
        let more = lines.len() - TOOL_RESULT_PREVIEW_LINES;
        out.push(Line::from(vec![
            Span::styled(TOOL_RESULT_BORDER, border_style),
            Span::styled(format!("…({more} more lines)"), RESULT_SUMMARY_STYLE),
        ]));
    }
}

/// Expanded tool result (issue #41): non-fence lines render exactly as
/// before (`  `-indented, width-wrapped, result-colored); a ```mermaid
/// fence routes its body through the markdown mermaid render path into a
/// box-and-arrow diagram. The fence lines themselves are consumed by the
/// diagram, matching pretty-mode markdown. The 2-space indent keeps the
/// expanded body at the same left offset as the preview content (behind the
/// `│` bar).
fn push_tool_result_expanded(
    out: &mut Vec<Line<'static>>,
    lines: &[String],
    width: usize,
    style: Style,
) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().starts_with("```mermaid") {
            let mut body: Vec<&str> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().starts_with("```") {
                body.push(lines[j].as_str());
                j += 1;
            }
            // `j` is the closing fence index, or `lines.len()` when unclosed
            // (the rest of the block is the body, like a truncated fence).
            push_mermaid_diagram(out, &body.join("\n"), style, width);
            i = j + 1;
            continue;
        }
        for row in wrap_str(&format!("  {}", lines[i]), width) {
            out.push(Line::styled(row, style));
        }
        i += 1;
    }
}

/// Render a mermaid body through [`theway_markdown::render_mermaid_art`] and
/// push the diagram lines, recolored to the tool result `style`; blank
/// bodies fall back to the ordinary text rows. Over-wide or unsupported
/// sources render as the renderer's framed source box (markdown parity).
fn push_mermaid_diagram(out: &mut Vec<Line<'static>>, src: &str, style: Style, width: usize) {
    let Some(mut art) = theway_markdown::render_mermaid_art(
        src,
        &theway_markdown::MermaidStyles::default(),
        Some(width),
    ) else {
        for row in wrap_str(src, width) {
            out.push(Line::styled(row, style));
        }
        return;
    };
    if let Some(fg) = style.fg {
        for line in &mut art.styled_lines {
            for span in &mut line.spans {
                if span.style.fg.is_none() {
                    span.style = span.style.fg(fg);
                }
            }
        }
    }
    out.extend(art.styled_lines);
}
