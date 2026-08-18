//! Tests for `agent::compaction::algorithm` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use crate::agent::compaction::compaction::CompactionSettings;
use crate::agent::session::session::SessionTreeEntry;

fn settings() -> CompactionSettings {
    CompactionSettings::default()
}

#[test]
fn builtin_algorithm_name_is_builtin() {
    assert_eq!(BuiltinCompactAlgorithm.name(), "builtin");
}

#[test]
fn registry_default_is_empty_and_falls_back_to_builtin() {
    let registry = CompactAlgorithmRegistry::default();
    assert!(registry.custom_names().is_empty());

    let algorithm = registry.algorithm("builtin");
    assert_eq!(algorithm.name(), "builtin");

    let algorithm = registry.algorithm("");
    assert_eq!(algorithm.name(), "builtin");

    // Unknown names fall back to builtin and are never fatal.
    let algorithm = registry.algorithm("does-not-exist");
    assert_eq!(algorithm.name(), "builtin");
}

#[test]
fn registry_register_rejects_builtin_shadowing_and_lists_sorted_custom_names() {
    let mut registry = CompactAlgorithmRegistry::new();

    registry.register(Arc::new(BuiltinCompactAlgorithm));
    assert!(registry.custom_names().is_empty());

    registry.register(Arc::new(StubAlgorithm("z-algo")));
    registry.register(Arc::new(StubAlgorithm("a-algo")));
    assert_eq!(registry.custom_names(), vec!["a-algo", "z-algo"]);

    let algorithm = registry.algorithm("a-algo");
    assert_eq!(algorithm.name(), "a-algo");
}

struct StubAlgorithm(&'static str);

#[async_trait::async_trait]
impl CompactAlgorithm for StubAlgorithm {
    fn name(&self) -> &str {
        self.0
    }
}

#[tokio::test]
async fn trait_defaults_delegate_to_builtin_helpers() {
    let algorithm = StubAlgorithm("stub");
    let s = settings();

    // decide_compact default uses should_compact.
    assert!(!algorithm.decide_compact(1, 128_000, &s).await);
    assert!(algorithm.decide_compact(200_000, 128_000, &s).await);

    // select_cut_point default uses find_cut_point.
    let entries = vec![SessionTreeEntry::ThinkingLevelChange {
        id: "t".into(),
        parent_id: None,
        timestamp: "t".into(),
        thinking_level: "high".into(),
    }];
    let cut = algorithm.select_cut_point(&entries, &s).await;
    assert_eq!(cut.cut_index, 0);
}
