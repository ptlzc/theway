//! Content-addressed cache for the feed's plain-text rows (issue #35).
//!
//! `wire_feed_lines` used to re-render the whole feed (`plain_lines(100)`) on
//! every snapshot — O(history) generation plus O(history) wire bytes per
//! frame. This cache fingerprints each block and only re-renders the suffix
//! after the first dirty block; snapshots then publish just the rows appended
//! since the last publish (`feed_lines_base` marks their absolute offset).
//!
//! The feed is append-mostly: block contents change in place only via
//! thinking-summary backfill (and `/clear`), so the common case is a pure
//! prefix match costing one fingerprint scan and zero re-renders.

use super::model::{Feed, push_plain_paragraphs};
use super::{Block, Level};

/// Content fingerprint of one feed block (fnv-1a over kind + fields). Two
/// blocks with identical fingerprints render identically for the same width,
/// so the cache reuses their rendered rows. Shared with the TUI's styled
/// render cache (`theway_tui::feed_cache`).
pub fn block_fingerprint(block: &Block) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    match block {
        Block::User { text, timestamp } => {
            mix(b"user\x00");
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Assistant { text, timestamp } => {
            mix(b"assistant\x00");
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Thinking { text, timestamp } => {
            mix(b"thinking\x00");
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::ToolCall {
            name,
            args,
            metadata,
            timestamp,
        } => {
            mix(b"tool_call\x00");
            mix(name.as_bytes());
            mix(args.as_bytes());
            mix(metadata.as_deref().unwrap_or("").as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Error {
            message,
            code,
            recoverable,
            timestamp,
        } => {
            mix(b"error\x00");
            mix(message.as_bytes());
            mix(code.as_deref().unwrap_or("").as_bytes());
            mix(if *recoverable { b"1" } else { b"0" });
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::ToolResult {
            lines,
            is_error,
            timestamp,
            ..
        } => {
            mix(b"toolresult\x00");
            mix(if *is_error { b"1" } else { b"0" });
            for line in lines {
                mix(line.as_bytes());
                mix(b"\x00");
            }
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
        Block::Plain {
            text,
            level,
            timestamp,
        } => {
            mix(b"plain\x00");
            mix(format!("{level:?}").as_bytes());
            mix(text.as_bytes());
            mix(timestamp.as_deref().unwrap_or("").as_bytes());
        }
    }
    hash
}

/// Incremental plain-text row cache for the daemon's `feed_lines` snapshots.
pub struct PlainLinesCache {
    rows: Vec<String>,
    /// Absolute row index where each block's rows start (aligned with
    /// `fingerprints`; includes the blank separator row pushed before the
    /// block). Rows are never trimmed here — headless consumers print the
    /// whole transcript.
    block_starts: Vec<usize>,
    fingerprints: Vec<u64>,
    width: usize,
    /// When set, every adjacent block pair gets a separator row (not just
    /// user-message boundaries).
    separate_all: bool,
    /// Blocks re-rendered by the last `update` (0 = everything was cached).
    pub last_rebuilt: usize,
    /// Absolute row where the last rebuild began. Replacing the authoritative
    /// rows from this point applies the update without cloning the prefix.
    pub last_rebuilt_from_row: usize,
}

impl PlainLinesCache {
    pub fn new(width: usize) -> Self {
        Self {
            rows: Vec::new(),
            block_starts: Vec::new(),
            fingerprints: Vec::new(),
            width: width.max(1),
            separate_all: false,
            last_rebuilt: 0,
            last_rebuilt_from_row: 0,
        }
    }

    /// Opt into separating every adjacent block pair (not just user-message
    /// boundaries) — mirrors the TUI's `[feed] separate_all` theme flag for
    /// the daemon's plain-text projection.
    pub fn with_separate_all(mut self, separate_all: bool) -> Self {
        self.separate_all = separate_all;
        self
    }

    /// Cached rows (absolute row space; `feed_lines_base` = the count of rows
    /// a client already has).
    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// Absolute row index where each block's rows start (aligned with the
    /// block list, includes the blank separator row pushed before a block).
    pub fn block_starts(&self) -> &[usize] {
        &self.block_starts
    }

    /// Reconcile with `feed`, re-rendering only the suffix after the first
    /// dirty block. Width changes invalidate everything.
    pub fn update(&mut self, feed: &Feed, width: usize) {
        let width = width.max(1);
        if width != self.width {
            self.rows.clear();
            self.block_starts.clear();
            self.fingerprints.clear();
            self.width = width;
        }
        let blocks = feed.blocks();

        let mut first_dirty = 0;
        while first_dirty < blocks.len()
            && first_dirty < self.fingerprints.len()
            && self.fingerprints[first_dirty] == block_fingerprint(&blocks[first_dirty])
        {
            first_dirty += 1;
        }
        self.rebuild_from(feed, width, first_dirty);
    }

    /// Reconcile from an event-provided dirty block without scanning or
    /// hashing the clean prefix. Appends and truncation are detected from the
    /// cached/current lengths; callers only need to name in-place mutations.
    pub fn update_from_dirty(&mut self, feed: &Feed, width: usize, dirty: Option<usize>) {
        let width = width.max(1);
        let blocks = feed.blocks();
        let first_dirty = if width != self.width {
            0
        } else {
            dirty
                .unwrap_or(blocks.len())
                .min(blocks.len())
                .min(self.fingerprints.len())
        };
        self.rebuild_from(feed, width, first_dirty);
    }

    fn rebuild_from(&mut self, feed: &Feed, width: usize, first_dirty: usize) {
        if width != self.width {
            self.rows.clear();
            self.block_starts.clear();
            self.fingerprints.clear();
            self.width = width;
        }
        let blocks = feed.blocks();
        self.last_rebuilt = blocks.len().saturating_sub(first_dirty);
        self.last_rebuilt_from_row = self
            .block_starts
            .get(first_dirty)
            .copied()
            .unwrap_or(self.rows.len());
        if first_dirty < self.fingerprints.len() || blocks.len() != self.fingerprints.len() {
            self.rows.truncate(self.last_rebuilt_from_row);
            self.block_starts.truncate(first_dirty);
            self.fingerprints.truncate(first_dirty);
        }
        if self.last_rebuilt == 0 {
            return;
        }

        let mut previous = first_dirty.checked_sub(1).map(|index| &blocks[index]);
        for block in blocks.iter().skip(first_dirty) {
            self.block_starts.push(self.rows.len());
            if super::should_separate_with(
                previous,
                block,
                !self.rows.is_empty(),
                self.separate_all,
            ) {
                self.rows.push(String::new());
            }
            match block {
                Block::User { text, timestamp } => push_plain_paragraphs(
                    &mut self.rows,
                    text,
                    Some(&super::model::display_prefix(
                        timestamp.as_deref(),
                        "you \u{25b8} ",
                    )),
                    width,
                ),
                Block::Assistant { text, timestamp } => push_plain_paragraphs(
                    &mut self.rows,
                    text,
                    Some(&super::model::display_prefix(
                        timestamp.as_deref(),
                        "ai \u{25b8} ",
                    )),
                    width,
                ),
                Block::Thinking { text, timestamp } => push_plain_paragraphs(
                    &mut self.rows,
                    text,
                    Some(&super::model::display_prefix(
                        timestamp.as_deref(),
                        "[thinking] ",
                    )),
                    width,
                ),
                Block::ToolCall {
                    name,
                    args,
                    metadata,
                    timestamp,
                } => {
                    let mut text = format!("\u{2699} {name}{args}");
                    if let Some(metadata) = metadata {
                        text.push_str(&format!(" · {metadata}"));
                    }
                    push_plain_paragraphs(
                        &mut self.rows,
                        &text,
                        Some(&super::model::display_prefix(timestamp.as_deref(), "")),
                        width,
                    );
                }
                Block::Error {
                    message,
                    code,
                    recoverable,
                    timestamp,
                } => {
                    let mut text = format!("error: {message}");
                    if let Some(code) = code {
                        text.push_str(&format!(" ({code})"));
                    }
                    if *recoverable {
                        text.push_str(" [recoverable]");
                    }
                    push_plain_paragraphs(
                        &mut self.rows,
                        &text,
                        Some(&super::model::display_prefix(timestamp.as_deref(), "")),
                        width,
                    );
                }
                Block::ToolResult { lines, .. } => {
                    for line in lines {
                        self.rows
                            .extend(super::model::wrap_str(&format!("    {line}"), width));
                    }
                }
                Block::Plain {
                    text,
                    level: _,
                    timestamp,
                } => {
                    let prefix = timestamp
                        .as_deref()
                        .map(|ts| super::model::display_prefix(Some(ts), ""));
                    push_plain_paragraphs(&mut self.rows, text, prefix.as_deref(), width);
                }
            }
            self.fingerprints.push(block_fingerprint(block));
            previous = Some(block);
        }
    }
}

/// Trim the oldest feed blocks until the width-`width` plain-text rendering
/// fits within `max_lines` rows — the resume-replay cap matching the TUI's
/// `max_feed_lines` scrollback limit. Returns `true` when blocks were dropped.
///
/// The cut is computed from the rendered row count (same projection the wire
/// `feed_lines` uses), so a huge tool result counts as the many rows it
/// actually renders instead of a single block.
pub fn trim_feed_to_lines(feed: &mut Feed, width: usize, max_lines: usize) -> bool {
    if max_lines == 0 {
        return false;
    }
    let mut cache = PlainLinesCache::new(width);
    cache.update(feed, width);
    let rows = cache.rows().len();
    if rows <= max_lines {
        return false;
    }
    let keep_from = rows - max_lines;
    // `block_starts` is monotonically increasing; the first block whose first
    // row sits at or after `keep_from` is the oldest one we can keep.
    let first_keep = cache
        .block_starts()
        .partition_point(|&start| start < keep_from);
    if first_keep == 0 {
        return false;
    }
    let blocks = feed.wire_blocks();
    let kept: Vec<super::WireFeedBlock> = blocks.into_iter().skip(first_keep).collect();
    feed.replace_blocks(&kept);
    true
}

// Silence an unused-import check for `Level` (used via `Block::Plain` pattern
// with `level: _` — the type is part of the public re-export contract).
#[allow(unused)]
fn _level_in_scope(_: Level) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::WireFeedBlock;

    fn feed_with(blocks: &[WireFeedBlock]) -> Feed {
        let mut feed = Feed::new();
        feed.replace_blocks(blocks);
        feed
    }

    fn user(text: &str) -> WireFeedBlock {
        WireFeedBlock::User {
            text: text.into(),
            timestamp: None,
        }
    }

    #[test]
    fn unchanged_feed_is_zero_work() {
        let feed = feed_with(&[user("hello")]);
        let mut cache = PlainLinesCache::new(100);
        cache.update(&feed, 100);
        assert_eq!(cache.last_rebuilt, 1);
        let snapshot = cache.rows().to_vec();
        cache.update(&feed, 100);
        assert_eq!(cache.last_rebuilt, 0);
        assert_eq!(cache.rows(), &snapshot[..]);
    }

    #[test]
    fn append_only_rerenders_the_tail() {
        let feed = feed_with(&[user("one")]);
        let mut cache = PlainLinesCache::new(100);
        cache.update(&feed, 100);
        assert_eq!(cache.last_rebuilt, 1);

        let feed = feed_with(&[user("one"), user("two")]);
        cache.update(&feed, 100);
        assert_eq!(cache.last_rebuilt, 1);
        let rows = cache.rows();
        assert!(rows.iter().any(|row| row.contains("you ▸ one")), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("you ▸ two")), "{rows:?}");
        assert!(rows.iter().any(|row| row.is_empty()), "separator: {rows:?}");
    }

    #[test]
    fn changed_middle_rerenders_suffix() {
        let feed = feed_with(&[user("one"), user("two"), user("three")]);
        let mut cache = PlainLinesCache::new(100);
        cache.update(&feed, 100);
        assert_eq!(cache.last_rebuilt, 3);

        let feed = feed_with(&[user("one"), user("CHANGED"), user("three")]);
        cache.update(&feed, 100);
        assert_eq!(cache.last_rebuilt, 2);
        let rows = cache.rows();
        assert!(rows.iter().any(|row| row.contains("CHANGED")), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("three")), "{rows:?}");
    }

    #[test]
    fn cleared_feed_truncates_all() {
        let feed = feed_with(&[user("one")]);
        let mut cache = PlainLinesCache::new(100);
        cache.update(&feed, 100);
        let empty = feed_with(&[]);
        cache.update(&empty, 100);
        assert!(cache.rows().is_empty());
        assert_eq!(cache.last_rebuilt, 0);
    }

    #[test]
    fn width_change_invalidates() {
        let feed = feed_with(&[user("one")]);
        let mut cache = PlainLinesCache::new(100);
        cache.update(&feed, 100);
        cache.update(&feed, 40);
        assert_eq!(cache.last_rebuilt, 1);
        assert_eq!(cache.rows().len(), 1);
    }
    fn assistant(text: &str) -> WireFeedBlock {
        WireFeedBlock::Assistant {
            text: text.into(),
            timestamp: None,
        }
    }

    fn thinking(text: &str) -> WireFeedBlock {
        WireFeedBlock::Thinking {
            text: text.into(),
            timestamp: Some("t".into()),
        }
    }

    fn tool(name: &str, args: &str) -> WireFeedBlock {
        WireFeedBlock::ToolCall {
            name: name.into(),
            args: args.into(),
            metadata: None,
            timestamp: None,
        }
    }

    fn tool_result(lines: &[&str], is_error: bool) -> WireFeedBlock {
        WireFeedBlock::ToolResult {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            is_error,
            timestamp: None,
        }
    }

    fn plain(text: &str) -> WireFeedBlock {
        WireFeedBlock::Plain {
            text: text.into(),
            level: crate::feed::Level::System,
            timestamp: None,
        }
    }

    #[test]
    fn block_fingerprints_cover_all_variants_and_distinguish_fields() {
        let user = feed_with(&[user("hi")]);
        let assistant = feed_with(&[assistant("hi")]);
        let thinking = feed_with(&[thinking("hi")]);
        let tool_feed = feed_with(&[tool("read", "x")]);
        let ok_result = feed_with(&[tool_result(&["ok"], false)]);
        let err_result = feed_with(&[tool_result(&["bad"], true)]);
        let plain = feed_with(&[plain("hi")]);

        let fp = |f: &Feed| block_fingerprint(&f.blocks()[0]);
        assert_ne!(fp(&user), fp(&assistant));
        assert_ne!(fp(&assistant), fp(&thinking));
        assert_ne!(fp(&thinking), fp(&tool_feed));
        assert_ne!(fp(&tool_feed), fp(&ok_result));
        assert_ne!(fp(&ok_result), fp(&err_result));
        assert_ne!(fp(&err_result), fp(&plain));

        let tool2 = feed_with(&[tool("read", "y")]);
        assert_ne!(fp(&tool_feed), fp(&tool2));
        let ok_result2 = feed_with(&[tool_result(&["ok", "more"], false)]);
        assert_ne!(fp(&ok_result), fp(&ok_result2));
        let plain2 = feed_with(&[WireFeedBlock::Plain {
            text: "hi".into(),
            level: crate::feed::Level::Error,
            timestamp: None,
        }]);
        assert_ne!(fp(&plain), fp(&plain2));
    }

    #[test]
    fn update_renders_every_block_kind() {
        let feed = feed_with(&[
            user("hello"),
            assistant("world"),
            thinking("think"),
            tool("bash", " ls"),
            tool_result(&["one", "two"], true),
            plain("note"),
        ]);
        let mut cache = PlainLinesCache::new(80);
        cache.update(&feed, 80);
        let rows = cache.rows();
        assert!(rows.iter().any(|r| r.contains("you ▸ hello")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("ai ▸ world")), "{rows:?}");
        assert!(
            rows.iter().any(|r| r.contains("[thinking] think")),
            "{rows:?}"
        );
        assert!(rows.iter().any(|r| r.contains("⚙ bash ls")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("    one")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("    two")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("note")), "{rows:?}");
    }

    #[test]
    fn update_from_dirty_supports_append_dirty_and_width_change() {
        let feed = feed_with(&[user("one"), assistant("two")]);
        let mut cache = PlainLinesCache::new(80);
        cache.update(&feed, 80);

        let changed = feed_with(&[user("one"), assistant("CHANGED")]);
        cache.update_from_dirty(&changed, 80, Some(1));
        assert_eq!(cache.last_rebuilt, 1);
        assert!(cache.rows().iter().any(|r| r.contains("CHANGED")));

        // dirty = None means "append at end"; with same length it rebuilds none.
        cache.update_from_dirty(&changed, 80, None);
        assert_eq!(cache.last_rebuilt, 0);

        let appended = feed_with(&[user("one"), assistant("two"), plain("three")]);
        cache.update_from_dirty(&appended, 80, None);
        assert_eq!(cache.last_rebuilt, 1);
        assert!(cache.rows().iter().any(|r| r.contains("three")));

        // width change forces a full rebuild even with dirty Some.
        cache.update_from_dirty(&appended, 40, Some(0));
        assert_eq!(cache.last_rebuilt, 3);
        assert!(cache.rows().iter().any(|r| r.contains("two")));
    }

    #[test]
    fn update_from_dirty_truncates_to_current_length() {
        let feed = feed_with(&[user("one"), assistant("two")]);
        let mut cache = PlainLinesCache::new(80);
        cache.update(&feed, 80);
        let shorter = feed_with(&[user("one")]);
        cache.update_from_dirty(&shorter, 80, None);
        assert_eq!(cache.last_rebuilt, 0);
        assert_eq!(cache.rows().iter().filter(|r| r.contains("two")).count(), 0);
    }

    #[test]
    fn separate_all_gaps_every_block_pair() {
        let feed = feed_with(&[user("one"), assistant("two"), plain("three")]);
        let mut cache = PlainLinesCache::new(80);
        cache.update(&feed, 80);
        // Default: user→assistant gap, assistant→plain flush.
        let rows = cache.rows().to_vec();
        assert!(rows[1].is_empty(), "{rows:?}");
        assert!(
            !rows[3].is_empty(),
            "flush between assistant and plain: {rows:?}"
        );

        // separate_all: every adjacent pair gets a blank row.
        let mut cache = PlainLinesCache::new(80).with_separate_all(true);
        cache.update(&feed, 80);
        let rows = cache.rows();
        let blanks = rows.iter().filter(|r| r.is_empty()).count();
        assert_eq!(blanks, 2, "{rows:?}");
        assert!(rows[3].is_empty(), "{rows:?}");
    }
}
