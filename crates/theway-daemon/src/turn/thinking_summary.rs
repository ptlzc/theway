//! Thinking summarization (`[orchestrator] thinking_summary`): when a
//! finished thinking burst is long enough, hand it to a background subagent
//! that compresses it into a structured summary and backfills the feed block.
//!
//! The raw thinking stays in the session transcript; only the display feed
//! block is replaced. Summaries run detached from the turn loop, so a slow
//! summarizer never blocks streaming.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use theway_transport::feed::{Feed, FeedUpdate};
use tokio::sync::mpsc;

/// Compresses one finished thinking burst into a structured summary.
/// Daemon-side: the `thewayd` binary wires this to a tool-less subagent run
/// (`run_agent` with the "general" spec and an empty tool set).
pub type ThinkingSummarizerFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
>;

#[derive(Clone)]
pub struct ThinkingSummarySettings {
    /// Minimum thinking text length (chars) that triggers summarization.
    pub min_chars: usize,
    pub summarizer: ThinkingSummarizerFn,
}

/// Max concurrent in-flight summarizer runs per session; extra bursts stay
/// raw in the feed until a slot frees up.
const MAX_IN_FLIGHT: usize = 4;

/// Burst-tracking state owned by `TurnHost`.
#[derive(Default)]
pub struct ThinkingBurst {
    /// A `ThinkingDelta` arrived and no other update closed the burst yet.
    pub open: bool,
    /// Summarizer tasks currently running.
    pub in_flight: usize,
}

/// Apply one non-trigger [`FeedUpdate`], closing an open thinking burst and
/// possibly spawning a summarizer task. Backfill updates are sent back over
/// `feed_tx` so the host loop publishes a fresh snapshot.
pub fn apply(
    feed: &mut Feed,
    burst: &mut ThinkingBurst,
    settings: Option<&ThinkingSummarySettings>,
    feed_tx: &mpsc::UnboundedSender<FeedUpdate>,
    update: FeedUpdate,
) {
    match update {
        FeedUpdate::ThinkingDelta(_) => {
            burst.open = true;
            feed.apply(update);
        }
        FeedUpdate::ThinkingSummary { .. } => {
            feed.apply(update);
            burst.in_flight = burst.in_flight.saturating_sub(1);
        }
        other => {
            let was_open = burst.open;
            burst.open = false;
            feed.apply(other);
            if was_open
                && let Some(settings) = settings
                && burst.in_flight < MAX_IN_FLIGHT
                && let Some((index, text)) = feed.last_thinking_block()
                && text.len() >= settings.min_chars
            {
                burst.in_flight += 1;
                let summarizer = settings.summarizer.clone();
                let feed_tx = feed_tx.clone();
                tokio::spawn(async move {
                    let summary = summarizer(text)
                        .await
                        .unwrap_or_else(|_| "(thinking summary unavailable)".to_string());
                    let _ = feed_tx.send(FeedUpdate::ThinkingSummary {
                        block_index: index,
                        summary,
                    });
                });
            }
        }
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("turn/thinking_summary");
