//! Session-cumulative KV cache usage round-trips through `SessionState` and
//! `WireStatus`.
//!
//! The proto `SessionState.session_context_usage` field and the wire
//! `WireStatus.session_usage` field carry the same session-cumulative token
//! counters: total input, cached input, non-cached input, output, and cache
//! write totals.

use super::*;

#[test]
fn session_context_usage_round_trips_through_session_state_and_wire_status() {
    // Arrange: a full snapshot with session-cumulative usage populated.
    let mut snapshot = fixture_snapshot();
    snapshot.session_usage = WireContextUsage {
        cached_tokens: 800,
        new_tokens: 400,
        total_input_tokens: 1_200,
        output_tokens: 340,
        cache_write_tokens: 50,
        provider_cache_hit_rate: Some(800.0 / 1_200.0),
        prefix_cache_hit_rate: Some(0.75),
        prefix_hit_tokens: 900,
        context_window: 200_000,
    };

    // Act: convert WireStatus -> SessionState -> WireStatus.
    let proto = session_state(&snapshot);
    let restored = wire_status(&proto);

    // Assert: the proto carries the session usage and the round-trip preserves it.
    let proto_usage = proto
        .session_context_usage
        .as_ref()
        .expect("session_context_usage must be populated");
    assert_eq!(proto_usage.cached_tokens, 800);
    assert_eq!(proto_usage.new_tokens, 400);
    assert_eq!(proto_usage.total_input_tokens, 1_200);
    assert_eq!(proto_usage.output_tokens, 340);
    assert_eq!(proto_usage.cache_write_tokens, 50);
    assert_eq!(proto_usage.provider_cache_hit_rate, Some(800.0 / 1_200.0));
    assert_eq!(proto_usage.prefix_cache_hit_rate, Some(0.75));
    assert_eq!(proto_usage.prefix_hit_tokens, Some(900));
    assert_eq!(proto_usage.context_window, 200_000);

    assert_eq!(
        restored.session_usage.cached_tokens,
        snapshot.session_usage.cached_tokens
    );
    assert_eq!(
        restored.session_usage.new_tokens,
        snapshot.session_usage.new_tokens
    );
    assert_eq!(
        restored.session_usage.total_input_tokens,
        snapshot.session_usage.total_input_tokens
    );
    assert_eq!(
        restored.session_usage.output_tokens,
        snapshot.session_usage.output_tokens
    );
    assert_eq!(
        restored.session_usage.cache_write_tokens,
        snapshot.session_usage.cache_write_tokens
    );
    assert_eq!(
        restored.session_usage.provider_cache_hit_rate,
        snapshot.session_usage.provider_cache_hit_rate
    );
    assert_eq!(
        restored.session_usage.prefix_cache_hit_rate,
        snapshot.session_usage.prefix_cache_hit_rate
    );
    assert_eq!(
        restored.session_usage.prefix_hit_tokens,
        snapshot.session_usage.prefix_hit_tokens
    );
    assert_eq!(
        restored.session_usage.context_window,
        snapshot.session_usage.context_window
    );
}

#[test]
fn default_session_usage_round_trips_as_zero() {
    // Arrange: fixture already defaults `session_usage` to all-zero counters.

    // Act
    let restored = wire_status(&session_state(&fixture_snapshot()));

    // Assert
    assert_eq!(restored.session_usage.cached_tokens, 0);
    assert_eq!(restored.session_usage.new_tokens, 0);
    assert_eq!(restored.session_usage.total_input_tokens, 0);
    assert_eq!(restored.session_usage.output_tokens, 0);
    assert_eq!(restored.session_usage.cache_write_tokens, 0);
    assert_eq!(restored.session_usage.provider_cache_hit_rate, None);
    assert_eq!(restored.session_usage.prefix_cache_hit_rate, None);
    assert_eq!(restored.session_usage.prefix_hit_tokens, 0);
    assert_eq!(restored.session_usage.context_window, 0);
}
