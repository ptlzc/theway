//! Free rendering / display helpers for the TUI surface (split out of `ui/mod.rs`).
//!
//! Panel/overlay line builders, control-plane-prompt redaction, textarea
//! construction, and terminal enter/leave sequences. Everything here is a free
//! function — no `App` state.

use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use once_cell::sync::Lazy;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use regex::Regex;
use theway_ratatui_textarea::TextArea;

use theway_transport::feed;

pub(super) fn panel_line(text: String, color: Color, width: usize) -> Line<'static> {
    Line::styled(
        feed::truncate_chars(&text, width.max(1)),
        Style::default().fg(color),
    )
}

pub(super) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn safe_control_prompt_label(text: &str) -> String {
    safe_control_prompt_text(text, 120)
}

pub(super) fn safe_control_prompt_text(text: &str, cap: usize) -> String {
    let redaction_window = cap.max(1).saturating_mul(4).min(1024);
    let redacted = redact_control_prompt_secrets(&feed::truncate_chars(text, redaction_window));
    feed::truncate_chars(&redacted, cap.max(1)).replace('\n', " ")
}

fn redact_control_prompt_secrets(text: &str) -> String {
    static TOKENISH_FIELD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r#"(?i)(token|secret|password|api[_-]?key|authorization|cookie)(["'=:\s]+)([^"',\s&}]+)"#,
        )
        .expect("control prompt redaction regex must compile")
    });
    let redacted = theway_transport::bug_report::redact(text);
    TOKENISH_FIELD
        .replace_all(&redacted, "$1$2[REDACTED]")
        .into_owned()
}

pub(super) fn panel_rule_preview(text: &str, width: usize) -> String {
    let redacted = theway_transport::bug_report::redact(text).replace('\n', " ");
    feed::truncate_chars(&redacted, width.max(1))
}

pub(super) fn human_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub(super) fn new_textarea() -> TextArea {
    // The ported textarea has no placeholder or cursor-line-style knobs;
    // defaults match the previous look (plain cursor, no placeholder).
    TextArea::new()
}

pub(super) fn enter_tui() -> Result<()> {
    enable_raw_mode()?;
    write_enter_tui_commands(&mut std::io::stdout())?;
    Ok(())
}

pub(super) fn leave_tui() -> Result<()> {
    write_leave_tui_commands(&mut std::io::stdout())?;
    disable_raw_mode()?;
    Ok(())
}

pub(super) fn write_enter_tui_commands(out: &mut impl std::io::Write) -> std::io::Result<()> {
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    // Mouse capture is written explicitly instead of via crossterm's
    // `EnableMouseCapture`: on Windows that command routes through winapi
    // (`is_ansi_code_supported() == false`) and pokes the real console
    // directly, so the sequences never reach a non-console writer — the
    // enter/leave byte streams desync the moment the TUI is redirected
    // (tests, `tee`, winpty-style wrappers). Windows Terminal and conhost
    // (Win10+) both implement the VT mouse protocol (`?1000h` normal + `?1006h`
    // SGR, the two modes the feed wheel needs), so writing them explicitly is
    // behavior-preserving on a real console and faithful to the writer here.
    write!(out, "\x1b[?1000h\x1b[?1006h")?;
    out.flush()
}

pub(super) fn write_leave_tui_commands(out: &mut impl std::io::Write) -> std::io::Result<()> {
    write!(out, "\x1b[?1006l\x1b[?1000l")?;
    execute!(out, DisableBracketedPaste, LeaveAlternateScreen)?;
    Ok(())
}
