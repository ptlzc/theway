//! Session resources (session-resource-model) — the app-side surface the transport
//! servers program against for the session lifecycle RPCs / HTTP routes.
//!
//! Two pieces live here:
//!
//! * [`SessionFactory`] — builds a fresh, fully-wired `AgentHarness` for any session id
//!   (resume semantics, the in-process version of CLI `--resume-id`). Provided by the CLI
//!   crate through [`crate::ui::AppConfig`]; consumed by [`crate::ui::App::switch_session`]
//!   inside the serialized event loop.
//! * [`SessionOps`] — sync query/mutation ops that do NOT need the event loop
//!   (list / create / rename / delete). Switching the *current* session is deliberately
//!   NOT here: it mutates App runtime state (kernel harness, feed, busy flag) and must go
//!   through `WebCommand::SwitchSession` on the serialized loop.
//!
//! The server crate holds an `Arc<dyn SessionOps>` (via
//! [`theway_transport::TransportEndpoints`]) and never touches the session repo
//! directly, keeping the "server programs only against the app's public surface" boundary.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use parking_lot::Mutex;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagStatus;
use theway_core::{JsonlSessionRepo, SessionTreeEntry};

use theway_transport::transport::SessionOps;
use theway_transport::wire::SessionSummary;

/// Builds a fresh, fully-wired [`theway_core::AgentHarness`] for the session identified by
/// the given id (resume semantics: full id or unique prefix, same as CLI `--resume-id`).
///
/// Async because opening the transcript, restoring per-session DAG state and rehydrating
/// the agent are all IO. The returned harness carries its own session-stamped tools
/// (dag_* / task), listeners and notification hooks; the process-level pieces (DAG engine,
/// subagent registry, feed channel) are shared with the original harness.
pub type SessionFactory = Arc<
    dyn Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Arc<theway_core::AgentHarness>>> + Send>,
        > + Send
        + Sync,
>;

/// Live "current session" state shared between the App event loop and [`AppSessionOps`].
///
/// The transport loop syncs it on every published snapshot (and on session switch), so
/// `SessionOps::list` can report `busy` / `model` for the current session without reaching
/// into `App` internals.
#[derive(Clone, Debug, Default)]
pub struct CurrentSessionState {
    pub session_id: String,
    pub busy: bool,
    pub model: String,
    pub cwd: String,
}

/// Session lifecycle ops exposed to the transport servers (session-resource-model tasks
/// 3.5/3.6). All ops are repo-backed; `delete` additionally consults the DAG engine for
/// the delete-protection rule.
///
/// `SessionOps` over the cwd-scoped [`JsonlSessionRepo`] + the process-wide [`DagEngine`].
pub struct AppSessionOps {
    repo: Arc<JsonlSessionRepo>,
    dag_engine: Arc<DagEngine>,
    current: Arc<Mutex<CurrentSessionState>>,
}

impl AppSessionOps {
    pub fn new(
        repo: Arc<JsonlSessionRepo>,
        dag_engine: Arc<DagEngine>,
        current: Arc<Mutex<CurrentSessionState>>,
    ) -> Self {
        Self {
            repo,
            dag_engine,
            current,
        }
    }
}

#[async_trait]
impl SessionOps for AppSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        let current = self.current.lock().clone();
        let runs = self.dag_engine.list_runs();

        let mut summaries = Vec::new();
        for path in self.repo.list().await.map_err(repo_err)? {
            let session = self.repo.open(&path).await.map_err(repo_err)?;
            let meta = session
                .storage()
                .get_metadata_json()
                .await
                .map_err(repo_err)?;
            let session_id = meta
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let created_at = meta
                .get("createdAt")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let cwd = meta
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let name = session
                .session_name()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let preview = crate::session::first_user_text(&session).await;
            let model = last_model_change(&session).await;

            // Last activity: transcript mtime (epoch millis). Cheap, and a session is only
            // ever written when something actually happened in it.
            let last_activity_at = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            let session_runs = runs
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(session_id.as_str()));
            let graph_count = session_runs.clone().count() as u32;
            let active_graph_count = session_runs
                .filter(|run| run.status == DagStatus::Running)
                .count() as u32;

            let is_current = current.session_id == session_id;
            summaries.push(SessionSummary {
                session_id,
                name,
                cwd,
                model: if is_current && !current.model.is_empty() {
                    current.model.clone()
                } else {
                    model
                },
                created_at,
                last_activity_at,
                graph_count,
                active_graph_count,
                busy: is_current && current.busy,
                preview,
            });
        }
        Ok(summaries)
    }

    async fn create(&self) -> Result<String> {
        let cwd = {
            let state = self.current.lock();
            if state.cwd.is_empty() {
                ".".to_string()
            } else {
                state.cwd.clone()
            }
        };
        let session = self.repo.create(cwd).await.map_err(repo_err)?;
        let meta = session
            .storage()
            .get_metadata_json()
            .await
            .map_err(repo_err)?;
        Ok(meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string())
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("session name must not be empty");
        }
        let path = crate::session::find_path_by_id(&self.repo, id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        let session = self.repo.open(&path).await.map_err(repo_err)?;
        session.append_session_name(name).await.map_err(repo_err)?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        let path = crate::session::find_path_by_id(&self.repo, id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        // Resolve the metadata id first: DAG runs are stamped with it, not the file stem.
        let session = self.repo.open(&path).await.map_err(repo_err)?;
        let meta = session
            .storage()
            .get_metadata_json()
            .await
            .map_err(repo_err)?;
        let session_id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        // Delete protection: refuse while any of this session's DAG runs is still active.
        let active: Vec<String> = self
            .dag_engine
            .list_runs()
            .iter()
            .filter(|run| {
                run.session_id.as_deref() == Some(session_id.as_str())
                    && run.status == DagStatus::Running
            })
            .map(|run| run.id.clone())
            .collect();
        if !active.is_empty() {
            return Ok(active);
        }

        crate::session::delete_by_id(&self.repo, id).await?;
        Ok(Vec::new())
    }
}

/// Last recorded model in the transcript (`provider:model-id`), or "" when the session
/// never switched models explicitly.
async fn last_model_change(session: &theway_core::Session) -> String {
    let Ok(entries) = session.entries().await else {
        return String::new();
    };
    let mut model = String::new();
    for entry in entries {
        if let SessionTreeEntry::ModelChange {
            provider, model_id, ..
        } = entry
        {
            model = format!("{provider}:{model_id}");
        }
    }
    model
}

fn repo_err(e: theway_core::SessionError) -> anyhow::Error {
    anyhow::Error::msg(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ops(
        repo: Arc<JsonlSessionRepo>,
        current_id: &str,
    ) -> (AppSessionOps, Arc<Mutex<CurrentSessionState>>) {
        let engine = Arc::new(DagEngine::new());
        let current = Arc::new(Mutex::new(CurrentSessionState {
            session_id: current_id.to_string(),
            busy: true,
            model: "faux:current".into(),
            cwd: "/cwd".into(),
        }));
        let ops = AppSessionOps::new(repo, engine.clone(), current.clone());
        (ops, current)
    }

    async fn session_id_of(session: &theway_core::Session) -> String {
        session
            .storage()
            .get_metadata_json()
            .await
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn list_reports_current_session_busy_with_live_model() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(JsonlSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let (ops, _current) = ops(repo, &id);
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, id);
        assert!(summaries[0].busy, "current session must report busy");
        assert_eq!(summaries[0].model, "faux:current");
        assert_eq!(summaries[0].graph_count, 0);
        assert_eq!(summaries[0].active_graph_count, 0);
    }

    #[tokio::test]
    async fn create_makes_new_session_with_inherited_cwd() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(JsonlSessionRepo::new(dir.path()));
        let first = repo.create("/cwd").await.unwrap();
        let first_id = session_id_of(&first).await;

        let (ops, _current) = ops(repo.clone(), &first_id);
        let new_id = ops.create().await.unwrap();
        assert_ne!(new_id, first_id);
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|s| s.cwd == "/cwd"));
    }

    #[tokio::test]
    async fn rename_round_trips_through_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(JsonlSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let (ops, _current) = ops(repo, &id);
        ops.rename(&id, "  my session  ").await.unwrap();
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries[0].name, "my session");

        let err = ops.rename(&id, "   ").await.unwrap_err().to_string();
        assert!(err.contains("must not be empty"), "{err}");
        let err = ops
            .rename("no-such-session", "x")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session matches"), "{err}");
    }

    #[tokio::test]
    async fn delete_removes_session_when_no_active_graphs() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(JsonlSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let (ops, _current) = ops(repo.clone(), &id);
        let active = ops.delete(&id).await.unwrap();
        assert!(active.is_empty(), "no graphs → delete succeeds");
        assert!(ops.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_refuses_session_with_active_dag_run() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(JsonlSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let engine = Arc::new(DagEngine::new());
        // A goal run is a real engine run; stamp it to this session like the goal hook does.
        let run_id = engine.plan_goal("test condition", Some(id.clone()));
        let current = Arc::new(Mutex::new(CurrentSessionState {
            session_id: id.clone(),
            busy: false,
            model: String::new(),
            cwd: "/cwd".into(),
        }));
        let ops = AppSessionOps::new(repo.clone(), engine.clone(), current);

        let active = ops.delete(&id).await.unwrap();
        assert_eq!(
            active,
            vec![run_id.clone()],
            "active run must refuse the delete"
        );
        assert_eq!(ops.list().await.unwrap().len(), 1, "session must survive");

        // Terminal run → protection lifts.
        engine.cancel_run(&run_id, Some("test cleanup"));
        let active = ops.delete(&id).await.unwrap();
        assert!(active.is_empty(), "aborted run must not block delete");
        assert!(ops.list().await.unwrap().is_empty());
    }
}
