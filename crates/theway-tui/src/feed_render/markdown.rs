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
pub(crate) fn markdown_style(
    color_level: theway_markdown::ColorLevel,
) -> theway_markdown::MarkdownStyle {
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
    .adapt_for(color_level)
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
/// A non-empty `prefix` is prepended to the first rendered line only, in
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
    color_level: theway_markdown::ColorLevel,
) {
    let (rendered, _checkpoint) = theway_markdown::render_markdown_ratatui_full_width(
        text,
        markdown_style(color_level),
        true,
        Some(theway_markdown::default_syntect_with_color_level(
            color_level,
        )),
        Some(width),
    );
    let width = width.max(1);
    for (i, line) in rendered.lines.into_iter().enumerate() {
        push_rendered_markdown_line(
            out,
            i,
            line,
            prefix,
            prefix_style,
            width,
            &rendered.code_blocks,
            &rendered.hyperlinks,
        );
    }
}

/// Process ONE rendered markdown line into width-wrapped, prefixed, underlined
/// `ratatui` lines. Shared by the one-shot [`push_markdown`] path and the
/// streaming tail renderer (`feed_cache`): frozen lines are processed exactly
/// once, unfrozen tail lines once per frame.
pub(crate) fn push_rendered_markdown_line(
    out: &mut Vec<Line<'static>>,
    line_index: usize,
    line: Line<'static>,
    prefix: &str,
    prefix_style: Style,
    width: usize,
    code_blocks: &[theway_markdown::CodeBlockSpan],
    hyperlinks: &[theway_markdown::HyperlinkTarget],
) {
    let width = width.max(1);
    let first = line_index == 0;
    let in_code = code_blocks
        .iter()
        .any(|cb| cb.output_line_range.contains(&line_index));
    let keep = in_code || is_table_line(&line);

    let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let source = if first {
        format!("{prefix}{line_text}")
    } else {
        line_text
    };

    let mapped = if keep || unicode_width::UnicodeWidthStr::width(source.as_str()) <= width {
        let mut line = line;
        if first && !prefix.is_empty() {
            line.spans
                .insert(0, Span::styled(prefix.to_string(), prefix_style));
        }
        let start = out.len();
        out.push(line);
        MappedLine {
            start,
            source: None,
            rows: Vec::new(),
        }
    } else {
        // Re-wrap the line with `wrap_str` semantics but re-apply each
        // span's style through the row byte ranges, so inline styles
        // (bold/italic/code/link) survive the wrap instead of degrading
        // to a single base style.
        let prefix_len = if first { prefix.len() } else { 0 };
        let mut pieces: Vec<(std::ops::Range<usize>, Style)> =
            Vec::with_capacity(line.spans.len() + 1);
        if first && !prefix.is_empty() {
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
        MappedLine {
            start,
            source: Some(source),
            rows,
        }
    };

    // Underline hyperlinks on this line: the renderer reports each link as
    // (pre-wrap line index, display-column range), which maps directly onto
    // verbatim rows and through the byte ranges onto wrapped rows.
    let prefix_width = unicode_width::UnicodeWidthStr::width(prefix);
    for link in hyperlinks
        .iter()
        .filter(|link| link.line_index == line_index)
    {
        let shift = if first { prefix_width } else { 0 };
        let start_col = link.column_range.start + shift;
        let end_col = link.column_range.end + shift;
        match &mapped.source {
            Some(source) => {
                let byte_start =
                    theway_pager_render::line_utils::byte_offset_at_width(source, start_col);
                let byte_end =
                    theway_pager_render::line_utils::byte_offset_at_width(source, end_col);
                for (j, row) in mapped.rows.iter().enumerate() {
                    let overlap = byte_start.max(row.range.start)..byte_end.min(row.range.end);
                    if overlap.start >= overlap.end {
                        continue;
                    }
                    let row_idx = mapped.start + j;
                    let row_start_width =
                        unicode_width::UnicodeWidthStr::width(&source[..row.range.start]);
                    let c0 = unicode_width::UnicodeWidthStr::width(&source[..overlap.start])
                        - row_start_width;
                    let c1 = unicode_width::UnicodeWidthStr::width(&source[..overlap.end])
                        - row_start_width;
                    underline_range(&mut out[row_idx], c0, c1);
                }
            }
            None => underline_range(&mut out[mapped.start], start_col, end_col),
        }
    }
}
