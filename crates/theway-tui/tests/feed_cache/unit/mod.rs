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
            ..Default::default()
        };
        cache.update(&feed, 40, &peek, 1000);
        assert_eq!(cache.last_rebuilt, 2);
        cache.update(&feed, 40, &peek, 1000);
        assert_eq!(cache.last_rebuilt, 0);
    }

    #[test]
    fn per_frame_counter_change_reuses_cached_blocks() {
        // Hand-written PartialEq (issue #44): cps/in/out/spinner_phase change
        // every frame and must NOT invalidate the cache; a full rebuild per
        // frame would degrade the #34/#35 incremental rendering.
        let feed = feed_with(&[user("one"), assistant("two")]);
        let mut cache = FeedRenderCache::new();
        cache.update(&feed, 80, &FeedRenderOptions::default(), 1000);
        assert_eq!(cache.last_rebuilt, 2);

        let mut opts = FeedRenderOptions::default();
        opts.thinking_cps = 1234.5;
        opts.thinking_input_tokens = 57_100;
        opts.thinking_output_tokens = 1_200;
        opts.spinner_phase = 9;
        cache.update(&feed, 80, &opts, 1000);
        assert_eq!(
            cache.last_rebuilt, 0,
            "per-frame counters must not invalidate the cache"
        );
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
        assert_eq!(block_fingerprint(&same_a), block_fingerprint(&same_b));
        let different = Block::Assistant {
            text: "other".into(),
            timestamp: None,
        };
        assert_ne!(block_fingerprint(&same_a), block_fingerprint(&different));
        let user_block = Block::User {
            text: "same".into(),
            timestamp: None,
        };
        assert_ne!(block_fingerprint(&same_a), block_fingerprint(&user_block));
    }

    // ── v2 feed rhythm (#30): gap flows through the cache ────────────────

    #[test]
    fn feed_gap_renders_and_invalidates_on_change() {
        let feed = feed_with(&[user("hello"), assistant("world")]);
        let mut cache = FeedRenderCache::new();
        let mut opts = FeedRenderOptions::default();
        opts.theme.feed.gap = 2;
        cache.update(&feed, 40, &opts, 1000);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[0].trim_end(), "\u{276f} hello");
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "");
        assert_eq!(rows[3], "world");

        // Changing the gap is a structural theme change → full rebuild.
        opts.theme.feed.gap = 1;
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 2);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "world");
    }

    #[test]
    fn feed_separator_survives_incremental_splices() {
        let feed = feed_with(&[user("hello"), assistant("world")]);
        let mut cache = FeedRenderCache::new();
        let mut opts = FeedRenderOptions::default();
        opts.theme.feed.gap = 0;
        opts.theme.feed.separator = Some('─');
        cache.update(&feed, 40, &opts, 1000);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], "─".repeat(40));

        // Appending a block re-renders only the tail; the separator rows for
        // the frozen prefix must survive.
        let feed = feed_with(&[user("hello"), assistant("world"), user("third")]);
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 1);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[1], "─".repeat(40), "frozen separator intact");
        // A User block also gets a separator before it.
        assert_eq!(rows[3], "─".repeat(40), "new block got its separator");
        assert_eq!(rows[4].trim_end(), "\u{276f} third");
    }

    #[test]
    fn feed_separate_all_gaps_every_block_pair() {
        let feed = feed_with(&[
            user("hello"),
            assistant("world"),
            WireFeedBlock::ToolCall {
                name: "bash".into(),
                args: " ls".into(),
            metadata: None,
                timestamp: None,
            },
            user("third"),
        ]);
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 40, &opts, 1000);
        // Default rhythm: only user boundaries get a gap (user→assistant,
        // tool→user); assistant→tool stays flush.
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[0].trim_end(), "\u{276f} hello");
        assert_eq!(rows[1], "", "user→assistant gap");
        assert_eq!(rows[2], "world");
        assert!(rows[3].contains("bash"), "assistant→tool flush: {rows:?}");
        assert_eq!(rows[4], "", "tool→user gap");
        assert_eq!(rows[5].trim_end(), "\u{276f} third");

        // separate_all: every adjacent pair gets a gap.
        let mut opts = FeedRenderOptions::default();
        opts.theme.feed.separate_all = true;
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 4, "theme change → full rebuild");
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[0].trim_end(), "\u{276f} hello");
        assert_eq!(rows[1], "", "user→assistant gap");
        assert_eq!(rows[2], "world");
        assert_eq!(rows[3], "", "assistant→tool gap");
        assert!(rows[4].contains("bash"), "{rows:?}");
        assert_eq!(rows[5], "", "tool→user gap");
        assert_eq!(rows[6].trim_end(), "\u{276f} third");

        // Appending still separates the new tail under separate_all.
        let feed = feed_with(&[
            user("hello"),
            assistant("world"),
            WireFeedBlock::ToolCall {
                name: "bash".into(),
                args: " ls".into(),
            metadata: None,
                timestamp: None,
            },
            user("third"),
            assistant("done"),
        ]);
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 1);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[6].trim_end(), "\u{276f} third");
        assert_eq!(rows[7], "", "user→assistant gap on append");
        assert_eq!(rows[8], "done");
    }

    #[test]
    fn streaming_append_gaps_user_boundary_by_default() {
        // Regression: an appended assistant block after a user block takes the
        // streaming path; its leading gap must be pushed on the first frame.
        let feed = feed_with(&[user("hello"), assistant("wor")]);
        let mut cache = FeedRenderCache::new();
        let opts = FeedRenderOptions::default();
        cache.update(&feed, 40, &opts, 1000);
        // Streaming continues the last block.
        let feed = feed_with(&[user("hello"), assistant("world")]);
        cache.update(&feed, 40, &opts, 1000);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[0].trim_end(), "\u{276f} hello");
        assert_eq!(rows[1], "", "user→assistant gap under streaming append");
        assert_eq!(rows[2], "world");

        // A brand-new assistant block after a user block also gets the gap.
        // Layout: ❯hello / "" / world / "" / ❯third / "" / x
        let feed = feed_with(&[user("hello"), assistant("world"), user("third"), assistant("x")]);
        cache.update(&feed, 40, &opts, 1000);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[4].trim_end(), "\u{276f} third");
        assert_eq!(rows[5], "", "new streaming block got its gap");
        assert_eq!(rows[6], "x");
    }

    // ── v2 block frame (#31): margins/borders through the cache ──────────

    #[test]
    fn block_frame_survives_incremental_splices() {
        let feed = feed_with(&[
            user("hello"),
            WireFeedBlock::ToolCall {
                name: "bash".into(),
                args: " ls".into(),
            metadata: None,
                timestamp: None,
            },
        ]);
        let mut cache = FeedRenderCache::new();
        let mut opts = FeedRenderOptions::default();
        opts.theme.tool.margin_top = 1;
        opts.theme.tool.border_top = crate::ui::theme::BlockBorder::Thin;
        cache.update(&feed, 40, &opts, 1000);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.split('\n').collect();
        // user, feed gap, tool margin, tool border, tool content.
        assert_eq!(rows[2], "", "margin");
        assert_eq!(rows[3], "─".repeat(40), "border");

        // Appending keeps the frozen frame intact.
        let feed = feed_with(&[
            user("hello"),
            WireFeedBlock::ToolCall {
                name: "bash".into(),
                args: " ls".into(),
            metadata: None,
                timestamp: None,
            },
            user("third"),
        ]);
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 1);
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.split('\n').collect();
        assert_eq!(rows[3], "─".repeat(40), "frozen border intact");
        assert!(rows[6].trim_end().ends_with("third"), "{rows:?}");
    }


    /// Tool area unit semantics (issue #69): a lone tool call renders as one
    /// unit; when its result arrives the pair becomes a SINGLE unit — the
    /// prefix scan must treat call+result as one fingerprint, and the update
    /// must re-render the pair without leaving a gap between them.
    #[test]
    fn tool_result_arrival_rebuilds_pair_as_one_unit() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool.bg = Some(ratatui::style::Color::Rgb(40, 40, 46));
        theme.feed.separate_all = true;
        let opts = FeedRenderOptions {
            theme,
            ..Default::default()
        };
        let mut feed = Feed::new();
        feed.replace_blocks(&[
            WireFeedBlock::User {
                text: "go".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolCall {
                name: "read".into(),
                args: " x".into(),
                metadata: None,
                timestamp: None,
            },
        ]);
        let mut cache = FeedRenderCache::new();
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 2, "user + call units");

        // Result arrives: [User, ToolCall] -> [User, ToolCall, ToolResult].
        feed.replace_blocks(&[
            WireFeedBlock::User {
                text: "go".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolCall {
                name: "read".into(),
                args: " x".into(),
                metadata: None,
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["content".into()],
                is_error: false,
                timestamp: None,
            },
        ]);
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 1, "only the pair unit rebuilds");
        let text = flat(cache.lines());
        let rows: Vec<&str> = text.lines().collect();
        assert!(rows[0].contains("go"), "{rows:?}");
        assert_eq!(rows[1], "", "separate_all gap at user->call: {rows:?}");
        assert!(rows[2].contains("⏵ read"), "{rows:?}");
        assert!(rows[3].contains("content"), "{rows:?}");
        // User->call boundary has a gap (separate_all); the pair itself has
        // none — call row and result body are adjacent.
        assert_eq!(rows.len(), 4, "no gap inside the pair: {rows:?}");

        // Re-update with the same feed: everything cached.
        cache.update(&feed, 40, &opts, 1000);
        assert_eq!(cache.last_rebuilt, 0);
    }
