// Unit tests for the busy-band session token/cache stats formatter.

use super::stats::busy_stats_text_with_session;

#[test]
fn busy_stats_text_with_session_renders_token_and_cache_metrics() {
    assert_eq!(
        busy_stats_text_with_session(84.0, 10_000, 1_200, Some(0.7), Some(0.8)),
        "84 t/s · in: 10.0k · out: 1.2k · cache 70%"
    );
}

#[test]
fn busy_stats_text_with_session_falls_back_to_prefix_hit_rate() {
    // Provider reports no cache ratio → the client-side prefix estimate is
    // shown instead.
    assert_eq!(
        busy_stats_text_with_session(84.0, 10_000, 1_200, None, Some(0.8)),
        "84 t/s · in: 10.0k · out: 1.2k · cache 80%"
    );
}

#[test]
fn busy_stats_text_with_session_zero_input_renders_zero_hit() {
    assert_eq!(
        busy_stats_text_with_session(0.0, 0, 0, None, None),
        "0 t/s · in: 0 · out: 0 · cache -"
    );
}
