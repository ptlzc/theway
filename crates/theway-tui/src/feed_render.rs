//! Ratatui rendering for the conversation feed (daemon-kernel-layers: the
//! terminal rendering moved from the SDK into the TUI; the UI-agnostic model
//! lives in `theway_transport::feed`).
//!
//! [`lines`] renders a [`Feed`] to width-wrapped, styled `ratatui` lines,
//! ready to scroll/draw — the terminal counterpart of `Feed::plain_lines`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use theway_transport::feed::{
    Block, Level, display_prefix, should_separate, wrap_str,
};

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

/// Render the whole feed to width-wrapped `ratatui` lines, ready to scroll/draw.
pub fn lines(feed: &theway_transport::feed::Feed, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
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
                push_paragraphs(&mut out, text, Style::default(), Some(&prefix), width);
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
    out
}
