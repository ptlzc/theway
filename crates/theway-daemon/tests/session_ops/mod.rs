//! Tests for `session_ops` — split out of src (see docs/rust-test-files.md).
//!
//! Shared fixtures live here; each submodule covers one test face:
//! rolling summaries, lifecycle query/mutation ops, and collapse.

use std::collections::HashMap;
use std::sync::Arc;

use tempfile::tempdir;
use theway_contract::session::{SessionReader, SessionStore, StoredSessionEntry};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_storage::session_graph::SessionGraphStore;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::transport::SessionOps;
use theway_transport::wire::WireCollapseSessionRequest;

use crate::runtime_storage::SessionRepository;
use crate::session_execution::SessionExecutionRegistry;
use crate::session_ops::{
    AppSessionOps, ROLLING_SUMMARY_COMPONENTS, read_session_metadata, render_rolling_summary,
};

mod collapse;
mod lifecycle;
mod summary;

fn ops(repo: Arc<dyn SessionRepository>, _current_id: &str) -> AppSessionOps {
    let engine = Arc::new(DagEngine::new());
    AppSessionOps::new(repo, engine, "/cwd".into(), SessionExecutionRegistry::new())
}

async fn session_id_of(session: &(impl SessionReader + ?Sized)) -> String {
    session
        .get_metadata_json()
        .await
        .unwrap()
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string()
}
