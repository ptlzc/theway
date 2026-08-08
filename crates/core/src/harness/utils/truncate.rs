//! Text truncation primitives. TODO: full 1:1 port of
//! `packages/agent/src/harness/utils/truncate.ts` (~344 lines). For now we expose a basic
//! "truncate to N chars, prepend an ellipsis hint" helper used by tool results.

/// Truncate `text` to at most `max_chars` characters, prefixing a tag describing the cut when
/// truncation happened.
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!(
        "[truncated, kept {} of {} chars]\n{truncated}",
        max_chars,
        text.chars().count()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_short_text_through() {
        assert_eq!(truncate_text("hi", 10), "hi");
    }

    #[test]
    fn truncates_long_text() {
        let out = truncate_text(&"x".repeat(100), 10);
        assert!(out.starts_with("[truncated"));
        assert!(out.ends_with(&"x".repeat(10)));
    }
}
