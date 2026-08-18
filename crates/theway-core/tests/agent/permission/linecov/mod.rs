//! Additional line-coverage tests for `agent::permission` (see docs/rust-test-files.md).

use super::super::*;

fn args(cmd: &str) -> serde_json::Value {
    serde_json::json!({ "command": cmd })
}

#[test]
fn default_policy_is_constructed_via_default_impl() {
    let policy = PermissionPolicy::default();

    assert!(matches!(
        policy.evaluate("bash", &args("ls -la")),
        PermissionDecision::Allow
    ));
}

#[test]
fn rm_classifier_handles_unknown_long_flag_before_recursive_force() {
    let policy = PermissionPolicy::default_for_coding_agent();

    let decision = policy.evaluate("bash", &args("rm --preserve-root -rf /etc"));

    assert!(matches!(decision, PermissionDecision::Deny { .. }));
}
