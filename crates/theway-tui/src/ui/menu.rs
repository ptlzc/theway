//! Shared hierarchical-menu building blocks (issue #54/#72): the inline band
//! renderer and the canonical key mapping. The model selector (cascade
//! provider→model→thinking) and the `/side-panel` menu (root→Toggle/Position)
//! both render through [`render_menu_band`] and interpret keys through
//! [`map_menu_key`], so every menu in the TUI shares one look and one set of
//! bindings.

use ratatui::layout::Rect;
use ratatui::style::Style;

use super::theme::{ComposerStyle, PickerStyle};

/// One breadcrumb segment: column/level label + the currently pinned value.
pub(crate) struct MenuCrumb {
    pub label: &'static str,
    pub pinned: String,
}

/// Inline band payload: breadcrumbs for the path, the index of the active
/// crumb, an optional per-row title suffix, and the active level's rows
/// (`(text, is_cursor)`).
pub(crate) struct MenuBandData {
    pub crumbs: Vec<MenuCrumb>,
    pub active: usize,
    /// Column description appended to each choice row (e.g. "thinking
    /// level"). Empty = no suffix.
    pub title: String,
    pub rows: Vec<(String, bool)>,
}

/// Canonical menu key mapping shared by every hierarchical menu handler:
/// vim arrows and cursor keys both work; Esc steps back (or closes at the
/// root), Ctrl-C closes outright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    /// Esc: one level back; at the root the menu closes.
    Back,
    /// Ctrl-C: close the menu from any level.
    Close,
    Other,
}

pub(crate) fn map_menu_key(key: &crossterm::event::KeyEvent) -> MenuKey {
    let ctrl = key
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL);
    match key.code {
        crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => MenuKey::Up,
        crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => MenuKey::Down,
        crossterm::event::KeyCode::Left | crossterm::event::KeyCode::Char('h') => MenuKey::Left,
        crossterm::event::KeyCode::Right | crossterm::event::KeyCode::Char('l') => MenuKey::Right,
        crossterm::event::KeyCode::Enter => MenuKey::Enter,
        crossterm::event::KeyCode::Esc => MenuKey::Back,
        crossterm::event::KeyCode::Char('c') if ctrl => MenuKey::Close,
        _ => MenuKey::Other,
    }
}

/// Render the inline menu band: a horizontal breadcrumb row (the active
/// crumb gets a `❯` marker) with the active level's choice window rendered
/// as a vertical list under it. Transparent background — choices draw over
/// the feed with fg colors only (the composer background stays removed).
pub(crate) fn render_menu_band(
    frame: &mut ratatui::Frame,
    area: Rect,
    data: &MenuBandData,
    picker: &PickerStyle,
    composer: &ComposerStyle,
) {
    frame.render_widget(ratatui::widgets::Clear, area);

    let crumb_w = area.width as usize;
    let crumb_text = data
        .crumbs
        .iter()
        .enumerate()
        .map(|(index, crumb)| {
            let crumb = format!("{} › {}", crumb.label, crumb.pinned);
            if index == data.active {
                format!("❯ {crumb}")
            } else {
                crumb
            }
        })
        .collect::<Vec<_>>()
        .join("   ");
    frame.buffer_mut().set_string(
        area.x,
        area.y,
        theway_transport::feed::truncate_chars(&crumb_text, crumb_w),
        Style::default().fg(composer.info_text),
    );

    let list_y = area.y + 1;
    let list_x = area.x + 2;
    let list_w = area.width.saturating_sub(2) as usize;
    let mut y = list_y;
    for (text, is_cursor) in &data.rows {
        if y >= area.bottom() {
            break;
        }
        let style = if *is_cursor {
            Style::default()
                .fg(picker.highlight_fg)
                .bg(picker.highlight_bg)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::default().fg(picker.fg)
        };
        let line = if data.title.is_empty() {
            text.clone()
        } else {
            format!("{text}  ({})", data.title)
        };
        frame.buffer_mut().set_string(
            list_x,
            y,
            theway_transport::feed::truncate_chars(&line, list_w),
            style,
        );
        y += 1;
    }
    if y == list_y {
        // Empty active column: a single dim hint so the band is not blank.
        frame.buffer_mut().set_string(
            list_x,
            list_y,
            theway_transport::feed::truncate_chars(
                "(no choices — use /model <provider:model>)",
                list_w,
            ),
            Style::default().fg(picker.dim),
        );
    }
}

/// Band row budget: 1 breadcrumb row + up to this many choice rows.
pub(crate) fn band_rows(choice_count: usize, cap: usize) -> u16 {
    1 + choice_count.clamp(1, cap) as u16
}
