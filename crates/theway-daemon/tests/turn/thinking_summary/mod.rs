//! Tests for `turn::thinking_summary` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::time::Duration;
use tokio::sync::mpsc;

fn settings(min_chars: usize, summarizer: ThinkingSummarizerFn) -> ThinkingSummarySettings {
    ThinkingSummarySettings {
        min_chars,
        summarizer,
    }
}

fn ok_summarizer(prefix: &'static str) -> ThinkingSummarizerFn {
    Arc::new(move |text: String| {
        Box::pin(async move { Ok(format!("{prefix}:{text}")) })
    })
}

fn err_summarizer() -> ThinkingSummarizerFn {
    Arc::new(|_text: String| {
        Box::pin(async move { Err::<String, String>("boom".to_string()) })
    })
}

#[test]
fn thinking_delta_opens_burst_and_appends_feed() {
    // Arrange
    let mut feed = Feed::new();
    let mut burst = ThinkingBurst::default();
    let (feed_tx, _feed_rx) = mpsc::unbounded_channel();

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        None,
        &feed_tx,
        FeedUpdate::ThinkingDelta("raw thoughts".into()),
    );

    // Assert
    assert!(burst.open, "ThinkingDelta must open the burst");
    assert_eq!(feed.last_thinking_block(), Some((0, "raw thoughts".into())));
}

#[test]
fn thinking_summary_decrements_in_flight_and_backfills_feed() {
    // Arrange
    let mut feed = Feed::new();
    feed.apply(FeedUpdate::ThinkingDelta("raw thoughts".into()));
    let mut burst = ThinkingBurst {
        open: true,
        in_flight: 2,
    };
    let (feed_tx, _feed_rx) = mpsc::unbounded_channel();

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        None,
        &feed_tx,
        FeedUpdate::ThinkingSummary {
            block_index: 0,
            summary: "compressed".into(),
        },
    );

    // Assert
    assert_eq!(burst.in_flight, 1, "ThinkingSummary frees one slot");
    assert_eq!(feed.last_thinking_block(), Some((0, "compressed".into())));
}

#[tokio::test]
async fn closing_update_spawns_summary_and_sends_backfill() {
    // Arrange
    let mut feed = Feed::new();
    let mut burst = ThinkingBurst::default();
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel();
    let cfg = settings(1, ok_summarizer("sum"));

    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::ThinkingDelta("long enough".into()),
    );

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::TextDelta("next block".into()),
    );

    // Assert
    assert!(!burst.open, "non-thinking update closes the burst");
    assert_eq!(burst.in_flight, 1, "a summarizer task is in flight");

    let (session_id, msg) = tokio::time::timeout(Duration::from_millis(500), feed_rx.recv())
        .await
        .expect("summary must arrive within timeout")
        .expect("feed channel must stay open");
    assert_eq!(session_id, "sess-think");
    match msg {
        FeedUpdate::ThinkingSummary {
            block_index,
            summary,
        } => {
            assert_eq!(block_index, 0);
            assert_eq!(summary, "sum:long enough");
        }
        other => panic!("unexpected feed update: {other:?}"),
    }
}

#[tokio::test]
async fn closing_update_without_settings_does_not_spawn() {
    // Arrange
    let mut feed = Feed::new();
    let mut burst = ThinkingBurst::default();
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel();

    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        None,
        &feed_tx,
        FeedUpdate::ThinkingDelta("long enough".into()),
    );

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        None,
        &feed_tx,
        FeedUpdate::TextDelta("next block".into()),
    );

    // Assert
    assert!(!burst.open);
    assert_eq!(burst.in_flight, 0);
    assert!(feed_rx.try_recv().is_err(), "no summarizer must be spawned");
}

#[tokio::test]
async fn short_burst_does_not_spawn() {
    // Arrange
    let mut feed = Feed::new();
    let mut burst = ThinkingBurst::default();
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel();
    let cfg = settings(100, ok_summarizer("sum"));

    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::ThinkingDelta("short".into()),
    );

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::TextDelta("next block".into()),
    );

    // Assert
    assert!(!burst.open);
    assert_eq!(burst.in_flight, 0, "short bursts must stay raw");
    assert!(feed_rx.try_recv().is_err());
}

#[tokio::test]
async fn summarizer_error_sends_fallback_text() {
    // Arrange
    let mut feed = Feed::new();
    let mut burst = ThinkingBurst::default();
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel();
    let cfg = settings(1, err_summarizer());

    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::ThinkingDelta("long enough".into()),
    );

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::TextDelta("next block".into()),
    );

    // Assert
    let (_, msg) = tokio::time::timeout(Duration::from_millis(500), feed_rx.recv())
        .await
        .expect("fallback summary must arrive within timeout")
        .expect("feed channel must stay open");
    match msg {
        FeedUpdate::ThinkingSummary { summary, .. } => {
            assert_eq!(summary, "(thinking summary unavailable)");
        }
        other => panic!("unexpected feed update: {other:?}"),
    }
}

#[tokio::test]
async fn max_in_flight_skips_spawn_until_slot_frees() {
    // Arrange
    let mut feed = Feed::new();
    let mut burst = ThinkingBurst::default();
    let (feed_tx, mut feed_rx) = mpsc::unbounded_channel();
    let cfg = settings(1, ok_summarizer("sum"));

    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::ThinkingDelta("long enough".into()),
    );
    burst.in_flight = MAX_IN_FLIGHT;

    // Act
    apply(
        "sess-think",
        &mut feed,
        &mut burst,
        Some(&cfg),
        &feed_tx,
        FeedUpdate::TextDelta("next block".into()),
    );

    // Assert
    assert!(!burst.open);
    assert_eq!(burst.in_flight, MAX_IN_FLIGHT, "no slot freed");
    assert!(feed_rx.try_recv().is_err(), "no summarizer must be spawned");
}
