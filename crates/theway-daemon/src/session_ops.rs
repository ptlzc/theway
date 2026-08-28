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

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use theway_contract::session::{SessionReader, SessionStore, StoredSessionEntry};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagStatus;

use crate::runtime_storage::SessionRepository;
use crate::session_execution::SessionExecutionRegistry;
use theway_transport::transport::SessionOps;
use theway_transport::wire::SessionSummary;

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
pub struct AppSessionOps {
    repo: Arc<dyn SessionRepository>,
    dag_engine: Arc<DagEngine>,
    cwd: String,
    session_execution: SessionExecutionRegistry,
}

impl AppSessionOps {
    pub fn new(
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
        }
    }
}

/// Read the latest `session_metadata` custom entry from a session transcript.
async fn read_session_metadata(session: &(impl SessionReader + ?Sized)) -> Result<HashMap<String, String>> {
    let entries = session.find_entries("custom").await?;
    let mut metadata = HashMap::new();
    for entry in entries {
        if entry.payload.get("customType").and_then(serde_json::Value::as_str) != Some("session_metadata") {
            continue;
        }
        if let Some(map) = entry
            .payload
            .get("metadata")
            .and_then(|value| serde_json::from_value::<HashMap<String, String>>(value.clone()).ok())
        {
            metadata.extend(map);
        }
    }
    Ok(metadata)
}

/// Persist a metadata map as a new `session_metadata` custom transcript entry.
async fn append_session_metadata(
    session: &(impl SessionStore + ?Sized),
    metadata: &HashMap<String, String>,
) -> Result<()> {
    let id = session.create_entry_id().await?;
    let parent_id = session.get_leaf_id().await?;
    let timestamp = chrono::Utc::now().to_rfc3339();
    let entry = StoredSessionEntry::from_payload(serde_json::json!({
        "type": "custom",
        "id": id,
        "parentId": parent_id,
        "timestamp": timestamp,
        "customType": "session_metadata",
        "metadata": metadata,
    }))?;
    session.append_entry(entry).await?;
    Ok(())
}

#[async_trait]
impl SessionOps for AppSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        let runs = self.dag_engine.list_runs();

        let mut summaries = Vec::new();
        for record in self.repo.list().await? {
            let session_runs = runs
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(record.id.as_str()));
            let graph_count = session_runs.clone().count() as u32;
            let active_graph_count = session_runs
                .filter(|run| run.status == DagStatus::Running)
                .count() as u32;

            let metadata = match self.repo.open(&record.id).await? {
                Some(session) => read_session_metadata(session.as_ref()).await?,
                None => HashMap::new(),
            };
            summaries.push(SessionSummary {
                session_id: record.id,
                name: record.name.unwrap_or_default(),
                cwd: record.cwd,
                model: record.model,
                created_at: record.created_at,
                last_activity_at: record.last_activity_at,
                graph_count,
                active_graph_count,
                busy: false,
                preview: record.preview,
                metadata,
            });
        }
        Ok(summaries)
    }

    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String> {
        // New sessions record the daemon work_dir so activation can resolve the
        // matching execution context.
        let cwd = if self.cwd.is_empty() {
            ".".to_string()
        } else {
            self.cwd.clone()
        };
        let session = self
            .repo
            .create_with_id(std::path::Path::new(&cwd), session_id)
            .await?;
        let meta = session.get_metadata_json().await?;
        let id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !metadata.is_empty() {
            append_session_metadata(session.as_ref(), metadata).await?;
        }
        Ok(id)
    }

    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()> {
        let session = self
            .repo
            .open(id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        let mut merged = read_session_metadata(session.as_ref()).await?;
        merged.extend(metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
        append_session_metadata(session.as_ref(), &merged).await?;
        Ok(())
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("session name must not be empty");
        }
        let session = self
            .repo
            .open(id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        theway_storage::session::append_session_name(session.as_ref(), name).await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        let session = self
            .repo
            .open(id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        let meta = session.get_metadata_json().await?;
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

        self.repo.delete(id).await?;
        // Credentials are memory-only and session-scoped; deleting the session
        // must also drop its zeroizing secrets and execution context.
        self.session_execution.remove(&session_id);
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use theway_contract::session::SessionReader;
    use theway_storage::sqlite_repo::SqliteSessionRepo;

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

    #[tokio::test]
    async fn list_returns_repo_summaries_without_live_current_flags() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let ops = ops(repo, &id);
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, id);
        assert!(!summaries[0].busy, "no daemon-side current busy flag");
        assert_eq!(summaries[0].graph_count, 0);
        assert_eq!(summaries[0].active_graph_count, 0);
    }

    #[tokio::test]
    async fn create_makes_new_session_with_inherited_cwd() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let first = repo.create("/cwd").await.unwrap();
        let first_id = session_id_of(&first).await;

        let ops = ops(repo.clone(), &first_id);
        let new_id = ops.create(None, &HashMap::new()).await.unwrap();
        assert_ne!(new_id, first_id);
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|s| s.cwd == "/cwd"));
    }

    #[tokio::test]
    async fn rename_round_trips_through_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let ops = ops(repo, &id);
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
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create(work.display().to_string()).await.unwrap();
        let id = session_id_of(&session).await;

        let ops = ops(repo.clone(), &id);
        ops.session_execution
            .set(
                id.clone(),
                theway_contract::session::SessionBinding {
                    client_key: "client-1".into(),
                    runtime: theway_contract::session::SessionRuntimeContext {
                        work_dir: work.display().to_string(),
                        provider: None,
                        model: None,
                        base_url: None,
                        thinking: None,
                    },
                },
            )
            .unwrap();
        ops.session_execution
            .set_credential(&id, "faux", b"sentinel".to_vec())
            .unwrap();
        let active = ops.delete(&id).await.unwrap();
        assert!(active.is_empty(), "no graphs → delete succeeds");
        assert!(ops.list().await.unwrap().is_empty());
        assert!(ops.session_execution.get_credential(&id, "faux").is_none());
        assert!(ops.session_execution.get(&id).is_none());
    }

    #[tokio::test]
    async fn delete_refuses_session_with_active_dag_run() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let engine = Arc::new(DagEngine::new());
        // A goal run is a real engine run; stamp it to this session like the goal hook does.
        let run_id = engine.plan_goal("test condition", Some(id.clone()));
        let ops = AppSessionOps::new(
            repo.clone(),
            engine.clone(),
            "/cwd".into(),
            SessionExecutionRegistry::new(),
        );

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

    #[tokio::test]
    async fn create_with_custom_id_and_metadata_round_trips_through_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        let mut metadata = HashMap::new();
        metadata.insert("tenant".to_string(), "acme".to_string());
        metadata.insert("source".to_string(), "workmate".to_string());

        let id = ops.create(Some("custom-session"), &metadata).await.unwrap();
        assert_eq!(id, "custom-session");

        let summaries = ops.list().await.unwrap();
        let summary = summaries
            .iter()
            .find(|s| s.session_id == "custom-session")
            .expect("custom session must be listed");
        assert_eq!(
            summary.metadata.get("tenant").map(String::as_str),
            Some("acme")
        );
        assert_eq!(
            summary.metadata.get("source").map(String::as_str),
            Some("workmate")
        );
    }

    #[tokio::test]
    async fn create_with_duplicate_custom_id_returns_already_exists() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        ops.create(Some("dup"), &HashMap::new()).await.unwrap();
        let err = ops
            .create(Some("dup"), &HashMap::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.to_lowercase().contains("already exists") || err.contains("exists"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn update_metadata_merges_and_appears_in_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        let mut initial = HashMap::new();
        initial.insert("tenant".to_string(), "acme".to_string());
        let id = ops.create(Some("meta-session"), &initial).await.unwrap();

        let mut update = HashMap::new();
        update.insert("env".to_string(), "prod".to_string());
        update.insert("tenant".to_string(), "globex".to_string());
        ops.update_metadata(&id, &update).await.unwrap();

        let summary = ops
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.session_id == id)
            .unwrap();
        assert_eq!(
            summary.metadata.get("tenant").map(String::as_str),
            Some("globex")
        );
        assert_eq!(
            summary.metadata.get("env").map(String::as_str),
            Some("prod")
        );
    }

    #[tokio::test]
    async fn update_metadata_unknown_session_returns_not_found() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        let err = ops
            .update_metadata("missing", &HashMap::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session matches"), "{err}");
    }

    #[tokio::test]
    async fn metadata_is_persisted_across_ops_instances() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let mut initial = HashMap::new();
        initial.insert("tenant".to_string(), "acme".to_string());

        let first = ops(repo.clone(), "current");
        let id = first
            .create(Some("persistent-meta"), &initial)
            .await
            .unwrap();

        let mut update = HashMap::new();
        update.insert("env".to_string(), "prod".to_string());
        first.update_metadata(&id, &update).await.unwrap();

        // A fresh AppSessionOps must read the same metadata from the repo, not
        // from an in-memory cache.
        let second = ops(repo, "current");
        let summary = second
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.session_id == id)
            .unwrap();
        assert_eq!(
            summary.metadata.get("tenant").map(String::as_str),
            Some("acme")
        );
        assert_eq!(
            summary.metadata.get("env").map(String::as_str),
            Some("prod")
        );
    }
}
