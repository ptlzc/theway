//! Block-level render cache for the conversation feed (issue #34).
//!
//! The feed is append-mostly and every snapshot republishes ALL feed blocks,
//! so re-rendering the whole feed per frame was quadratic over history. This
//! cache fingerprints each block and only re-renders the suffix after the
//! first dirty block; unchanged blocks reuse their rendered lines. The
//! rendered output is composed as one `Vec<Line>` with per-block ranges (the
//! range includes the blank separator line pushed before the block), so a
//! dirty suffix can be spliced in with a single truncate + extend.
//!
//! Width, render options (thinking mode / tool expansion) and the scrollback
//! cap are cache keys: any change invalidates everything (those toggles are
//! rare user actions, not per-frame events). The head trim drains only when
//! the line count exceeds the cap by `TRIM_MARGIN`, keeping the drain
//! memmove off the hot path; `trimmed` tracks how many lines were dropped so
//! the caller's uncapped scroll/selection coordinates stay valid.

use ratatui::text::Line;

use theway_transport::feed::{Feed, should_separate};

use crate::feed_render::{self, FeedRenderOptions};

/// Growth margin above `cap` before the head trim actually drains.
const TRIM_MARGIN: usize = 512;

#[derive(Default)]
pub struct FeedRenderCache {
    lines: Vec<Line<'static>>,
    /// Rendered line range per feed block (index-aligned with `fingerprints`;
    /// includes the separator line pushed before the block).
    block_ranges: Vec<std::ops::Range<usize>>,
    fingerprints: Vec<u64>,
    /// Lines dropped from the head by the scrollback-cap trim.
    trimmed: usize,
    width: usize,
    opts: FeedRenderOptions,
    cap: usize,
    /// Blocks re-rendered by the last `update` (0 = everything was cached).
    /// Exposed for tests and diagnostics.
    pub last_rebuilt: usize,
}

impl FeedRenderCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached rendered lines (head-trimmed to `cap`).
    pub fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    /// Lines dropped from the head; uncapped line index = capped index + trimmed.
    pub fn trimmed(&self) -> usize {
        self.trimmed
    }

    /// Reconcile the cache with `feed` at `width`/`opts`, capped at `cap`
    /// lines. Only blocks after the first fingerprint mismatch are
    /// re-rendered; a pure prefix match costs one fingerprint scan.
    pub fn update(&mut self, feed: &Feed, width: usize, opts: &FeedRenderOptions, cap: usize) {
        let width = width.max(1);
        let cap = cap.max(1);
        if width != self.width || *opts != self.opts || cap != self.cap {
            self.reset(width, *opts, cap);
        }
        let blocks = feed.blocks();

        // Prefix scan: the first block whose fingerprint differs (or is new).
        let mut first_dirty = 0;
        while first_dirty < blocks.len()
            && first_dirty < self.fingerprints.len()
            && self.fingerprints[first_dirty]
                == feed_render::block_fingerprint(&blocks[first_dirty])
        {
            first_dirty += 1;
        }
        self.last_rebuilt = blocks.len().saturating_sub(first_dirty);

        // Cut the rendered suffix at the first dirty block (its range start
        // includes the preceding separator, so the splice is clean).
        if first_dirty < self.fingerprints.len() || blocks.len() != self.fingerprints.len() {
            let cut = self
                .block_ranges
                .get(first_dirty)
                .map(|range| range.start)
                .unwrap_or(self.lines.len());
            self.lines.truncate(cut);
            self.block_ranges.truncate(first_dirty);
            self.fingerprints.truncate(first_dirty);
        }
        if self.last_rebuilt == 0 {
            return;
        }

        let mut previous = first_dirty.checked_sub(1).map(|index| &blocks[index]);
        for block in blocks.iter().skip(first_dirty) {
            let range_start = self.lines.len();
            if should_separate(previous, block, !self.lines.is_empty()) {
                self.lines.push(Line::raw(""));
            }
            self.lines
                .extend(feed_render::render_block(block, width, opts));
            self.block_ranges.push(range_start..self.lines.len());
            self.fingerprints
                .push(feed_render::block_fingerprint(block));
            previous = Some(block);
        }

        // Head trim with margin: drain down to `cap` only once the margin is
        // exceeded, so the per-frame cost stays O(changed blocks).
        if self.lines.len() > self.cap + TRIM_MARGIN {
            let cut = self.lines.len() - self.cap;
            self.lines.drain(..cut);
            self.trimmed += cut;
            for range in &mut self.block_ranges {
                let start = range.start.saturating_sub(cut);
                let end = range.end.saturating_sub(cut).max(start);
                *range = start..end;
            }
        }
    }

    fn reset(&mut self, width: usize, opts: FeedRenderOptions, cap: usize) {
        self.lines.clear();
        self.block_ranges.clear();
        self.fingerprints.clear();
        self.trimmed = 0;
        self.width = width;
        self.opts = opts;
        self.cap = cap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_transport::feed::WireFeedBlock;

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

    fn assistant(text: &str) -> WireFeedBlock {
        WireFeedBlock::Assistant {
            text: text.into(),
            timestamp: None,
        }
    }

    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn unchanged_feed_reuses_everything() {
        let feed = feed_with(&[user("hello"), assistant("world")]);
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 2);
        let snapshot = flat(cache.lines());
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 0);
        assert_eq!(flat(cache.lines()), snapshot);
    }

    #[test]
    fn append_only_rerenders_the_tail() {
        let blocks = vec![user("one"), assistant("two")];
        let feed = feed_with(&blocks);
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 2);

        let blocks = vec![user("one"), assistant("two"), user("three")];
        let feed = feed_with(&blocks);
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 1);
        assert_eq!(cache.fingerprints.len(), 3);
        assert_eq!(cache.block_ranges.len(), 3);
        let text = flat(cache.lines());
        assert!(text.contains("❯ one"), "{text}");
        assert!(text.contains("❯ three"), "{text}");
    }

    #[test]
    fn changed_middle_rerenders_suffix_only() {
        let blocks = vec![user("one"), assistant("two"), user("three")];
        let feed = feed_with(&blocks);
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 3);

        let blocks = vec![user("one"), assistant("CHANGED"), user("three")];
        let feed = feed_with(&blocks);
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 2);
        let text = flat(cache.lines());
        assert!(text.contains("CHANGED"), "{text}");
        assert!(text.contains("❯ three"), "{text}");
        // The separator between blocks survives the splice.
        let joined = flat(cache.lines());
        assert!(joined.contains("\n\n"), "separator lost:\n{joined}");
    }

    #[test]
    fn cleared_feed_truncates_all() {
        let feed = feed_with(&[user("one"), assistant("two")]);
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 80, &opts, 1000);
        let empty = feed_with(&[]);
        cache.update(&empty, 80, &opts, 1000);
        assert!(cache.lines().is_empty());
        assert_eq!(cache.last_rebuilt, 0);
        assert!(cache.fingerprints.is_empty());
    }

    #[test]
    fn width_or_option_change_invalidates() {
        let feed = feed_with(&[user("one"), assistant("two")]);
        let mut cache = FeedRenderCache::new();
        cache.update(&feed, 80, &FeedRenderOptions::default(), 1000);
        assert_eq!(cache.last_rebuilt, 2);
        cache.update(&feed, 40, &FeedRenderOptions::default(), 1000);
        assert_eq!(cache.last_rebuilt, 2);
        cache.update(&feed, 40, &FeedRenderOptions::default(), 1000);
        assert_eq!(cache.last_rebuilt, 0);

        let peek = FeedRenderOptions {
            thinking_mode: crate::feed_render::ThinkingMode::Peek,
            tools_expanded: false,
        };
        cache.update(&feed, 40, &peek, 1000);
        assert_eq!(cache.last_rebuilt, 2);
        cache.update(&feed, 40, &peek, 1000);
        assert_eq!(cache.last_rebuilt, 0);
    }

    #[test]
    fn cap_trims_head_and_tracks_trimmed() {
        // 700 plain lines at width 80 → 700 rendered lines.
        let mut feed = Feed::new();
        for i in 0..700 {
            feed.push_plain_untimed(format!("line {i}"), theway_transport::feed::Level::Output);
        }
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 80, &opts, 100);
        assert_eq!(cache.lines().len(), 100);
        assert_eq!(cache.trimmed(), 600);
        // The kept lines are the newest.
        let text = flat(cache.lines());
        assert!(!text.contains("line 0"), "{text}");
        assert!(text.contains("line 699"), "{text}");
        // A no-change update does not trim again.
        cache.update(&feed, 80, &opts, 100);
        assert_eq!(cache.trimmed(), 600);
        assert_eq!(cache.last_rebuilt, 0);
    }

    #[test]
    fn fingerprints_differ_by_kind_and_content() {
        use theway_transport::feed::Block;
        let same_a = Block::Assistant {
            text: "same".into(),
            timestamp: None,
        };
        let same_b = Block::Assistant {
            text: "same".into(),
            timestamp: None,
        };
        assert_eq!(
            feed_render::block_fingerprint(&same_a),
            feed_render::block_fingerprint(&same_b)
        );
        let different = Block::Assistant {
            text: "other".into(),
            timestamp: None,
        };
        assert_ne!(
            feed_render::block_fingerprint(&same_a),
            feed_render::block_fingerprint(&different)
        );
        let user_block = Block::User {
            text: "same".into(),
            timestamp: None,
        };
        assert_ne!(
            feed_render::block_fingerprint(&same_a),
            feed_render::block_fingerprint(&user_block)
        );
    }
}
