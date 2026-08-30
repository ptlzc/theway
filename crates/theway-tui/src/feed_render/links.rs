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
