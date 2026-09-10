use theway_transport::feed::WireFeedBlock;

use crate::ui::feed_metrics::{feed_text_bytes, feed_text_tokens};

/// Feed meters enumerate every block kind: a `Context` row counts both its
/// label and its body, so pre-injected content shows up in the busy-band
/// bytes/tokens meters like any other rendered text.
#[test]
fn feed_meters_count_context_rows() {
    let blocks = vec![
        WireFeedBlock::Context {
            label: "skill:git".into(),
            text: "abcdefgh".into(),
            timestamp: None,
        },
        WireFeedBlock::Assistant {
            text: "reply".into(),
            timestamp: None,
        },
    ];
    assert_eq!(
        feed_text_bytes(&blocks),
        "skill:git".len() + 8 + "reply".len()
    );
    assert!(feed_text_tokens(&blocks) > 0);
}
