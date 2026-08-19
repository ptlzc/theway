//! UTF-8-safe byte cursor normalization for incremental text APIs.

/// Return the nearest valid byte cursor at or before `requested` and the text
/// from that cursor. Cursors beyond the end clamp to the end.
pub(crate) fn slice_from(text: &str, requested: u64) -> (u64, &str) {
    let requested = usize::try_from(requested).unwrap_or(usize::MAX);
    let mut offset = requested.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    (offset as u64, &text[offset..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_arbitrary_utf8_offsets_without_skipping_text() {
        let text = "a🙂中文";
        assert_eq!(slice_from(text, 2), (1, "🙂中文"));
        assert_eq!(slice_from(text, u64::MAX), (text.len() as u64, ""));
    }
}
