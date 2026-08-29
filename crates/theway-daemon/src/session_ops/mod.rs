//! Session resources (session-resource-model) — the app-side surface the transport
//! servers program against for the session lifecycle RPCs / HTTP routes.
//!
//! * [`SessionFactory`] — builds a fresh, fully-wired session runtime for any session id
//!   (resume semantics, the in-process version of CLI `--resume-id`). Built by the daemon
//!   orchestration layer (`orchestration::SessionRuntimeBuilder`) and carried by the transport
//!   host (`turn::daemon::TurnHost`).
//! * [`SessionOps`] — sync query/mutation ops that do NOT need the event loop
//!   (list / create / rename / delete). Sessions are addressed explicitly by id; there is
//!   no daemon-side session-switch operation.
//!
//! The daemon's transport host holds an `Arc<dyn SessionOps>` (via
//! [`theway_transport::TransportEndpoints`]) and never touches the session repo
//! directly, keeping the "transport programs only against the kernel's public surface" boundary.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::runtime_storage::SessionRepository;
use crate::session_execution::SessionExecutionRegistry;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::SubagentJobRegistry;

mod collapse;
mod graph;
mod lifecycle;
mod metadata;
mod ops;
mod wire;

pub use collapse::collapse_session_for_command;
pub(crate) use metadata::read_session_metadata;

#[cfg(test)]
pub(crate) use metadata::{ROLLING_SUMMARY_COMPONENTS, render_rolling_summary};

/// Builds a fresh, fully-wired [`crate::orchestration::SessionRuntime`] for the session identified by
/// the given id (resume semantics: full id or unique prefix, same as CLI `--resume-id`).
///
/// Async because opening the transcript, restoring per-session DAG state and rehydrating
/// the agent are all IO. The returned runtime keeps the harness and trigger executor
/// together so a session resume cannot retain session-scoped services from the previous
/// session.
pub type SessionFactory = Arc<
    dyn Fn(
            String,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<crate::orchestration::SessionRuntime>>
                    + Send,
            >,
        > + Send
        + Sync,
>;

/// Session lifecycle ops exposed to the transport servers (session-resource-model tasks
/// 3.5/3.6). All ops are repo-backed; `delete` additionally consults the DAG engine for
/// the delete-protection rule.
///
/// `SessionOps` over a cwd-scoped [`SessionRepository`] and the process DAG engine.
pub(crate) struct AppSessionOps {
    repo: Arc<dyn SessionRepository>,
    dag_engine: Arc<DagEngine>,
    cwd: String,
    session_execution: SessionExecutionRegistry,
    subagent_registry: Option<SubagentJobRegistry>,
    session_graph_path: Option<PathBuf>,
}

impl AppSessionOps {
    #[allow(dead_code)] // Used by mirrored tests; production uses `with_session_graph`.
    pub(crate) fn new(
        repo: Arc<dyn SessionRepository>,
        dag_engine: Arc<DagEngine>,
        cwd: String,
        session_execution: SessionExecutionRegistry,
    ) -> Self {
        Self {
            repo,
            dag_engine,
            cwd,
            session_execution,
            subagent_registry: None,
            session_graph_path: None,
        }
    }

    pub(crate) fn with_session_graph(
        repo: Arc<dyn SessionRepository>,
        dag_engine: Arc<DagEngine>,
        cwd: String,
        session_execution: SessionExecutionRegistry,
        subagent_registry: SubagentJobRegistry,
        session_graph_path: PathBuf,
    ) -> Self {
        Self {
            repo,
            dag_engine,
            cwd,
            session_execution,
            subagent_registry: Some(subagent_registry),
            session_graph_path: Some(session_graph_path),
        }
    }
}

#[cfg(test)]
// Test files live in `tests/session_ops/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("session_ops");
