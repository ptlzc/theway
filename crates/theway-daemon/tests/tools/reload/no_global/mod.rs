//! Tests for an unbound application-owned reload runtime slot.

use super::super::*;
use theway_core::AgentToolError;

#[tokio::test]
async fn execute_without_installed_runtime_returns_typed_error() {
    let (_harness, cell) = super::super::tests::build_harness(false);
    let tool = ReloadTool::new(cell, ReloadRuntimeSlot::default());

    // Act
    let result = tool
        .execute(
            "c1",
            serde_json::json!({}),
            CancellationToken::new(),
            None,
        )
        .await;

    // Assert: typed error, no panic.
    assert!(matches!(
        result,
        Err(AgentToolError::Message(ref m)) if m.contains("runtime not installed")
    ));
}
