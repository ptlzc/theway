/// Stateful, width-aware incremental wrapper with the exact `wrap_str`
/// semantics (break at last space, hard-break overlong words, preserve
/// leading whitespace) applied across arbitrary chunk boundaries (issue #35).
///
/// `push_str` feeds appended text and moves COMPLETE rows into `rows`; the
/// current partial row stays in `tail`. A `\n` always terminates the current
/// row (empty paragraphs yield an empty row, matching `push_paragraphs`).
pub(crate) struct IncrementalWrap {
    width: usize,
    /// Current partial row (the live tail line).
    pub(crate) tail: String,
    /// Complete rows flushed so far.
    pub rows: Vec<String>,
}

impl IncrementalWrap {
    pub fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            tail: String::new(),
            rows: Vec::new(),
        }
    }

    /// Append text; flushes every row completed by the append.
    pub fn push_str(&mut self, delta: &str) {
        for ch in delta.chars() {
            if ch == '\n' {
                self.rows.push(std::mem::take(&mut self.tail));
                continue;
            }
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let cur_w = unicode_width::UnicodeWidthStr::width(self.tail.as_str());
            if cur_w + cw > self.width && !self.tail.is_empty() {
                if let Some(bp) = self.tail.rfind(' ') {
                    let rest = self.tail.split_off(bp);
                    let rest = rest.trim_start_matches(' ').to_string();
                    self.rows.push(self.tail.trim_end().to_string());
                    self.tail = rest;
                } else {
                    self.rows.push(std::mem::take(&mut self.tail));
                }
            }
            self.tail.push(ch);
        }
    }
}
