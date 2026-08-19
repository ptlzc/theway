//! Tests for `runtime_capabilities` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn active_hook_registrations_always_include_core_hook_points() {
    // Act
    let points = active_hook_registrations(0, false);

    // Assert
    assert_eq!(
        points,
        vec![
            "before_tool_call".to_string(),
            "on_control_plane_prompt".to_string(),
            "before_trigger_action".to_string(),
        ]
    );
}

#[test]
fn active_hook_registrations_adds_after_tool_call_when_lsp_is_wired() {
    // Act
    let points = active_hook_registrations(1, false);

    // Assert
    assert!(points.contains(&"after_tool_call".to_string()));
}

#[test]
fn active_hook_registrations_adds_cli_hooks_when_loaded() {
    // Act
    let points = active_hook_registrations(0, true);

    // Assert
    assert!(points.contains(&"cli_hooks".to_string()));
}

#[test]
fn active_trigger_features_lists_all_pipeline_behaviors() {
    // Act
    let features = active_trigger_features();

    // Assert
    assert_eq!(
        features,
        vec![
            "dedup".to_string(),
            "cycle suppress".to_string(),
            "fire-once rules".to_string(),
            "inject-and-run".to_string(),
        ]
    );
}
