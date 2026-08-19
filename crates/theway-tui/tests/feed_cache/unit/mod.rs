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
