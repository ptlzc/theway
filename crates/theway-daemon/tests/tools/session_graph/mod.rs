//! Tests for the `session_graph_*` tool bodies.

use crate::runtime_storage::SessionRepository;
use std::sync::Arc;
use theway_storage::sqlite_repo::SqliteSessionRepo;

#[test]
fn session_graph_tools_are_named_and_registered() {
    let dir = tempfile::tempdir().unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(SqliteSessionRepo::new(dir.path()));
    let tools = crate::tools::session_graph::SessionGraphTools::create(
        repo,
        dir.path().join("session_graph.db"),
        dir.path().to_path_buf(),
    );
    let names: Vec<String> = tools
        .iter()
        .map(|tool| tool.definition().name.clone())
        .collect();
    for expected in [
        "session_graph_list",
        "session_graph_read",
        "session_graph_status",
        "session_graph_wait",
        "session_graph_attach",
    ] {
        assert!(names.iter().any(|name| name == expected), "{names:?}");
    }
}
