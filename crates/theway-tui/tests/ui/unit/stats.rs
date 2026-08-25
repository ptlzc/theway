// Unit tests for the busy-band session KV-cache stats formatter.

use super::stats::busy_stats_text_with_session;

#[test]
fn busy_stats_text_with_session_renders_kv_cache_metrics() {
    assert_eq!(
        busy_stats_text_with_session(84.0, 10_000, 7_000, 1_200),
        "84 char/s · input: 10.0k · cached: 7.0k · new: 3.0k · output: 1.2k · hit: 70%"
    );
}

#[test]
fn busy_stats_text_with_session_zero_input_renders_zero_hit() {
    assert_eq!(
        busy_stats_text_with_session(0.0, 0, 0, 0),
        "0 char/s · input: 0 · cached: 0 · new: 0 · output: 0 · hit: 0%"
    );
}
