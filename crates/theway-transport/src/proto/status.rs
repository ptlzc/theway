/// Convert the internal live status projection into the structured
/// `SessionSnapshot` wire model. This is the only legacy projection left: it
/// exists for stream frames and fallback fakes until the daemon composes a
/// `SessionObservabilityOps` implementation.
pub fn session_snapshot(snapshot: &WireStatus) -> wire::SessionSnapshot {
    let nested = WireSessionSnapshot::from(snapshot);
    wire_session_snapshot(&nested)
}

/// Project an authoritative snapshot into a per-subscriber incremental frame.
/// Non-feed fields remain complete; transcript fields contain only the rows
/// and block patches after that subscriber's cursors.
pub(crate) fn incremental_session_snapshot(
    snapshot: &WireStatus,
    delta: &WireFeedDelta,
    feed_lines_start: usize,
) -> wire::SessionSnapshot {
    let feed_lines_base = delta.feed_lines_base as usize;
    let suffix_start = feed_lines_start.saturating_sub(feed_lines_base);
    let mut nested = WireSessionSnapshot::from(snapshot);
    nested.feed.blocks = Vec::new();
    nested.feed.block_patches = delta.feed_block_patches.clone();
    nested.feed.lines = delta.feed_lines[suffix_start..].to_vec();
    nested.feed.blocks_base = delta.feed_blocks_base;
    nested.feed.lines_base = feed_lines_start as u64;
    wire_session_snapshot(&nested)
}
