//! Additional line-coverage tests for `agent::session::session` (see docs/rust-test-files.md).

use std::sync::Arc;

use super::super::*;
use crate::agent::session::memory_storage::MemorySessionStorage;

#[tokio::test]
async fn session_name_returns_none_when_latest_name_is_blank() {
    // Arrange
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    session.append_session_name("   ").await.unwrap();

    // Act
    let name = session.session_name().await.unwrap();

    // Assert
    assert_eq!(name, None);
}
