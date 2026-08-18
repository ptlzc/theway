//! Additional tests for `reload` — kept in a separate bridged module so the
//! original mirrored suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use once_cell::sync::OnceCell as SyncOnceCell;
use std::sync::Arc;
use theway_core::ToolExecutionMode;

#[test]
fn execution_mode_is_sequential() {
    let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
    let tool = ReloadTool::new(cell);

    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Sequential));
}
