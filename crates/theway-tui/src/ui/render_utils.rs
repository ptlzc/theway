//! Free rendering / display helpers for the TUI surface (split out of `ui/mod.rs`).
//!
//! Panel/overlay line builders, control-plane-prompt redaction, queued-prompt previews,
//! textarea construction, terminal enter/leave sequences, and the headless feed printer.
//! Everything here is a free function — no `App` state.

use std::io::Write as _;

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
use tui_textarea::TextArea;

use super::FeedUpdate;
use theway::app::feed;

const QUEUED_PREVIEW_CHARS: usize = 80;

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

pub(super) fn safe_control_prompt_payload(value: &serde_json::Value, cap: usize) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    safe_control_prompt_text(&text, cap)
}
fn redact_control_prompt_secrets(text: &str) -> String {
    static TOKENISH_FIELD: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r#"(?i)(token|secret|password|api[_-]?key|authorization|cookie)(["'=:\s]+)([^"',\s&}]+)"#,
        )
        .expect("control prompt redaction regex must compile")
    });
    let redacted = theway::bug_report::redact(text);
    TOKENISH_FIELD
        .replace_all(&redacted, "$1$2[REDACTED]")
        .into_owned()
}

pub(super) fn panel_rule_preview(text: &str, width: usize) -> String {
    let redacted = theway::bug_report::redact(text).replace('\n', " ");
    feed::truncate_chars(&redacted, width.max(1))
}

pub(super) fn queue_preview(text: &str) -> String {
    let redacted = theway::bug_report::redact(text).replace('\n', " ");
    feed::truncate_chars(&redacted, QUEUED_PREVIEW_CHARS)
}

pub(super) fn prompt_display(text: &str, image_count: usize) -> String {
    if image_count == 0 {
        return text.to_string();
    }
    let suffix = image_attachment_display(image_count);
    if text.is_empty() {
        suffix
    } else {
        format!("{text}\n{suffix}")
    }
}

fn image_attachment_display(image_count: usize) -> String {
    match image_count {
        1 => "[1 image attachment]".to_string(),
        n => format!("[{n} image attachments]"),
    }
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

pub(super) fn user_facing_run_error(error: &str) -> String {
    let Some(rest) = error.strip_prefix("no API key for provider: ") else {
        return error.to_string();
    };
    let provider = rest.split(';').next().unwrap_or(rest).trim();
    if provider.is_empty() {
        return error.to_string();
    }
    let vars = theway_llm_provider::env_api_keys::env_var_names(provider);
    let credential_hint = if vars.is_empty() {
        "configure a provider-specific credential".to_string()
    } else {
        format!("set {}", vars.join(" or "))
    };
    format!("no API key for provider: {provider}; run /login {provider} or {credential_hint}")
}

pub(super) fn new_textarea() -> TextArea<'static> {
    let mut textarea = TextArea::default();
    textarea.set_cursor_line_style(Style::default());
    textarea.set_placeholder_text("type a message, or /help");
    textarea
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

pub(super) fn print_headless_update(update: &FeedUpdate, at_line_start: &mut bool) {
    let mut out = std::io::stdout();
    match update {
        FeedUpdate::TextDelta(delta) => {
            let _ = write!(out, "{delta}");
            *at_line_start = delta.ends_with('\n');
        }
        FeedUpdate::ThinkingDelta(_) => {}
        FeedUpdate::ToolStart { name, args } => {
            if !*at_line_start {
                let _ = writeln!(out);
            }
            let _ = writeln!(out, "⚙ {name}{args}");
            *at_line_start = true;
        }
        FeedUpdate::ToolProgress { .. } => {}
        FeedUpdate::ToolEnd { lines, .. } => {
            for line in lines {
                let _ = writeln!(out, "    {line}");
            }
            *at_line_start = true;
        }
        FeedUpdate::Plain { text, .. } => {
            if !*at_line_start {
                let _ = writeln!(out);
            }
            let _ = writeln!(out, "{text}");
            *at_line_start = true;
        }
        FeedUpdate::TriggerPollStatus(_) => {}
        FeedUpdate::SkillsReloaded { .. } => {}
        FeedUpdate::TurnStart => {}
        FeedUpdate::TurnEnd => {
            if !*at_line_start {
                let _ = writeln!(out);
                *at_line_start = true;
            }
        }
    }
    let _ = out.flush();
}
