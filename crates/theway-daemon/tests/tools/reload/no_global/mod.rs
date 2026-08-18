//! Tests for the `reload` tool's process-global runtime lookup when no runtime has been
//! installed (or a previous test has cleared it). Mirrored from `src/tools/reload.rs`.

use super::super::*;
use theway_core::AgentToolError;

#[tokio::test]
async fn execute_without_installed_runtime_returns_typed_error() {
    // Arrange: pin no runtime on the tool and make the process-global slot empty.
    let (_harness, cell) = super::super::tests::build_harness(false);
    let tool = ReloadTool::new(cell);
    let previous = current_runtime();
    *CURRENT_RUNTIME.lock() = None;

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

    // Restore the previous process-global runtime so other daemon tests that run after this
    // one still see an installed runtime.
    *CURRENT_RUNTIME.lock() = previous;
}
