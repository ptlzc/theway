// ── wire model pure helpers (config merge, status deltas, serde) ────

use crate::feed::Level;
use crate::wire::WireFeedBlockPatch;

fn plain_block(text: &str) -> WireFeedBlock {
    WireFeedBlock::Plain {
        text: text.into(),
        level: Level::System,
        timestamp: None,
    }
}

#[test]
fn wire_daemon_config_clears_and_unknown_fields() {
    let config = WireDaemonConfig {
        clear_fields: vec!["model".into(), "not-a-field".into()],
        ..Default::default()
    };
    assert!(config.clears("model"));
    assert!(!config.clears("provider"));
    assert_eq!(config.unknown_clear_fields(), vec!["not-a-field"]);
}

#[test]
fn wire_daemon_config_merge_applies_fields_and_clears() {
    let mut current = WireDaemonConfig {
        provider: Some("old-provider".into()),
        model: Some("old-model".into()),
        builtin_skills: vec!["old-skill".into()],
        skills_dirs: vec!["old-dir".into()],
        ..Default::default()
    };
    let patch = WireDaemonConfig {
        provider: Some("new-provider".into()),
        builtin_skills: vec!["a".into(), "b".into()],
        clear_fields: vec!["model".into(), "skills_dirs".into(), "unknown".into()],
        ..Default::default()
    };
    let touched = current.merge_from(&patch);
    assert_eq!(current.provider.as_deref(), Some("new-provider"));
    assert_eq!(current.model, None);
    assert_eq!(current.builtin_skills, vec!["a", "b"]);
    assert_eq!(current.skills_dirs, Vec::<String>::new());
    // provider + model clear + skills_dirs clear + builtin_skills set
    assert_eq!(touched, 4);
}

#[test]
fn wire_daemon_config_merge_counts_only_actual_clears() {
    let mut current = WireDaemonConfig::default();
    let patch = WireDaemonConfig {
        clear_fields: vec!["model".into(), "base_url".into()],
        ..Default::default()
    };
    // Nothing was present, so no touched areas for clears.
    assert_eq!(current.merge_from(&patch), 0);
}

#[test]
fn wire_daemon_config_serde_round_trips_partial_and_clear_fields() {
    let config = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        skills_dirs: vec!["/skills".into()],
        clear_fields: vec!["base_url".into()],
        ..Default::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: WireDaemonConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back, config);
    assert!(json.contains("clear_fields"));
}

#[test]
fn wire_status_update_full_resets_delta_cursors() {
    let mut status = fixture_status("hello");
    status.feed_blocks_base = 12;
    status.feed_block_patches = vec![WireFeedBlockPatch {
        index: 0,
        block: plain_block("patch"),
    }];
    status.feed_lines_base = 34;

    let update = WireStatusUpdate::full(status);
    assert!(update.full_status().is_some());
    assert!(update.feed_delta().is_none());
    let full = update.full_status().unwrap();
    assert_eq!(full.feed_blocks_base, 0);
    assert!(full.feed_block_patches.is_empty());
    assert_eq!(full.feed_lines_base, 0);
}

#[test]
fn wire_status_update_from_status_is_full() {
    let update = WireStatusUpdate::from(fixture_status("x"));
    assert!(update.full_status().is_some());
}

#[test]
fn wire_status_delta_apply_success() {
    let mut latest = fixture_status("old");
    let delta = WireStatusUpdate::delta(
        1,
        vec![WireFeedBlockPatch {
            index: 1,
            block: plain_block("new block"),
        }],
        2,
        1,
        vec!["new line".into()],
        2,
    );
    assert!(delta.apply_to(&mut latest));
    assert_eq!(latest.feed_blocks.len(), 2);
    assert!(matches!(&latest.feed_blocks[1], WireFeedBlock::Plain { text, .. } if text == "new block"));
    assert_eq!(latest.feed_lines, vec!["old", "new line"]);
}

#[test]
fn wire_status_delta_apply_reset_blocks() {
    let mut latest = fixture_status("old");
    let delta = WireStatusUpdate::delta(
        0,
        vec![WireFeedBlockPatch {
            index: 0,
            block: plain_block("fresh"),
        }],
        1,
        0,
        vec!["fresh line".into()],
        1,
    );
    assert!(delta.apply_to(&mut latest));
    assert_eq!(latest.feed_blocks.len(), 1);
    assert_eq!(latest.feed_lines, vec!["fresh line"]);
}

#[test]
fn wire_status_delta_apply_rejects_cursor_gaps() {
    let mut latest = fixture_status("old");

    // Block base does not match current length.
    let bad_base = WireStatusUpdate::delta(2, Vec::new(), 2, 0, Vec::new(), 0);
    assert!(!bad_base.apply_to(&mut latest));

    // Patch index jumps past projected length.
    let bad_index = WireStatusUpdate::delta(0, vec![WireFeedBlockPatch {
        index: 2,
        block: plain_block("x"),
    }], 1, 0, Vec::new(), 0);
    assert!(!bad_index.apply_to(&mut latest));

    // Replacement with a different block kind is rejected.
    let mut latest = fixture_status("old");
    let bad_kind = WireStatusUpdate::delta(1, vec![WireFeedBlockPatch {
        index: 0,
        block: plain_block("not user"),
    }], 1, 0, Vec::new(), 0);
    assert!(!bad_kind.apply_to(&mut latest));

    // Line cursor mismatch is rejected.
    let mut latest = fixture_status("old");
    let bad_lines = WireStatusUpdate::delta(0, Vec::new(), 0, 0, vec!["too many".into()], 3);
    assert!(!bad_lines.apply_to(&mut latest));
}