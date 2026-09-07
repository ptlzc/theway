//! Integration coverage for `agent::context_cache` public API.

use theway_core::{ContextCacheTracker, PrefixHitEstimate, canonical_context_bytes};
use theway_llm_provider::{Context as PiContext, Message, UserContent, UserMessage, UserRole};

fn context_with_body(body: &str) -> PiContext {
    PiContext {
        system_prompt: Some("system".into()),
        messages: vec![Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text(body.to_string()),
            timestamp: 0,
        })],
        tools: None,
    }
}

#[test]
fn finalize_with_zero_total_bytes_returns_zero_unknown_rate() {
    let tracker = ContextCacheTracker::new();
    let estimate = PrefixHitEstimate {
        overlap_bytes: 0,
        total_bytes: 0,
    };

    let result = tracker.finalize(&estimate, 100);

    assert_eq!(result.prefix_hit_tokens, 0);
    assert_eq!(result.prefix_cache_hit_rate, None);
}

#[test]
fn identical_long_contexts_match_every_chunk() {
    let mut tracker = ContextCacheTracker::new();
    let body = "x".repeat(1_000);
    let context = context_with_body(&body);

    tracker.estimate(Some("s"), "p", "m", &context);
    let estimate = tracker.estimate(Some("s"), "p", "m", &context);

    assert!(estimate.overlap_bytes > 512, "{}", estimate.overlap_bytes);
    assert!(estimate.overlap_bytes >= 256);
    let result = tracker.finalize(&estimate, 100);
    assert!(result.prefix_hit_tokens > 0);
}

#[test]
fn canonical_context_bytes_are_stable_for_identical_contexts() {
    let a = canonical_context_bytes(&context_with_body("same"));
    let b = canonical_context_bytes(&context_with_body("same"));
    assert_eq!(a, b);
}
