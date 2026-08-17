//! Tests for `agent::messages` custom variant helpers — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use serde_json::json;

fn assert_custom(msg: &AgentMessage, expected_role: &str, expected_payload: serde_json::Value) {
    match msg {
        AgentMessage::Custom(custom) => {
            assert_eq!(custom.role, expected_role);
            assert_eq!(custom.payload, expected_payload);
            assert!(
                custom.timestamp > 0,
                "timestamp must be positive millis, got {}",
                custom.timestamp
            );
        }
        other => panic!("expected custom message, got {other:?}"),
    }
}

#[test]
fn compaction_summary_creates_expected_custom_message() {
    // Arrange
    let expected = json!({ "summary": "compaction summary text" });

    // Act
    let msg = compaction_summary("compaction summary text");

    // Assert
    assert_custom(&msg, "compaction_summary", expected);
}

#[test]
fn branch_summary_creates_expected_custom_message() {
    // Arrange
    let expected = json!({ "summary": "branch summary text" });

    // Act
    let msg = branch_summary("branch summary text");

    // Assert
    assert_custom(&msg, "branch_summary", expected);
}

#[test]
fn custom_uses_supplied_role_and_payload() {
    // Arrange
    let payload = json!({ "key": "value", "n": 42 });

    // Act
    let msg = custom("my_custom_role", payload.clone());

    // Assert
    assert_custom(&msg, "my_custom_role", payload);
}
