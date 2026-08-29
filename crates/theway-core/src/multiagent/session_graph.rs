//! Session-scoped subagent graph state and collapse material helpers.
//!
//! A session persists its live DAG/subagent projection as a `session_graph_state`
//! custom entry (like `goal_state`). Collapse turns that state plus the latest
//! compaction summary into the material stored on a collapse node, and can
//! optionally re-home active runs/jobs to a child session (`--adopt`).

use async_trait::async_trait;

use crate::agent::AgentRunError;
use crate::agent::assembly::AgentHarness;
pub use crate::agent::session::session::{
    SESSION_GRAPH_STATE_CUSTOM_TYPE, SessionGraphState, SubagentJobSnapshot,
};
use crate::agent::session::session::{Session, SessionTreeEntry};

use super::graph::engine::DagEngine;
use super::graph::persist::to_persisted;
use super::jobs::SubagentJobRegistry;

/// Conversion into an optional session id for snapshot/attach helpers.
pub trait IntoSessionId {
    fn into_session_id(self) -> Option<String>;
}

impl IntoSessionId for Option<&str> {
    fn into_session_id(self) -> Option<String> {
        self.map(str::to_string)
    }
}

impl IntoSessionId for &str {
    fn into_session_id(self) -> Option<String> {
        Some(self.to_string())
    }
}

impl IntoSessionId for Option<String> {
    fn into_session_id(self) -> Option<String> {
        self
    }
}

impl IntoSessionId for String {
    fn into_session_id(self) -> Option<String> {
        Some(self)
    }
}

impl IntoSessionId for &String {
    fn into_session_id(self) -> Option<String> {
        Some(self.clone())
    }
}

/// Build the live `SessionGraphState` for one session from engine + job registry.
pub fn snapshot_for_session<S: IntoSessionId>(
    engine: &DagEngine,
    jobs: &SubagentJobRegistry,
    session_id: S,
) -> SessionGraphState {
    let session_id = session_id.into_session_id();
    let session_id = session_id.as_deref();
    let dags = engine
        .list_runs()
        .into_iter()
        .filter(|run| run.session_id.as_deref() == session_id)
        .map(|run| to_persisted(&run))
        .collect();
    let subagents = jobs.snapshot_for_session(session_id);
    SessionGraphState { dags, subagents }
}

/// Source for [`collapse_material`].
#[async_trait]
pub trait CollapseMaterialSource {
    async fn compact_text(&self) -> String;
    async fn raw_text_ref(&self) -> String;
    async fn subagent_graph(&self) -> SessionGraphState;
}

#[async_trait]
impl CollapseMaterialSource for Session {
    async fn compact_text(&self) -> String {
        self.latest_collapse_summary()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    async fn raw_text_ref(&self) -> String {
        self.session_id().await.ok().flatten().unwrap_or_default()
    }

    async fn subagent_graph(&self) -> SessionGraphState {
        self.session_graph_state()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }
}

#[async_trait]
impl CollapseMaterialSource for SessionGraphState {
    async fn compact_text(&self) -> String {
        String::new()
    }

    async fn raw_text_ref(&self) -> String {
        String::new()
    }

    async fn subagent_graph(&self) -> SessionGraphState {
        self.clone()
    }
}

#[async_trait]
impl CollapseMaterialSource for &Session {
    async fn compact_text(&self) -> String {
        (**self)
            .latest_collapse_summary()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    async fn raw_text_ref(&self) -> String {
        (**self)
            .session_id()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    async fn subagent_graph(&self) -> SessionGraphState {
        (**self)
            .session_graph_state()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }
}

#[async_trait]
impl CollapseMaterialSource for &SessionGraphState {
    async fn compact_text(&self) -> String {
        String::new()
    }

    async fn raw_text_ref(&self) -> String {
        String::new()
    }

    async fn subagent_graph(&self) -> SessionGraphState {
        (*self).clone()
    }
}

/// Build collapse material from a source.
///
/// For a [`Session`] the compact text is read from the newest `Compaction` entry
/// (written by `AgentHarness::force_compact` / the existing `/compact` path), the
/// raw text handle is the source session id, and the subagent graph is the
/// persisted `session_graph_state` baseline. A bare [`SessionGraphState`] is also
/// accepted and is returned as the subagent graph with empty material fields.
pub async fn collapse_material<S: CollapseMaterialSource>(
    source: S,
) -> (String, String, SessionGraphState) {
    (
        source.compact_text().await,
        source.raw_text_ref().await,
        source.subagent_graph().await,
    )
}

/// Run the existing `force_compact` path and then build collapse material from
/// the same source session. This is the daemon-facing entry that reuses the
/// current `/compact` algorithm without changing its behavior.
pub async fn collapse_material_with_compaction(
    harness: &AgentHarness,
    custom_instructions: Option<String>,
) -> Result<(String, String, SessionGraphState), AgentRunError> {
    harness.force_compact(custom_instructions).await?;
    Ok(collapse_material(harness.session()).await)
}

/// Move all runs and jobs owned by `from_session` to `to_session`.
///
/// Returns the number of moved entities (runs + jobs). Used by collapse
/// `--adopt`; without it the old session keeps ownership and the new session can
/// still read/monitor through the persisted graph state.
pub fn attach_runs<S: IntoSessionId, T: IntoSessionId>(
    engine: &DagEngine,
    jobs: &SubagentJobRegistry,
    from_session: S,
    to_session: T,
) -> usize {
    let from = from_session.into_session_id();
    let to = to_session.into_session_id();
    let run_count = match (&from, &to) {
        (Some(from), Some(to)) => engine.rehome_runs(from, to),
        _ => 0,
    };
    let job_ids: Vec<String> = jobs
        .list()
        .into_iter()
        .filter(|job| job.session_id == from)
        .map(|job| job.id)
        .collect();
    for job_id in &job_ids {
        jobs.update(job_id, |job| {
            if job.session_id == from {
                job.session_id = to.clone();
            }
        });
    }
    run_count + job_ids.len()
}

/// Convenience for reading a persisted graph state from raw entries (used by
/// tests and daemon fallback paths).
pub fn graph_state_from_entries(entries: &[SessionTreeEntry]) -> Option<SessionGraphState> {
    crate::agent::session::session::latest_session_graph_state(entries)
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/session_graph");
