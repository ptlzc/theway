//! Shared truncation primitives. Mirrors `packages/coding-agent/src/core/tools/truncate.ts`.
//! Head-/tail-truncate by line count + byte cap; report what was kept vs dropped.

pub const DEFAULT_MAX_LINES: usize = 2_000;
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024; // 256 KiB

#[derive(Clone, Debug, Default)]
pub struct Truncation {
    pub total_lines: usize,
    pub kept_lines: usize,
    pub truncated_lines: usize,
    pub total_bytes: usize,
    pub kept_bytes: usize,
}

impl Truncation {
    pub fn note(&self) -> Option<String> {
        if self.truncated_lines == 0 {
            return None;
        }
        Some(format!(
            "[truncated: kept {}/{} lines, {} of {} bytes]",
            self.kept_lines, self.total_lines, self.kept_bytes, self.total_bytes
        ))
    }
}

/// Truncate `text` to at most `max_lines` lines and `max_bytes` bytes, keeping the head.
pub fn truncate_head(text: &str, max_lines: usize, max_bytes: usize) -> (String, Truncation) {
    let total_bytes = text.len();
    let mut total_lines = 0usize;
    let mut kept_bytes = 0usize;
    let mut kept_lines = 0usize;
    let mut out = String::with_capacity(total_bytes.min(max_bytes));
    for line in text.split_inclusive('\n') {
        total_lines += 1;
        if kept_lines < max_lines && kept_bytes + line.len() <= max_bytes {
            out.push_str(line);
            kept_lines += 1;
            kept_bytes += line.len();
        }
    }
    let trunc = Truncation {
        total_lines,
        kept_lines,
        truncated_lines: total_lines.saturating_sub(kept_lines),
        total_bytes,
        kept_bytes,
    };
    (out, trunc)
}

/// Same as [`truncate_head`] but keeps the tail. Used by `bash` so the last error is visible.
pub fn truncate_tail(text: &str, max_lines: usize, max_bytes: usize) -> (String, Truncation) {
    let total_bytes = text.len();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total_lines = lines.len();
    let mut kept_bytes = 0usize;
    let mut kept_lines = 0usize;
    let mut tail: Vec<&str> = Vec::new();
    for line in lines.iter().rev() {
        if kept_lines >= max_lines || kept_bytes + line.len() > max_bytes {
            break;
        }
        tail.push(line);
        kept_lines += 1;
        kept_bytes += line.len();
    }
    tail.reverse();
    let out = tail.concat();
    let trunc = Truncation {
        total_lines,
        kept_lines,
        truncated_lines: total_lines.saturating_sub(kept_lines),
        total_bytes,
        kept_bytes,
    };
    (out, trunc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_truncation_when_within_limits() {
        let (out, t) = truncate_head("hi\nthere\n", 100, 1024);
        assert_eq!(out, "hi\nthere\n");
        assert_eq!(t.truncated_lines, 0);
    }

    #[test]
    fn truncates_by_line_count() {
        let body = "a\nb\nc\nd\n";
        let (out, t) = truncate_head(body, 2, 1024);
        assert_eq!(out, "a\nb\n");
        assert_eq!(t.kept_lines, 2);
        assert_eq!(t.truncated_lines, 2);
    }

    #[test]
    fn tail_keeps_last() {
        let body = "a\nb\nc\nd\n";
        let (out, _t) = truncate_tail(body, 2, 1024);
        assert_eq!(out, "c\nd\n");
    }
}
