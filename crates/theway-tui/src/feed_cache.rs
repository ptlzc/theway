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

use theway_transport::feed::{Block, Feed, block_fingerprint, should_separate_with};

use crate::feed_render::{self, FeedRenderOptions, ThinkingMode};

/// Growth margin above `cap` before the head trim actually drains.
const TRIM_MARGIN: usize = 512;

/// Streaming render state for the LAST feed block while it appends (issue
/// #35): instead of re-rendering the growing block from scratch every frame
/// (O(n²) over the stream), deltas feed an incremental renderer and only the
/// unfrozen tail is re-processed per frame.
enum StreamState {
    /// Assistant markdown via `StreamingMarkdownRenderer`: frozen lines are
    /// processed once, the tail once per frame.
    Markdown {
        renderer: theway_markdown::StreamingMarkdownRenderer,
        /// Frozen RENDERER lines already processed into `cache.lines`.
        frozen_lines: usize,
        /// Cache rows those frozen lines occupy (wrapping expands 1→N).
        frozen_cache_rows: usize,
    },
    /// Thinking text via the incremental wrapper: completed rows append to
    /// `cache.lines` (Full) or rebuild a small Peek window each frame. Keeps
    /// a ring of the last few completed rows for the Peek window.
    Thinking {
        wrap: feed_render::IncrementalWrap,
        mode: ThinkingMode,
        /// Completed rows already pushed into `cache.lines`.
        completed_rows: usize,
        /// Ring of the last THINKING_PEEK_LINES completed rows (for Peek).
        last_rows: std::collections::VecDeque<String>,
        /// Characters streamed so far (header counter).
        char_count: usize,
    },
}

struct StreamingEntry {
    block_index: usize,
    /// Block source text at the last update (append detection prefix).
    source: String,
    /// Source length at the last rebase; streaming growth beyond
    /// `REBASE_BYTES` past this point falls back to a one-shot render so an
    /// unbroken paragraph (nothing freezes) cannot drive O(n) per frame.
    rebase_source_len: usize,
    state: StreamState,
}

/// Maximum streamed delta before a one-shot rebase bounds the unfrozen tail
/// (issue #35): an unbroken paragraph never freezes in the streaming
/// renderer, so without this cap per-frame cost would grow linearly with the
/// block and quadratically over the stream.
const REBASE_BYTES: usize = 24 * 1024;

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
    /// Streaming state for the last block while it appends (issue #35).
    streaming: Option<StreamingEntry>,
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
    /// re-rendered; a pure prefix match costs one fingerprint scan. When the
    /// LAST block is assistant/thinking and grows by pure append, a streaming
    /// renderer extends it in O(delta + tail) instead of re-rendering it.
    pub fn update(&mut self, feed: &Feed, width: usize, opts: &FeedRenderOptions, cap: usize) {
        let width = width.max(1);
        let cap = cap.max(1);
        if width != self.width || *opts != self.opts || cap != self.cap {
            self.reset(width, *opts, cap);
        }
        // Refresh the per-frame counters WITHOUT invalidating: option equality
        // deliberately ignores cps/in/out/spinner_phase (they change every
        // frame), but the streaming tail re-renders its stats line each frame
        // and must read the current values (issue #44). Frozen historical
        // blocks keep the values they were rendered with.
        self.opts = *opts;
        let blocks = feed.blocks();

        // Prefix scan: the first block whose fingerprint differs (or is new).
        let mut first_dirty = 0;
        while first_dirty < blocks.len()
            && first_dirty < self.fingerprints.len()
            && self.fingerprints[first_dirty] == block_fingerprint(&blocks[first_dirty])
        {
            first_dirty += 1;
        }
        self.last_rebuilt = blocks.len().saturating_sub(first_dirty);

        // A shrunken feed (e.g. `/clear`) has nothing to rebuild but must
        // still truncate: only return early when the block count is stable.
        if self.last_rebuilt == 0 && blocks.len() == self.fingerprints.len() {
            return;
        }

        // The dirty suffix starts at `cut` (the dirty block's range start,
        // which includes the preceding separator, so splices are clean).
        let cut = self
            .block_ranges
            .get(first_dirty)
            .map(|range| range.start)
            .unwrap_or(self.lines.len());

        // Streaming fast path: exactly one dirty block (the last) that
        // appends. `stream_block` owns the splice (it truncates to
        // `cut + frozen_rows` itself, keeping the frozen prefix intact).
        if blocks.len() == first_dirty + 1
            && let Some(block) = blocks.last()
            && let Some((streamable, text)) = streamable_block_text(block, opts)
        {
            let mut entry = self.streaming.take();
            let resume = entry.as_ref().is_some_and(|entry| {
                entry.block_index == first_dirty
                    && entry.state.kind_matches(block)
                    && entry.source.len() <= text.len()
                    && text[..entry.source.len()] == entry.source
            });
            if !resume {
                entry = Some(StreamingEntry {
                    block_index: first_dirty,
                    source: String::new(),
                    rebase_source_len: 0,
                    state: StreamState::new(block, opts, width),
                });
            }
            if streamable
                && let Some(entry) = entry
                && text.len().saturating_sub(entry.rebase_source_len) < REBASE_BYTES
            {
                let previous = first_dirty.checked_sub(1).map(|index| &blocks[index]);
                self.stream_block(first_dirty, cut, entry, block, previous, text, width);
                return;
            }
        }

        // One-shot fallback: render the dirty suffix block by block.
        self.streaming = None;
        if first_dirty < self.fingerprints.len() || blocks.len() != self.fingerprints.len() {
            self.lines.truncate(cut);
            self.block_ranges.truncate(first_dirty);
            self.fingerprints.truncate(first_dirty);
        }
        let mut previous = first_dirty.checked_sub(1).map(|index| &blocks[index]);
        for block in blocks.iter().skip(first_dirty) {
            let range_start = self.lines.len();
            if should_separate_with(
                previous,
                block,
                !self.lines.is_empty(),
                opts.theme.feed.separate_all,
            ) {
                feed_render::push_feed_gap(&mut self.lines, width, &opts.theme.feed);
            }
            self.lines
                .extend(feed_render::render_block(block, width, opts));
            self.block_ranges.push(range_start..self.lines.len());
            self.fingerprints.push(block_fingerprint(block));
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

    /// Streaming append for the last block: splice only the unfrozen tail.
    fn stream_block(
        &mut self,
        block_index: usize,
        mut cut: usize,
        mut entry: StreamingEntry,
        block: &Block,
        previous: Option<&Block>,
        text: &str,
        width: usize,
    ) {
        // First streaming frame of this block (fresh entry): the caller's
        // `cut` is either the pre-gap line count (brand-new block, one-shot
        // semantics) or the block range start (one-shot → streaming switch,
        // where the range already contains its leading gap). Either way,
        // truncate back to `cut` and re-push the gap so the splice keeps the
        // frozen prefix + gap and only re-renders the block body.
        let first_stream_frame = entry.source.is_empty();
        if first_stream_frame
            && should_separate_with(previous, block, cut > 0, self.opts.theme.feed.separate_all)
        {
            self.lines.truncate(cut);
            feed_render::push_feed_gap(&mut self.lines, width, &self.opts.theme.feed);
            cut = self.lines.len();
        }
        let delta = &text[entry.source.len()..];
        entry.source.push_str(delta);
        match &mut entry.state {
            StreamState::Markdown {
                renderer,
                frozen_lines,
                frozen_cache_rows,
            } => {
                self.lines.truncate(cut + *frozen_cache_rows);
                renderer.push_and_render(
                    delta,
                    Some(theway_markdown::default_syntect_with_color_level(
                        self.opts.color_level,
                    )),
                );
                let view = renderer.view();
                // Process newly frozen lines exactly once.
                let frozen_total = view.lines.len().min(renderer.frozen_lines_count());
                for i in *frozen_lines..frozen_total {
                    feed_render::push_rendered_markdown_line(
                        &mut self.lines,
                        i,
                        view.lines[i].clone(),
                        "",
                        ratatui::style::Style::default(),
                        width,
                        view.code_blocks,
                        view.hyperlinks,
                    );
                }
                *frozen_lines = frozen_total;
                *frozen_cache_rows = self.lines.len().saturating_sub(cut);
                // Unfrozen tail re-processes every frame.
                for i in frozen_total..view.lines.len() {
                    feed_render::push_rendered_markdown_line(
                        &mut self.lines,
                        i,
                        view.lines[i].clone(),
                        "",
                        ratatui::style::Style::default(),
                        width,
                        view.code_blocks,
                        view.hyperlinks,
                    );
                }
            }
            StreamState::Thinking {
                wrap,
                mode,
                completed_rows,
                last_rows,
                char_count,
            } => {
                wrap.push_str(delta);
                let new_rows = std::mem::take(&mut wrap.rows);
                *char_count += delta.chars().count();
                for row in &new_rows {
                    if last_rows.len() >= feed_render::THINKING_PEEK_LINES {
                        last_rows.pop_front();
                    }
                    last_rows.push_back(row.clone());
                }
                let old_completed = *completed_rows;
                *completed_rows = old_completed + new_rows.len();
                match mode {
                    ThinkingMode::Full => {
                        // The stats line at `cut` re-renders every frame (the
                        // char count grows); completed body rows stay frozen
                        // after it, and the partial tail row re-renders.
                        let stats =
                            feed_render::thinking_stats_line(*char_count, &self.opts, width);
                        if self.lines.len() > cut {
                            self.lines.truncate(cut + 1 + old_completed);
                            self.lines[cut] = stats;
                        } else {
                            self.lines.push(stats);
                        }
                        for row in &new_rows {
                            self.lines.push(ratatui::text::Line::styled(
                                row.clone(),
                                feed_render::THINKING_STYLE,
                            ));
                        }
                        self.lines.push(ratatui::text::Line::styled(
                            wrap.tail.clone(),
                            feed_render::THINKING_STYLE,
                        ));
                    }
                    ThinkingMode::Peek => {
                        // Rebuild the small peek window: stats line + last rows.
                        self.lines.truncate(cut);
                        self.lines.push(feed_render::thinking_stats_line(
                            *char_count,
                            &self.opts,
                            width,
                        ));
                        let total_rows = *completed_rows + 1;
                        let mut shown: Vec<&str> = last_rows.iter().map(String::as_str).collect();
                        shown.push(wrap.tail.as_str());
                        let start = shown.len().saturating_sub(feed_render::THINKING_PEEK_LINES);
                        for row in &shown[start..] {
                            self.lines.push(ratatui::text::Line::styled(
                                format!("  {row}"),
                                feed_render::THINKING_STYLE,
                            ));
                        }
                        if total_rows > feed_render::THINKING_PEEK_LINES {
                            self.lines.push(ratatui::text::Line::styled(
                                "  … Ctrl+O cycles: hidden/peek/full",
                                feed_render::RESULT_SUMMARY_STYLE,
                            ));
                        }
                    }
                    ThinkingMode::Hidden => {
                        self.lines.truncate(cut);
                    }
                }
            }
        }
        let end = self.lines.len();
        if self.block_ranges.len() == block_index {
            self.block_ranges.push(cut..end);
            self.fingerprints.push(block_fingerprint(block));
        } else {
            self.block_ranges[block_index] = cut..end;
            self.fingerprints[block_index] = block_fingerprint(block);
        }
        self.streaming = Some(entry);
    }

    fn reset(&mut self, width: usize, opts: FeedRenderOptions, cap: usize) {
        self.lines.clear();
        self.block_ranges.clear();
        self.fingerprints.clear();
        self.streaming = None;
        self.trimmed = 0;
        self.width = width;
        self.opts = opts;
        self.cap = cap;
    }
}

impl StreamState {
    fn kind_matches(&self, block: &Block) -> bool {
        matches!(
            (self, block),
            (StreamState::Markdown { .. }, Block::Assistant { .. })
                | (StreamState::Thinking { .. }, Block::Thinking { .. })
        )
    }

    fn new(block: &Block, opts: &FeedRenderOptions, width: usize) -> Self {
        match block {
            Block::Assistant { .. } => {
                let mut renderer = theway_markdown::StreamingMarkdownRenderer::new(
                    feed_render::markdown_style(opts.color_level),
                    true,
                );
                renderer.set_max_table_width(Some(width));
                StreamState::Markdown {
                    renderer,
                    frozen_lines: 0,
                    frozen_cache_rows: 0,
                }
            }
            Block::Thinking { .. } => {
                let mut wrap = feed_render::IncrementalWrap::new(width);
                // The `⏵ ` prefix is part of the first wrapped row (matching
                // the one-shot paragraph render).
                wrap.push_str(feed_render::TOOL_PREFIX);
                StreamState::Thinking {
                    wrap,
                    mode: opts.thinking_mode,
                    completed_rows: 0,
                    last_rows: std::collections::VecDeque::new(),
                    char_count: 0,
                }
            }
            _ => unreachable!("streaming state is only built for assistant/thinking blocks"),
        }
    }
}

/// Whether `block` takes the streaming path under `opts`, plus its text.
/// Thinking blocks stream in Full/Peek mode; Hidden renders nothing.
fn streamable_block_text<'a>(
    block: &'a Block,
    opts: &FeedRenderOptions,
) -> Option<(bool, &'a str)> {
    match block {
        Block::Assistant { text, .. } => Some((true, text)),
        Block::Thinking { text, .. } => {
            let streamable = opts.thinking_mode != ThinkingMode::Hidden;
            Some((streamable, text))
        }
        _ => None,
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("feed_cache/unit");

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use crate::feed_render::FeedRenderOptions;
    use theway_transport::feed::{Block, Feed, WireFeedBlock};

    fn feed_from_blocks(blocks: &[Block]) -> Feed {
        let wire: Vec<WireFeedBlock> = blocks
            .iter()
            .map(|block| match block {
                Block::User { text, timestamp } => WireFeedBlock::User {
                    text: text.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::Assistant { text, timestamp } => WireFeedBlock::Assistant {
                    text: text.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::Thinking { text, timestamp } => WireFeedBlock::Thinking {
                    text: text.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::Tool {
                    name,
                    args,
                    timestamp,
                } => WireFeedBlock::Tool {
                    name: name.clone(),
                    args: args.clone(),
                    timestamp: timestamp.clone(),
                },
                Block::ToolResult {
                    lines,
                    is_error,
                    timestamp,
                    ..
                } => WireFeedBlock::ToolResult {
                    lines: lines.clone(),
                    is_error: *is_error,
                    timestamp: timestamp.clone(),
                },
                Block::Plain {
                    text,
                    level,
                    timestamp,
                } => WireFeedBlock::Plain {
                    text: text.clone(),
                    level: *level,
                    timestamp: timestamp.clone(),
                },
            })
            .collect();
        let mut feed = Feed::new();
        feed.replace_blocks(&wire);
        feed
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

    const CHUNKS: [&str; 6] = [
        "# Heading\n\nSome **bold** text and a [link](https://example.com/x).\n",
        "More prose that will eventually wrap across the width because it is quite long.\n",
        "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n",
        "- list item one\n- list item two\n",
        "A | B\n---|---\n1 | 2\n",
        "Final paragraph.\n",
    ];

    #[test]
    fn streamed_assistant_matches_one_shot_render() {
        // Reference: one-shot render of the whole text.
        let full_text: String = CHUNKS.concat();
        let reference_feed = feed_from_blocks(&[Block::Assistant {
            text: full_text.clone(),
            timestamp: None,
        }]);
        let reference = flat(&crate::feed_render::lines(
            &reference_feed,
            40,
            &FeedRenderOptions::default(),
        ));

        // Streaming: append the chunks one by one through the cache.
        let mut cache = FeedRenderCache::new();
        let mut text = String::new();
        for (i, chunk) in CHUNKS.iter().enumerate() {
            text.push_str(chunk);
            let feed = feed_from_blocks(&[Block::Assistant {
                text: text.clone(),
                timestamp: None,
            }]);
            cache.update(&feed, 40, &FeedRenderOptions::default(), 1000);
            assert_eq!(cache.last_rebuilt, 1, "chunk {i} must stream");
        }
        assert_eq!(flat(cache.lines()), reference, "streamed != one-shot");
    }

    #[test]
    fn streamed_thinking_matches_one_shot_in_full_mode() {
        let text: String = "pondering the design\ndeeply, considering edge cases and tradeoffs that wrap over the width"
            .into();
        let reference_feed = feed_from_blocks(&[Block::Thinking {
            text: text.clone(),
            timestamp: None,
        }]);
        let reference = flat(&crate::feed_render::lines(
            &reference_feed,
            24,
            &FeedRenderOptions::default(),
        ));

        let mut cache = FeedRenderCache::new();
        for end in (0..text.len()).step_by(7).chain([text.len()]) {
            let feed = feed_from_blocks(&[Block::Thinking {
                text: text[..end].to_string(),
                timestamp: None,
            }]);
            cache.update(&feed, 24, &FeedRenderOptions::default(), 1000);
        }
        assert_eq!(flat(cache.lines()), reference, "streamed != one-shot");
    }

    #[test]
    fn streamed_thinking_peek_window_is_bounded() {
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Peek,
            tools_expanded: false,
            ..Default::default()
        };
        let mut text = String::new();
        for i in 0..50 {
            text.push_str(&format!("line {i}\n"));
            let feed = feed_from_blocks(&[Block::Thinking {
                text: text.clone(),
                timestamp: None,
            }]);
            cache.update(&feed, 40, &opts, 1000);
        }
        let rendered = flat(cache.lines());
        // Header + at most THINKING_PEEK_LINES rows + hint.
        assert!(rendered.contains("⏵ thinking ·"), "{rendered}");
        assert!(rendered.contains("line 49"), "{rendered}");
        assert!(!rendered.contains("line 0\n"), "{rendered}");
        assert!(rendered.contains("Ctrl+O cycles"), "{rendered}");
        assert!(cache.lines().len() <= 1 + feed_render::THINKING_PEEK_LINES + 1);
    }

    #[test]
    fn streamed_thinking_stats_tracks_per_frame_counters() {
        // The streaming tail re-renders the stats line each frame with the
        // CURRENT counters (issue #44): cps/in/out refresh without a cache
        // invalidation, so a live stream shows live numbers.
        let mut cache = FeedRenderCache::new();
        let feed = feed_from_blocks(&[Block::Thinking {
            text: "pondering".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_cps: 100.0,
            thinking_input_tokens: 500,
            thinking_output_tokens: 90,
            ..Default::default()
        };
        cache.update(&feed, 80, &opts, 1000);

        let feed = feed_from_blocks(&[Block::Thinking {
            text: "pondering the design".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_cps: 1000.0,
            thinking_input_tokens: 57_100,
            thinking_output_tokens: 1_200,
            ..Default::default()
        };
        cache.update(&feed, 80, &opts, 1000);
        let rendered = flat(cache.lines());
        assert!(
            rendered.contains("c/s: 1000 · in: 57.1k · out: 1.2k"),
            "streamed stats line must show the updated counters:\n{rendered}"
        );
    }

    #[test]
    fn mid_edit_falls_back_to_one_shot() {
        let mut cache = FeedRenderCache::new();
        let feed = feed_from_blocks(&[Block::Assistant {
            text: "streaming content".into(),
            timestamp: None,
        }]);
        cache.update(&feed, 80, &FeedRenderOptions::default(), 1000);
        // Backfill replaces the text (not an append): one-shot fallback.
        let feed = feed_from_blocks(&[Block::Assistant {
            text: "REPLACED summary".into(),
            timestamp: None,
        }]);
        cache.update(&feed, 80, &FeedRenderOptions::default(), 1000);
        assert_eq!(cache.last_rebuilt, 1);
        let rendered = flat(cache.lines());
        assert!(rendered.contains("REPLACED summary"), "{rendered}");
        assert!(!rendered.contains("streaming content"), "{rendered}");
    }

    #[test]
    fn new_block_after_streamed_block_finalizes() {
        let mut cache = FeedRenderCache::new();
        let blocks = vec![Block::Assistant {
            text: "answer text".into(),
            timestamp: None,
        }];
        let feed = feed_from_blocks(&blocks);
        cache.update(&feed, 80, &FeedRenderOptions::default(), 1000);
        // Append a tool block: the assistant block is now frozen history.
        let blocks = vec![
            Block::Assistant {
                text: "answer text".into(),
                timestamp: None,
            },
            Block::Tool {
                name: "read".into(),
                args: "(path=\"x\")".into(),
                timestamp: None,
            },
        ];
        let feed = feed_from_blocks(&blocks);
        cache.update(&feed, 80, &FeedRenderOptions::default(), 1000);
        assert_eq!(cache.last_rebuilt, 1);
        let rendered = flat(cache.lines());
        assert!(rendered.contains("answer text"), "{rendered}");
        assert!(rendered.contains("⏵ read"), "{rendered}");
    }
}
