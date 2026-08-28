use super::*;
use std::time::Instant;

/// Release-only synthetic benchmark for issue #36. It is ignored in normal
/// test runs because the wall-clock threshold is meaningful only in release
/// builds on an otherwise idle machine.
#[tokio::test]
#[ignore = "run with cargo test -p theway-daemon --release synthetic_10k -- --ignored --nocapture"]
async fn synthetic_10k_snapshot_publication_costs_scale_with_delta() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let session_id = host.session.id.clone();
    for index in 0..5_000 {
        host.system_line(format!("history-{index}"));
    }
    host.apply_feed_update(&session_id, FeedUpdate::ThinkingDelta("seed".into()));
    let thinking_index = 5_000;
    for index in 5_001..10_000 {
        host.system_line(format!("history-{index}"));
    }

    let mut authoritative = host.wire_snapshot();
    assert_eq!(authoritative.feed_blocks.len(), 10_000);

    const NO_CHANGE_SAMPLES: u32 = 2_000;
    let started = Instant::now();
    for _ in 0..NO_CHANGE_SAMPLES {
        let update = host.wire_update();
        let delta = update.feed_delta().unwrap();
        assert!(delta.feed_block_patches.is_empty());
        assert!(delta.feed_lines.is_empty());
        assert!(update.apply_to(&mut authoritative));
    }
    let no_change = started.elapsed() / NO_CHANGE_SAMPLES;

    host.apply_feed_update(&session_id, FeedUpdate::TextDelta("x".into()));
    assert!(host.wire_update().apply_to(&mut authoritative));
    const STREAMING_SAMPLES: u32 = 500;
    let started = Instant::now();
    for _ in 0..STREAMING_SAMPLES {
        host.apply_feed_update(&session_id, FeedUpdate::TextDelta("x".into()));
        let update = host.wire_update();
        assert_eq!(update.feed_delta().unwrap().feed_block_patches.len(), 1);
        assert!(update.apply_to(&mut authoritative));
    }
    let streaming = started.elapsed() / STREAMING_SAMPLES;

    host.apply_feed_update(
        &session_id,
        FeedUpdate::ThinkingSummary {
            block_index: thinking_index,
            summary: "public summary".into(),
        },
    );
    let started = Instant::now();
    let backfill = host.wire_update();
    assert_eq!(backfill.feed_delta().unwrap().feed_block_patches.len(), 1);
    assert!(backfill.apply_to(&mut authoritative));
    let backfill = started.elapsed();

    eprintln!(
        "10k daemon snapshot: no-change={no_change:?}, streaming={streaming:?}, backfill={backfill:?}"
    );
    assert!(
        no_change.as_micros() < 20,
        "10k no-change publication must stay below 20us, measured {no_change:?}"
    );
}
