//! Long-line handling for `grep` output: context lines keep a head preview,
//! match lines center the preview window on the match.

use super::MAX_MATCH_LINE_CHARS;

pub(super) fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_MATCH_LINE_CHARS {
        line.to_string()
    } else {
        let preview: String = line.chars().take(MAX_MATCH_LINE_CHARS).collect();
        format!("{preview}...[line truncated]")
    }
}

/// Center the preview window on the match's byte range; falls back to a head
/// preview when no range is known.
pub(super) fn preview_match_line(
    line: &str,
    match_range: Option<(usize, usize)>,
) -> (String, bool) {
    if line.chars().count() <= MAX_MATCH_LINE_CHARS {
        return (line.to_string(), false);
    }

    let Some((match_start, match_end)) = match_range else {
        let preview: String = line.chars().take(MAX_MATCH_LINE_CHARS).collect();
        return (format!("{preview}...[line truncated]"), true);
    };

    let match_start_chars = line[..match_start].chars().count();
    let match_len = line[match_start..match_end].chars().count().max(1);
    let visible_match_len = match_len.min(MAX_MATCH_LINE_CHARS);
    let context_budget = MAX_MATCH_LINE_CHARS.saturating_sub(visible_match_len);
    let before_budget = context_budget / 2;
    let after_budget = context_budget - before_budget;
    let start_char = match_start_chars.saturating_sub(before_budget);
    let end_char = match_start_chars + visible_match_len + after_budget;
    let total_chars = line.chars().count();

    let mut preview = String::new();
    if start_char > 0 {
        preview.push_str("[line truncated]...");
    }
    preview.extend(
        line.chars()
            .skip(start_char)
            .take(end_char.saturating_sub(start_char).min(total_chars)),
    );
    if end_char < total_chars {
        preview.push_str("...[line truncated]");
    }
    (preview, true)
}
