//! Character-level feed text selection (issue #53).
//!
//! The 2D selection model shared by the feed renderer and the input surface:
//! [`FeedSelection`] stores *uncapped* rendered-line indices and *display
//! columns* (not byte offsets). Column math always clamps to the row's text
//! width via [`line_text_width`] (unicode-width) — terminal semantics: a
//! pointer or key press past the row end lands on the row end. Painting and
//! text extraction share the same column range, so exactly what is
//! highlighted is what gets copied.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Selection highlight background (grok tokyonight `BG_HIGHLIGHT`, the same
/// color as the user-band default in `feed_render` — not a theme role).
pub(crate) const BAND_STYLE: Style = Style::new().bg(Color::Rgb(41, 46, 66));

/// Character-level feed selection: `(line, column)` pairs in uncapped
/// rendered-line coordinates and display columns (NOT byte offsets).
/// Columns clamp to each row's text width at every use site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedSelection {
    /// Where the selection started (mouse-down / Ctrl+Space).
    pub anchor: (usize, usize),
    /// The free end (mouse drag / Shift+arrows).
    pub head: (usize, usize),
}

impl FeedSelection {
    /// Direction-normalized bounds: `(start, end)` with `start <= end`
    /// (rows first, then columns — a backward drag within one row also
    /// normalizes).
    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Map the uncapped row coordinates onto a retained `lines` slice whose
    /// first `trimmed` rows were dropped by the feed cache's head trim.
    /// `None` when the whole selection was trimmed away (or the slice is
    /// empty). Columns pass through — each row clamps them to its own width.
    pub fn to_capped(self, trimmed: usize, capped_total: usize) -> Option<FeedSelection> {
        if capped_total == 0 {
            return None;
        }
        let (start, end) = self.ordered();
        if end.0 < trimmed {
            return None;
        }
        let max_row = capped_total - 1;
        Some(FeedSelection {
            anchor: (start.0.saturating_sub(trimmed).min(max_row), start.1),
            head: (end.0.saturating_sub(trimmed).min(max_row), end.1),
        })
    }

    /// Display-column range to paint on `row` (a capped line index, matching
    /// the `lines` slice handed to the renderer), clamped to the row's text
    /// width: `[0, width)` for interior rows, `[c1, width)` on the start
    /// row, `[0, c2)` on the end row, `(0, 0)` outside the selection.
    pub fn paint_cols(&self, row: usize, line: &Line<'_>) -> (usize, usize) {
        let (start, end) = self.ordered();
        if row < start.0 || row > end.0 {
            return (0, 0);
        }
        let width = line_text_width(line);
        let c1 = if row == start.0 { start.1.min(width) } else { 0 };
        let c2 = if row == end.0 { end.1.min(width) } else { width };
        (c1.min(c2), c1.max(c2))
    }
}

/// Display width of a rendered line's *text*: spans are counted up to the
/// last non-whitespace span — trailing padding/filler spans (user band,
/// block layout padding) are excluded, so selections clamp to the row end,
/// not the padded band width (issue #53).
pub(crate) fn line_text_width(line: &Line<'_>) -> usize {
    let mut total = 0usize;
    let mut pending_ws = 0usize;
    for span in &line.spans {
        let w = UnicodeWidthStr::width(span.content.as_ref());
        if w == 0 {
            continue;
        }
        if span.content.chars().all(char::is_whitespace) {
            pending_ws += w;
        } else {
            total += pending_ws + w;
            pending_ws = 0;
        }
    }
    total
}

/// Plain text of the selection: rows joined with `\n`, the start row cut at
/// its start column and the end row at its end column (interior rows in
/// full). Selection rows index into `lines` in capped coordinates — callers
/// map uncapped rows via [`FeedSelection::to_capped`].
pub(crate) fn extract_text(lines: &[Line<'static>], sel: &FeedSelection) -> String {
    let (start, end) = sel.ordered();
    if start.0 >= lines.len() {
        return String::new();
    }
    let last = end.0.min(lines.len() - 1);
    let mut out = String::new();
    for (row, line) in lines.iter().enumerate().take(last + 1).skip(start.0) {
        if row > start.0 {
            out.push('\n');
        }
        let (c1, c2) = sel.paint_cols(row, line);
        out.push_str(&line_slice(line, c1, c2));
    }
    out
}

/// Cut the `[c1, c2)` display-column slice of a rendered line into plain
/// text (columns clamp to the line's text width; byte offsets follow
/// unicode-width).
fn line_slice(line: &Line<'_>, c1: usize, c2: usize) -> String {
    let width = line_text_width(line);
    let c1 = c1.min(width);
    let c2 = c2.min(width);
    if c1 >= c2 {
        return String::new();
    }
    let mut out = String::new();
    let mut pos = 0usize;
    for span in &line.spans {
        let w = UnicodeWidthStr::width(span.content.as_ref());
        if w == 0 {
            continue;
        }
        let span_end = pos + w;
        if span_end <= c1 {
            pos = span_end;
            continue;
        }
        if pos >= c2 {
            break;
        }
        let cut_start =
            theway_pager_render::line_utils::byte_offset_at_width(&span.content, c1.saturating_sub(pos));
        let cut_end =
            theway_pager_render::line_utils::byte_offset_at_width(&span.content, c2.saturating_sub(pos));
        out.push_str(&span.content[cut_start..cut_end]);
        pos = span_end;
    }
    out
}

/// Paint `line` into `buf` at `(x, y)` with the selection background on
/// ONLY the `[c1, c2)` display columns (issue #53): the spans are split at
/// the column boundaries and [`BAND_STYLE`] is patched onto the selected
/// slice — the rest of the row keeps its own styles, and the selection
/// never covers the whole line or the screen width. Bounds-checked: a
/// resize race skips the write instead of panicking.
pub(crate) fn highlight_cols(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    line: &Line<'static>,
    c1: usize,
    c2: usize,
) {
    let width = line_text_width(line);
    let c1 = c1.min(width);
    let c2 = c2.min(width);
    if c1 >= c2 || y >= buf.area.bottom() || x >= buf.area.right() {
        return;
    }
    let painted = paint_slice(line, c1, c2);
    buf.set_line(x, y, &painted, buf.area.width.saturating_sub(x));
}

/// Split `line` at the display columns `c1`/`c2` and patch [`BAND_STYLE`]
/// onto the spans inside the slice.
fn paint_slice(line: &Line<'static>, c1: usize, c2: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
    let mut pos = 0usize;
    for span in &line.spans {
        let w = UnicodeWidthStr::width(span.content.as_ref());
        if w == 0 {
            continue;
        }
        let span_end = pos + w;
        if span_end <= c1 || pos >= c2 {
            spans.push(span.clone());
        } else {
            let cut_start =
                theway_pager_render::line_utils::byte_offset_at_width(&span.content, c1.saturating_sub(pos));
            let cut_end =
                theway_pager_render::line_utils::byte_offset_at_width(&span.content, c2.saturating_sub(pos));
            if cut_start > 0 {
                spans.push(Span::styled(
                    span.content[..cut_start].to_string(),
                    span.style,
                ));
            }
            if cut_end > cut_start {
                spans.push(Span::styled(
                    span.content[cut_start..cut_end].to_string(),
                    span.style.patch(BAND_STYLE),
                ));
            }
            if cut_end < span.content.len() {
                spans.push(Span::styled(
                    span.content[cut_end..].to_string(),
                    span.style,
                ));
            }
        }
        pos = span_end;
    }
    Line {
        style: line.style,
        alignment: line.alignment,
        spans,
    }
}
