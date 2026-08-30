use super::*;

fn feed_of(state: &theway_grpc::SessionSnapshot) -> &theway_grpc::SessionFeed {
    state.feed.as_ref().expect("snapshot feed")
}

#[test]
fn stream_snapshot_first_frame_is_authoritative() {
    let mut snapshot = fixture_snapshot("one");
    snapshot.feed_blocks = vec![plain_block("one")];
    snapshot.feed_block_patches = vec![WireFeedBlockPatch {
        index: 0,
        block: plain_block("one"),
    }];

    let update = WireStatusUpdate::full(snapshot.clone());
    let state = project_stream_snapshot(&update, &snapshot, &mut StreamCursor::default());
    let feed = feed_of(&state);

    assert_eq!(feed.blocks.len(), 1);
    assert!(feed.block_patches.is_empty());
    assert_eq!(feed.blocks_base, 0);
    assert_eq!(feed.lines, vec!["one"]);
}

#[test]
fn stream_snapshot_slices_normal_incremental_frame() {
    let mut cursor = StreamCursor::default();
    let mut first = fixture_snapshot("one");
    first.feed_blocks = vec![plain_block("one")];
    let first_update = WireStatusUpdate::full(first.clone());
    project_stream_snapshot(&first_update, &first, &mut cursor);

    let mut authoritative = fixture_snapshot("one");
    authoritative.feed_lines.push("two".into());
    authoritative.feed_blocks = vec![plain_block("one"), plain_block("two")];
    let mut delta = fixture_snapshot("two");
    delta.feed_blocks.clear();
    delta.feed_blocks_base = 1;
    delta.feed_lines_base = 1;
    delta.feed_block_patches = vec![WireFeedBlockPatch {
        index: 1,
        block: plain_block("two"),
    }];
    let update = WireStatusUpdate::delta_from_status(delta, 2, 2);
    let state = project_stream_snapshot(&update, &authoritative, &mut cursor);
    let feed = feed_of(&state);

    assert!(feed.blocks.is_empty());
    assert_eq!(feed.blocks_base, 1);
    assert_eq!(feed.block_patches.len(), 1);
    assert_eq!(feed.lines_base, 1);
    assert_eq!(feed.lines, vec!["two"]);
}

#[test]
fn stream_snapshot_resyncs_after_lag_or_clear() {
    let mut cursor = StreamCursor::default();
    let mut first = fixture_snapshot("one");
    first.feed_blocks = vec![plain_block("one"), plain_block("two")];
    let first_update = WireStatusUpdate::full(first.clone());
    project_stream_snapshot(&first_update, &first, &mut cursor);

    cursor.resync_pending = true;
    let mut unchanged = fixture_snapshot("");
    unchanged.feed_lines.clear();
    unchanged.feed_lines_base = 1;
    unchanged.feed_blocks_base = 2;
    let unchanged = WireStatusUpdate::delta_from_status(unchanged, 2, 1);
    let lagged = project_stream_snapshot(&unchanged, &first, &mut cursor);
    let lagged_feed = feed_of(&lagged);
    assert_eq!(lagged_feed.blocks.len(), 2);
    assert!(lagged_feed.block_patches.is_empty());

    let mut cleared = fixture_snapshot("new");
    cleared.feed_blocks = vec![plain_block("new")];
    cleared.feed_block_patches = vec![WireFeedBlockPatch {
        index: 0,
        block: plain_block("new"),
    }];
    let cleared_update = WireStatusUpdate::full(cleared.clone());
    let reset = project_stream_snapshot(&cleared_update, &cleared, &mut cursor);
    let reset_feed = feed_of(&reset);
    assert_eq!(reset_feed.blocks.len(), 1);
    assert!(reset_feed.block_patches.is_empty());
    assert_eq!(reset_feed.blocks_base, 0);
}
