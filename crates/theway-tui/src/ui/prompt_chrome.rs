//! Grok-style input box chrome (issue #28).
//!
//! Ported from `xai-grok-pager/src/views/prompt_widget/mod.rs` (`draw()` /
//! `render_info_line()` / `PromptStyle`) — only the *visual chrome* layer:
//! rounded border `╭─╮│╰─╯`, the `❯` prefix, and the bottom info line with
//! the model name + flags. The textarea widget itself renders into the
//! content rect this module returns; all key handling stays theway's.
//!
//! Colors are the TokyoNight values from `xai-grok-pager-render`'s theme
//! (`theme/tokyonight.rs`) — prompt_border(_active), accent_user, gray_dim,
//! gray, text_secondary, bg_base.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// `prompt_border_active` — brighter chrome when focused.
pub const BORDER_FOCUSED: Color = Color::Rgb(75, 92, 140);
/// `prompt_border` — dimmer chrome when unfocused (picker / control prompt open).
pub const BORDER_UNFOCUSED: Color = Color::Rgb(60, 75, 120);
/// `accent_user` (BLUE) — the focused `❯` prefix color.
pub const ACCENT_USER: Color = Color::Rgb(122, 162, 247);
/// `gray_dim` (FG_GUTTER) — unfocused prefix, info separators.
pub const GRAY_DIM: Color = Color::Rgb(59, 66, 97);
/// `gray` (COMMENT) — placeholder, info flags.
pub const GRAY: Color = Color::Rgb(86, 95, 137);
/// `text_secondary` (FG_DARK) — info-line model name (blended toward bg).
pub const TEXT_SECONDARY: Color = Color::Rgb(169, 177, 214);
/// `text_primary` (FG) — content text color.
pub const TEXT_PRIMARY: Color = Color::Rgb(192, 202, 245);
/// `bg_base` (BG_STORM) — the prompt's background surface.
pub const BG_BASE: Color = Color::Rgb(36, 40, 59);

/// The `❯ ` prompt prefix (2 columns, matching grok's `prompt_arrow`).
pub const PREFIX: &str = "\u{276F} ";
const PREFIX_WIDTH: u16 = 2;

/// Horizontal content padding inside the chrome (grok's
/// `chrome_pad_left` / `chrome_pad_right`).
const PAD_LEFT: u16 = 2;
const PAD_RIGHT: u16 = 1;

/// A flag displayed on the info line (e.g. "working", "2 queued").
pub struct PromptFlag<'a> {
    pub text: &'a str,
    pub color: Color,
    pub bold: bool,
}

/// Everything the chrome needs to draw the input box frame.
pub struct PromptChrome<'a> {
    /// Focused: picker/control-plane prompt closed. Toggles border + prefix color.
    pub focused: bool,
    /// Model label shown left on the info line (e.g. `grok-3`).
    pub model_name: &'a str,
    /// Flags appended after the model name, joined by " · ".
    pub flags: &'a [PromptFlag<'a>],
    /// Right-aligned "multiline" indicator (grok shows it when the draft has \n).
    pub multiline: bool,
    /// Optional session title inlined into the top divider (agent view in grok).
    pub title: Option<&'a str>,
    /// Placeholder shown when the draft is empty. Grok hides it while focused;
    /// theway's box is almost always focused, so it only shows when unfocused.
    pub placeholder: &'a str,
    /// Whether the textarea draft is empty (placeholder gate).
    pub input_empty: bool,
}

impl Default for PromptChrome<'_> {
    fn default() -> Self {
        Self {
            focused: true,
            model_name: "",
            flags: &[],
            multiline: false,
            title: None,
            placeholder: "Build anything",
            input_empty: true,
        }
    }
}

/// Draw the grok-style chrome into `area` and return the rect the textarea
/// renders into (inside the border, past the `❯` prefix).
///
/// Layout (mirrors grok's `draw()` with `chrome: true`, `show_accent_line:
/// false`, `vpad_top: 1`):
/// ```text
/// ╭──────────────────────╮   <- top divider (title right-aligned)
/// │ ❯ draft text         │   <- text rows, │ side borders
/// ╰─ model · flags ──────╯   <- info line (multiline right-aligned)
/// ```
/// Degrades to a plain fill when `area` is too small for the chrome.
pub fn render_prompt_chrome(buf: &mut Buffer, area: Rect, c: &PromptChrome) -> Rect {
    if area.width < 4 || area.height < 2 {
        buf.set_style(area, Style::default().fg(TEXT_PRIMARY).bg(BG_BASE));
        return area;
    }
    let bg = BG_BASE;
    let border_color = if c.focused {
        BORDER_FOCUSED
    } else {
        BORDER_UNFOCUSED
    };
    let div_style = Style::default().fg(border_color).bg(bg);

    // Fill the whole area so every cell has RGB colors (grok does the same
    // for later blending); the textarea overpaints the content cells.
    buf.set_style(area, Style::default().fg(TEXT_PRIMARY).bg(bg));

    let content_x = area.x + PAD_LEFT;
    let content_w = area.width.saturating_sub(PAD_LEFT + PAD_RIGHT);
    let text_y = area.y + 1;
    let text_h = area.height.saturating_sub(2);

    // ── Top divider: ╭──────╮ ─────────────────────────────────────────────────
    let top_y = area.y;
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, top_y)) {
            let ch = if x == area.x {
                '\u{256d}' // ╭
            } else if x == area.right().saturating_sub(1) {
                '\u{256e}' // ╮
            } else {
                '\u{2500}' // ─
            };
            cell.set_char(ch);
            cell.set_style(div_style);
        }
    }

    // Session title inlined in the divider (` title `, right-aligned ending
    // 2 cells before ╮), in the shared chrome-caption style.
    if let Some(title) = c.title.map(str::trim).filter(|t| !t.is_empty()) {
        let max_w = area.width.saturating_sub(6);
        if max_w >= 6 {
            let label = format!(" {title} ");
            let truncated = truncate_str(&label, max_w as usize);
            let label_w = UnicodeWidthStr::width(truncated.as_str()) as u16;
            let x = area.x + area.width.saturating_sub(3 + label_w);
            buf.set_string(x, top_y, &truncated, caption_style(bg, c.focused));
        }
    }

    // ── ❯ prefix on the first text row ───────────────────────────────────────
    let prefix_color = if c.focused { ACCENT_USER } else { GRAY_DIM };
    if text_h > 0 && content_w > PREFIX_WIDTH {
        buf.set_string(
            content_x,
            text_y,
            PREFIX,
            Style::default().fg(prefix_color).bg(bg),
        );
    }

    // ── Side borders: │ on each text row ─────────────────────────────────────
    for y in text_y..text_y + text_h {
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_char('\u{2502}'); // │
            cell.set_style(div_style);
        }
        if let Some(cell) = buf.cell_mut((area.right().saturating_sub(1), y)) {
            cell.set_char('\u{2502}');
            cell.set_style(div_style);
        }
    }

    // ── Placeholder when the draft is empty and the box is unfocused ─────────
    let ta_x = content_x + PREFIX_WIDTH;
    let ta_w = content_w.saturating_sub(PREFIX_WIDTH);
    if c.input_empty && !c.focused && ta_w > 0 && text_h > 0 {
        let truncated = truncate_str(c.placeholder, ta_w as usize);
        buf.set_string(ta_x, text_y, &truncated, Style::default().fg(GRAY).bg(bg));
    }

    // ── Bottom divider + info line: ╰─ model · flags ─╯ ───────────────────────
    let div_y = area.bottom().saturating_sub(1);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, div_y)) {
            let ch = if x == area.x {
                '\u{2570}' // ╰
            } else if x == area.right().saturating_sub(1) {
                '\u{256f}' // ╯
            } else {
                '\u{2500}' // ─
            };
            cell.set_char(ch);
            cell.set_style(div_style);
        }
    }
    let info_rect = Rect {
        x: content_x,
        y: div_y,
        width: content_w,
        height: 1,
    };
    render_info_line(buf, info_rect, c);

    Rect {
        x: ta_x,
        y: text_y,
        width: ta_w,
        height: text_h,
    }
}

/// Caption style for text embedded in the border chrome (info-line model
/// name, divider title): secondary text over the prompt bg, fading further
/// when unfocused — grok's `chrome_caption_style`.
fn caption_style(bg: Color, focused: bool) -> Style {
    let opacity = if focused { 0.6 } else { 0.4 };
    let fg = theway_pager_render::color::blend_color(bg, TEXT_SECONDARY, opacity).unwrap_or(GRAY);
    Style::default().fg(fg).bg(bg)
}

/// Render the info line on the bottom divider: left-aligned
/// ` model · flag1 · flag2`, right-aligned `multiline`. Mirrors grok's
/// `render_info_line` (right-edge anchored, 1-cell padding from the corners).
fn render_info_line(buf: &mut Buffer, area: Rect, c: &PromptChrome) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let bg = BG_BASE;
    let sep_style = Style::default().fg(GRAY_DIM).bg(bg);
    let flag_style = Style::default().fg(GRAY).bg(bg);
    let model_style = caption_style(bg, c.focused);

    // Left side: model name + flags, wrapped in padding spaces so the cells
    // next to ╰ / ╯ are blanked out instead of showing `─`.
    let mut left_spans: Vec<Span<'static>> = vec![Span::styled(" ", Style::default().bg(bg))];
    left_spans.push(Span::styled(c.model_name.to_owned(), model_style));
    for flag in c.flags {
        left_spans.push(Span::styled(" · ", sep_style));
        let mut style = Style::default().fg(flag.color).bg(bg);
        if flag.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        left_spans.push(Span::styled(flag.text.to_owned(), style));
    }
    left_spans.push(Span::styled(" ", Style::default().bg(bg)));

    // Right side: "multiline" indicator.
    let mut right_spans: Vec<Span<'static>> = Vec::new();
    if c.multiline {
        right_spans.push(Span::styled("multiline", flag_style));
    }

    let left_line = Line::from(left_spans);
    if !right_spans.is_empty() {
        right_spans.push(Span::styled(" ", Style::default().bg(bg)));
        let right_line = Line::from(right_spans);
        let right_w = right_line.width() as u16;
        let left_w = (left_line.width() as u16).min(area.width.saturating_sub(right_w + 1));
        let total_w = left_w + 1 + right_w; // 1 for the gap
        let x = area.x + area.width.saturating_sub(total_w);
        set_line_safe(buf, x, area.y, &left_line, left_w);
        let rx = area.x + area.width.saturating_sub(right_w);
        set_line_safe(buf, rx, area.y, &right_line, right_w);
    } else {
        let text_w = (left_line.width() as u16).min(area.width);
        let x = area.x + area.width.saturating_sub(text_w);
        set_line_safe(buf, x, area.y, &left_line, text_w);
    }
}

/// Truncate to `max` display columns (pager-render's line_utils).
fn truncate_str(text: &str, max: usize) -> String {
    theway_pager_render::line_utils::truncate_str(text, max)
}

/// Bounds-checked `Buffer::set_line` (grok's `SafeBuf::set_line_safe`):
/// resize races can leave a momentarily out-of-bounds rect — skip the write
/// instead of panicking, one missed frame is fine.
fn set_line_safe(buf: &mut Buffer, x: u16, y: u16, line: &Line<'_>, width: u16) {
    if y >= buf.area.y && y < buf.area.bottom() && x < buf.area.right() {
        buf.set_line(x, y, line, width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn render(area: Rect, c: &PromptChrome) -> Buffer {
        let mut buf = Buffer::empty(area);
        render_prompt_chrome(&mut buf, area, c);
        buf
    }

    fn cell_str(buf: &Buffer, x: u16, y: u16) -> String {
        buf.cell((x, y))
            .map(|c| c.symbol().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn rounded_border_corners() {
        let area = Rect::new(0, 0, 20, 4);
        let buf = render(area, &PromptChrome::default());
        assert_eq!(cell_str(&buf, 0, 0), "╭");
        assert_eq!(cell_str(&buf, 19, 0), "╮");
        assert_eq!(cell_str(&buf, 0, 3), "╰");
        assert_eq!(cell_str(&buf, 19, 3), "╯");
        assert_eq!(cell_str(&buf, 0, 1), "│");
        assert_eq!(cell_str(&buf, 19, 1), "│");
        assert_eq!(cell_str(&buf, 5, 0), "─");
    }

    #[test]
    fn prefix_drawn_on_first_text_row() {
        let area = Rect::new(0, 0, 20, 4);
        let buf = render(area, &PromptChrome::default());
        assert_eq!(cell_str(&buf, 2, 1), "❯");
        // Color: accent when focused.
        assert_eq!(buf.cell((2, 1)).unwrap().fg, ACCENT_USER);
    }

    #[test]
    fn prefix_dims_when_unfocused() {
        let area = Rect::new(0, 0, 20, 4);
        let c = PromptChrome {
            focused: false,
            ..Default::default()
        };
        let buf = render(area, &c);
        assert_eq!(buf.cell((2, 1)).unwrap().fg, GRAY_DIM);
        assert_eq!(buf.cell((0, 0)).unwrap().fg, BORDER_UNFOCUSED);
        // Placeholder appears when empty + unfocused.
        let row: String = (4..18)
            .filter_map(|x| buf.cell((x, 1)).map(|c| c.symbol()))
            .collect();
        assert!(row.starts_with("Build any"), "placeholder row: {row}");
    }

    #[test]
    fn info_line_has_model_and_flags() {
        let area = Rect::new(0, 0, 30, 4);
        let c = PromptChrome {
            model_name: "grok-3",
            flags: &[PromptFlag {
                text: "working",
                color: Color::Rgb(255, 0, 0),
                bold: true,
            }],
            ..Default::default()
        };
        let buf = render(area, &c);
        let row: String = (2..28)
            .filter_map(|x| buf.cell((x, 3)).map(|c| c.symbol()))
            .collect();
        assert!(row.contains("grok-3"), "info row: {row}");
        assert!(row.contains("working"), "info row: {row}");
    }

    #[test]
    fn info_line_right_aligns_multiline() {
        let area = Rect::new(0, 0, 30, 4);
        let c = PromptChrome {
            model_name: "grok-3",
            multiline: true,
            ..Default::default()
        };
        let buf = render(area, &c);
        // "multiline" right-aligned inside the info rect (x=2, w=27):
        // "multiline " (10 cols) ends at x=29, so text spans x=19..=27.
        assert_eq!(cell_str(&buf, 19, 3), "m");
        assert_eq!(cell_str(&buf, 27, 3), "e");
        assert_eq!(cell_str(&buf, 29, 3), "╯");
    }

    #[test]
    fn content_rect_is_inside_chrome() {
        let area = Rect::new(0, 0, 20, 5);
        let inner = render_prompt_chrome(&mut Buffer::empty(area), area, &PromptChrome::default());
        // x = area.x + PAD_LEFT + PREFIX_WIDTH, y = area.y + 1.
        assert_eq!(inner.x, 4);
        assert_eq!(inner.y, 1);
        assert_eq!(inner.width, 20 - 2 - 1 - 2);
        assert_eq!(inner.height, 3);
    }

    #[test]
    fn tiny_area_degrades_gracefully() {
        let area = Rect::new(0, 0, 3, 2);
        let inner = render_prompt_chrome(&mut Buffer::empty(area), area, &PromptChrome::default());
        assert_eq!(inner, area);
    }
}
