//! Rendering helpers for the conversation feed: block separation, timestamp labels,
//! level styling, and display-width-aware word wrapping used by `Feed::lines` and
//! `Feed::plain_lines`.

use chrono::{DateTime, Local, TimeZone, Utc};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::types::Block;
use super::types::Level;

pub(crate) fn should_separate(previous: Option<&Block>, current: &Block, has_output: bool) -> bool {
    if !has_output {
        return false;
    }
    matches!(
        (previous, current),
        (_, Block::User { .. })
            | (
                Some(Block::User { .. }),
                Block::Assistant { .. } | Block::Thinking { .. } | Block::Tool { .. }
            )
    )
}

pub(crate) fn style_for_level(level: Level) -> Style {
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

pub(crate) fn display_prefix(timestamp: Option<&str>, label: &str) -> String {
    match timestamp {
        Some(ts) if label.is_empty() => format!("{ts} "),
        Some(ts) => format!("{ts} {label}"),
        None => label.to_string(),
    }
}

pub(crate) fn current_time_label() -> Option<String> {
    Some(Local::now().format("%Y-%m-%d %H:%M").to_string())
}

pub(crate) fn message_timestamp_label(timestamp_ms: i64) -> Option<String> {
    if timestamp_ms <= 0 {
        return None;
    }
    let dt = Utc.timestamp_millis_opt(timestamp_ms).single()?;
    Some(format_timestamp_label(dt, Local::now()))
}

pub(crate) fn format_timestamp_label(timestamp: DateTime<Utc>, _now: DateTime<Local>) -> String {
    let local = timestamp.with_timezone(&Local);
    local.format("%Y-%m-%d %H:%M").to_string()
}

/// Split `text` on newlines, word-wrap each paragraph to `width`, and push styled lines. An
/// optional `prefix` is prepended to the very first paragraph (e.g. `you ▸ `).
pub(crate) fn push_paragraphs(
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

/// Width-wrapped plain-text rows (no terminal styling) — the headless / transport
/// counterpart of the ratatui `push_paragraphs`. Same separators, prefixes and wrap
/// semantics, but emits `String` rows so server-side consumers (gRPC/HTTP snapshots,
/// relay) don't need the ratatui terminal stack.
pub(crate) fn push_plain_paragraphs(
    out: &mut Vec<String>,
    text: &str,
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
            out.push(row);
        }
    }
}

/// Display-width-aware word wrap. Breaks at the last space that fits; hard-breaks a single
/// word longer than `width`. Preserves leading whitespace (so indented tool output keeps its
/// shape). Returns at least one row (possibly empty) so blank lines survive.
pub(crate) fn wrap_str(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut rows: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            if let Some(bp) = last_space.take() {
                let rest = cur.split_off(bp);
                let rest = rest.trim_start_matches(' ').to_string();
                let done = std::mem::replace(&mut cur, rest);
                rows.push(done.trim_end().to_string());
                cur_w = UnicodeWidthStr::width(cur.as_str());
            } else {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
        }
        cur.push(ch);
        cur_w += cw;
        if ch == ' ' {
            last_space = Some(cur.len());
        }
    }
    rows.push(cur);
    rows
}
