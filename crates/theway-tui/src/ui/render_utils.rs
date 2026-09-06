//! Free rendering / display helpers for the TUI surface (split out of `ui/mod.rs`).
//!
//! Panel/overlay line builders, control-plane-prompt redaction, textarea
//! construction, and terminal enter/leave sequences. Everything here is a free
//! function — no `App` state.

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
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

/// Last `n` characters of an id (session/node id), for compact panel display
/// (issue #104): `id_suffix("01a06cc8-…-78dc", 5)` → `"78dc"`.
pub(super) fn id_suffix(id: &str, n: usize) -> String {
    id.chars()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
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

/// Compact token-count label for the prompt chrome info line (e.g. `1.5k tok`,
/// `2.4M tok`). Bare counts under 10k stay exact.
pub(super) fn human_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tok", tokens as f64 / 1_000_000.0)
    } else if tokens >= 10_000 {
        format!("{:.1}k tok", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens} tok")
    }
}

/// Context-usage label for the composer info line (issue #38): USED tokens
/// over the window with the fill percentage, e.g. `60k/1M [60%]`. Both
/// counts round to the largest unit that keeps them legible (`60k`, `1M`).
pub(super) fn context_usage_label(used: u64, window: u64) -> String {
    let pct = ((used as f64 * 100.0 / window as f64).round()).clamp(0.0, 100.0) as u64;
    format!(
        "{}/{} [{pct}%]",
        compact_tokens(used),
        compact_tokens(window)
    )
}

/// `60k` / `1M` style token counts: rounded, no decimals, no suffix.
/// A k-round that reaches 1000k carries into `1M`.
fn compact_tokens(tokens: u64) -> String {
    if tokens >= 999_500 {
        format!("{}M", (tokens + 500_000) / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", (tokens + 500) / 1_000)
    } else {
        format!("{tokens}")
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
    execute!(
        out,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    out.flush()
}

pub(super) fn write_leave_tui_commands(out: &mut impl std::io::Write) -> std::io::Result<()> {
    execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_suffix_takes_last_n_chars() {
        assert_eq!(id_suffix("01a06cc8-ee64-7ed0-9091-07b5ce78dc", 5), "e78dc");
        assert_eq!(id_suffix("abc", 5), "abc");
        assert_eq!(id_suffix("", 5), "");
    }

    #[test]
    fn context_usage_label_shows_used_over_window_with_percent() {
        // The canonical shape: 600k used of a 1M window at 60%.
        assert_eq!(context_usage_label(600_000, 1_000_000), "600k/1M [60%]");
        // Sub-1k counts stay exact; the percentage still renders.
        assert_eq!(context_usage_label(512, 2_000), "512/2k [26%]");
        // Rounding: 999_500 k-rounds to 1M.
        assert_eq!(context_usage_label(999_500, 1_000_000), "1M/1M [100%]");
        // Overflow clamps at 100%.
        assert_eq!(context_usage_label(2_000_000, 1_000_000), "2M/1M [100%]");
    }

    #[test]
    fn compact_tokens_rounds_to_largest_unit() {
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(1_000), "1k");
        assert_eq!(compact_tokens(1_499), "1k");
        assert_eq!(compact_tokens(1_500), "2k");
        assert_eq!(compact_tokens(59_999), "60k");
        assert_eq!(compact_tokens(999_499), "999k");
        assert_eq!(compact_tokens(999_500), "1M");
        assert_eq!(compact_tokens(1_048_576), "1M");
        assert_eq!(compact_tokens(2_500_000), "3M");
    }
}
