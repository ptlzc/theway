#[tokio::test]
async fn authoritative_snapshot_replaces_local_feed_annotations() {
    let (mut app, _rx) = test_app().await;
    let status = fixture_status(app.latest.feed_blocks.clone());
    app.system_line("local note");
    app.apply_snapshot(status);
    // Full frames replace the local render model even when the authoritative
    // transcript itself is unchanged.
    let text = feed_text(&app);
    assert!(!text.contains("local note"), "{text}");
}

#[tokio::test]
async fn connection_log_survives_authoritative_snapshot() {
    let (mut app, _rx) = test_app().await;
    app.connection_line("daemon restarted; restored session sess-1");
    app.apply_snapshot(fixture_status(app.latest.feed_blocks.clone()));

    let text = feed_text(&app);
    assert!(
        text.contains("daemon restarted; restored session sess-1"),
        "{text}"
    );
}

#[tokio::test]
async fn snapshot_append_patch_pushes_only_new_block() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Plain {
        text: "banner".into(),
        level: theway_transport::feed::Level::System,
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    // Local annotations survive a pure tail append (no full rebuild).
    app.system_line("local note");
    let appended = WireFeedBlock::Assistant {
        text: "appended answer".into(),
        timestamp: None,
    };
    let mut second = fixture_status(Vec::new());
    second.feed_blocks_base = app.latest.feed_blocks.len() as u64;
    second.feed_block_patches = vec![WireFeedBlockPatch {
        index: second.feed_blocks_base,
        block: appended,
    }];
    app.apply_snapshot(second);
    let text = feed_text(&app);
    assert!(text.contains("banner"), "{text}");
    assert!(text.contains("appended answer"), "{text}");
    assert!(text.contains("local note"), "{text}");
}

#[tokio::test]
async fn snapshot_replacement_patch_updates_one_block() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Assistant {
        text: "partial".into(),
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    let mut patch = fixture_status(Vec::new());
    patch.feed_blocks_base = 1;
    patch.feed_block_patches = vec![WireFeedBlockPatch {
        index: 0,
        block: WireFeedBlock::Assistant {
            text: "complete".into(),
            timestamp: None,
        },
    }];

    app.apply_snapshot(patch);

    assert!(!app.resync_pending);
    assert_eq!(app.latest.feed_blocks.len(), 1);
    let text = feed_text(&app);
    assert!(text.contains("complete"), "{text}");
    assert!(!text.contains("partial"), "{text}");
}

/// Release-only consumer half of the issue #36 synthetic benchmark. The
/// daemon benchmark measures publication/materialization; this measures the
/// TUI's no-change, streaming-tail, and middle-backfill patch application.
#[tokio::test]
#[ignore = "run with cargo test -p theway-tui --release synthetic_10k -- --ignored --nocapture"]
async fn synthetic_10k_snapshot_apply_costs_scale_with_delta() {
    use std::time::Instant;

    let (mut app, _rx) = test_app().await;
    let mut blocks = Vec::with_capacity(10_000);
    for index in 0..9_999 {
        blocks.push(WireFeedBlock::Plain {
            text: format!("history-{index}"),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        });
    }
    blocks.push(WireFeedBlock::Assistant {
        text: "seed".into(),
        timestamp: None,
    });
    app.apply_snapshot(fixture_status(blocks));

    let mut no_change = fixture_status(Vec::new());
    no_change.feed_blocks_base = 10_000;
    const NO_CHANGE_SAMPLES: u32 = 10_000;
    let started = Instant::now();
    for _ in 0..NO_CHANGE_SAMPLES {
        app.apply_snapshot(no_change.clone());
    }
    let no_change = started.elapsed() / NO_CHANGE_SAMPLES;

    const PATCH_SAMPLES: u32 = 1_000;
    let started = Instant::now();
    for index in 0..PATCH_SAMPLES {
        let mut patch = fixture_status(Vec::new());
        patch.feed_blocks_base = 10_000;
        patch.feed_block_patches = vec![WireFeedBlockPatch {
            index: 9_999,
            block: WireFeedBlock::Assistant {
                text: format!("stream-{index}"),
                timestamp: None,
            },
        }];
        app.apply_snapshot(patch);
    }
    let streaming = started.elapsed() / PATCH_SAMPLES;

    let mut backfill = fixture_status(Vec::new());
    backfill.feed_blocks_base = 10_000;
    backfill.feed_block_patches = vec![WireFeedBlockPatch {
        index: 5_000,
        block: WireFeedBlock::Plain {
            text: "backfilled".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
    }];
    let started = Instant::now();
    app.apply_snapshot(backfill);
    let backfill = started.elapsed();

    eprintln!(
        "10k TUI snapshot: no-change={no_change:?}, streaming={streaming:?}, backfill={backfill:?}"
    );
    assert!(
        no_change.as_micros() < 20,
        "10k no-change apply must stay below 20us, measured {no_change:?}"
    );
}

#[tokio::test]
async fn snapshot_patch_gap_requests_authoritative_resync() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![WireFeedBlock::Assistant {
        text: "stable".into(),
        timestamp: None,
    }]);
    app.apply_snapshot(first);
    let mut gap = fixture_status(Vec::new());
    gap.feed_blocks_base = 2;
    gap.feed_block_patches = vec![WireFeedBlockPatch {
        index: 2,
        block: WireFeedBlock::Assistant {
            text: "missed".into(),
            timestamp: None,
        },
    }];

    app.apply_snapshot(gap);

    assert!(app.resync_pending);
    assert_eq!(app.latest.feed_blocks.len(), 1);
    assert!(feed_text(&app).contains("stable"));
}

#[test]
fn headless_line_cursor_replays_after_transcript_shrink() {
    let mut printed = 5;

    let start = super::headless_unprinted_start(0, 2, &mut printed);

    assert_eq!(start, Some(0));
    assert_eq!(printed, 2);
    assert_eq!(super::headless_unprinted_start(2, 1, &mut printed), Some(0));
    assert_eq!(printed, 3);
    assert_eq!(super::headless_unprinted_start(2, 1, &mut printed), None);
}

#[tokio::test]
async fn snapshot_truncation_rebuilds_feed() {
    let (mut app, _rx) = test_app().await;
    let first = fixture_status(vec![
        WireFeedBlock::Plain {
            text: "one".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
        WireFeedBlock::Plain {
            text: "two".into(),
            level: theway_transport::feed::Level::System,
            timestamp: None,
        },
    ]);
    app.apply_snapshot(first);
    // A shorter snapshot means the daemon truncated/reset the transcript —
    // prefix diff fails, the feed rebuilds from the new block list.
    let second = fixture_status(vec![WireFeedBlock::Plain {
        text: "fresh".into(),
        level: theway_transport::feed::Level::System,
        timestamp: None,
    }]);
    app.apply_snapshot(second);
    let text = feed_text(&app);
    assert!(text.contains("fresh"), "{text}");
    assert!(!text.contains("one"), "{text}");
    assert!(!text.contains("two"), "{text}");
}
