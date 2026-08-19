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
use super::{Block, Level, should_separate};

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
        Block::Tool {
            name,
            args,
            timestamp,
        } => {
            mix(b"tool\x00");
            mix(name.as_bytes());
            mix(args.as_bytes());
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
    /// Blocks re-rendered by the last `update` (0 = everything was cached).
    pub last_rebuilt: usize,
}

impl PlainLinesCache {
    pub fn new(width: usize) -> Self {
        Self {
            rows: Vec::new(),
            block_starts: Vec::new(),
            fingerprints: Vec::new(),
            width: width.max(1),
            last_rebuilt: 0,
        }
    }

    /// Cached rows (absolute row space; `feed_lines_base` = the count of rows
    /// a client already has).
    pub fn rows(&self) -> &[String] {
        &self.rows
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
        if first_dirty < self.fingerprints.len() || blocks.len() != self.fingerprints.len() {
            let cut = self
                .block_starts
                .get(first_dirty)
                .copied()
                .unwrap_or(self.rows.len());
            self.rows.truncate(cut);
            self.block_starts.truncate(first_dirty);
            self.fingerprints.truncate(first_dirty);
        }
        if self.last_rebuilt == 0 {
            return;
        }

        let mut previous = first_dirty.checked_sub(1).map(|index| &blocks[index]);
        for block in blocks.iter().skip(first_dirty) {
            self.block_starts.push(self.rows.len());
            if should_separate(previous, block, !self.rows.is_empty()) {
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
                Block::Tool {
                    name,
                    args,
                    timestamp,
                } => {
                    let text = format!("\u{2699} {name}{args}");
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
}
