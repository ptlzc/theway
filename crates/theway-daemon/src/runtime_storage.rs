//! Daemon persistence ports and their local/controller-backed adapters.
//!
//! [`SessionRepository`] hides transcript repository implementations from application
//! orchestration. [`RuntimeStorage`] owns DAG, trigger, cron, and subagent transcript state.
//! The controller-backed adapter delegates DAG and automation state over gRPC while using the
//! local session repository adapter for transcripts.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex as ParkingMutex;
use theway_contract::dag::PersistedRun;
use theway_contract::session::{SessionReader, SessionStore, StoredSessionEntry};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::{DagPersistSink, to_persisted};
use theway_core::multiagent::graph::types::DagStatus;
use theway_core::multiagent::jobs::JobTranscriptStore;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_storage::sqlite_storage::SqliteSessionStorage;
use theway_transport::client::GrpcClient;
use theway_transport::triggers::{CronJob, DynamicTriggerRule};
use theway_transport::wire::{
    WireLoadCronJobsRequest, WireLoadDagRunsRequest, WireLoadTriggerRulesRequest,
    WireSaveCronJobsRequest, WireSaveDagRunRequest, WireSaveTriggerRulesRequest, WireStoredCronJob,
    WireStoredTriggerRule,
};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::dag_persist::{self, DagPersistHandle};
use crate::job_transcripts::{DiskTranscriptStore, MemoryTranscriptStore};
use crate::session_execution::SessionExecutionRegistry;
use crate::triggers::cron::{read_jobs_file, write_jobs_file};
use crate::triggers::dynamic::{read_rules_file, write_rules_file};

/// Persistent runtime state operations owned by the daemon.
#[async_trait]
pub trait RuntimeStorage: Send + Sync {
    /// Open the application-facing session repository for `cwd`.
    async fn session_repository(&self, cwd: &Path) -> Result<Arc<dyn SessionRepository>>;

    /// Return the job-transcript store for this runtime storage backend.
    ///
    /// Issue #86: local storage keeps the disk store; controller-backed
    /// storage uses an in-memory store until a transcript RPC exists.
    fn job_transcript_store(&self, cwd: &Path) -> Arc<dyn JobTranscriptStore>;

    /// Load persisted DAG runs for a session.
    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>>;

    /// Spawn a DAG persistence sink for the engine.
    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<dyn DagPersistSink>;

    /// Spawn DAG persistence with per-session routing. The default preserves
    /// legacy/global behavior; local storage routes stores by registered cwd.
    #[allow(private_interfaces)]
    fn spawn_dag_persist_for_sessions(
        &self,
        engine: Arc<DagEngine>,
        cwd: PathBuf,
        _sessions: SessionExecutionRegistry,
    ) -> Arc<dyn DagPersistSink> {
        self.spawn_dag_persist(engine, cwd)
    }

    /// Load dynamic trigger rules for a session through the storage seam.
    async fn load_dynamic_triggers(
        &self,
        cwd: &Path,
        session_id: &str,
    ) -> Result<Vec<DynamicTriggerRule>>;

    /// Persist dynamic trigger rules for a session through the storage seam.
    async fn save_dynamic_triggers(
        &self,
        cwd: &Path,
        session_id: &str,
        rules: &[DynamicTriggerRule],
    ) -> Result<()>;

    /// Load cron jobs for a session through the storage seam.
    async fn load_cron_jobs(&self, cwd: &Path, session_id: &str) -> Result<Vec<CronJob>>;

    /// Persist cron jobs for a session through the storage seam.
    async fn save_cron_jobs(&self, cwd: &Path, session_id: &str, jobs: &[CronJob]) -> Result<()>;
}

/// Session metadata used by daemon application services. Persistence handles and database
/// implementation types remain behind [`SessionRepository`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub created_at: String,
    pub preview: Option<String>,
    pub tree_prefix: String,
    pub name: Option<String>,
    pub cwd: String,
    pub model: String,
    pub last_activity_at: i64,
    pub automation: AutomationCounts,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutomationCounts {
    pub cron_enabled: usize,
    pub cron_total: usize,
    pub trigger_enabled: usize,
    pub trigger_total: usize,
}

impl AutomationCounts {
    pub fn any_enabled(&self) -> bool {
        self.cron_enabled > 0 || self.trigger_enabled > 0
    }

    pub fn badge(&self) -> Option<String> {
        if self.cron_total == 0 && self.trigger_total == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if self.cron_enabled > 0 {
            parts.push(format!("{} cron", self.cron_enabled));
        }
        if self.trigger_enabled > 0 {
            parts.push(format!("{} trigger", self.trigger_enabled));
        }
        Some(if parts.is_empty() {
            "automation off".into()
        } else {
            parts.join(", ")
        })
    }
}

#[derive(Debug)]
pub struct SessionImport {
    pub session_id: String,
    pub session_path: PathBuf,
    pub entry_count: usize,
    pub triggers_imported: usize,
    pub cron_imported: usize,
    pub automation_enabled: bool,
    pub originally_enabled_triggers: Vec<String>,
    pub originally_enabled_cron: Vec<String>,
}

/// Cwd-scoped session catalog and transcript lifecycle port used by daemon application code.
#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn create(&self, cwd: &Path) -> Result<Arc<dyn SessionStore>>;
    /// Mint a session WITHOUT writing the db file (issue #46); the file is
    /// materialized on the first real write so an idle TUI leaves no empty
    /// conversation behind. Defaults to eager [`Self::create`] so
    /// remote/controller-backed repositories keep their current behavior.
    async fn create_lazy(&self, cwd: &Path) -> Result<Arc<dyn SessionStore>> {
        self.create(cwd).await
    }
    async fn create_with_id(&self, cwd: &Path, _id: Option<&str>) -> Result<Arc<dyn SessionStore>> {
        self.create(cwd).await
    }
    /// Create a collapsed child session with compact entries and collapse
    /// metadata. The default delegates to a normal create for remote adapters
    /// that do not implement collapse-aware session creation yet.
    async fn create_collapsed_child(
        &self,
        cwd: &Path,
        parent: &dyn SessionStore,
        entries: Vec<StoredSessionEntry>,
        collapse_node_id: String,
    ) -> Result<Arc<dyn SessionStore>> {
        let child = self.create(cwd).await?;
        child.append_entries(entries).await?;
        child
            .set_collapse_node_id(Some(collapse_node_id.clone()))
            .await?;
        parent
            .set_collapse_node_id(Some(collapse_node_id.clone()))
            .await?;
        parent.set_collapsed(true).await?;
        Ok(child)
    }
    async fn resume(&self, explicit_id: Option<&str>) -> Result<Arc<dyn SessionStore>>;
    async fn contains(&self, id: &str) -> Result<bool>;
    async fn open(&self, id: &str) -> Result<Option<Arc<dyn SessionStore>>>;
    async fn list(&self) -> Result<Vec<SessionRecord>>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn fork(
        &self,
        cwd: &Path,
        parent: &theway_core::Session,
        entries: Vec<StoredSessionEntry>,
    ) -> Result<Arc<dyn SessionStore>>;
    async fn import(&self, archive_path: &Path, cwd: &Path) -> Result<SessionImport>;
}

#[async_trait]
impl SessionRepository for SqliteSessionRepo {
    async fn create(&self, cwd: &Path) -> Result<Arc<dyn SessionStore>> {
        Ok(Arc::new(theway_storage::session::create(self, cwd).await?))
    }

    async fn create_lazy(&self, cwd: &Path) -> Result<Arc<dyn SessionStore>> {
        Ok(Arc::new(
            theway_storage::session::create_lazy(self, cwd).await?,
        ))
    }

    async fn create_with_id(&self, cwd: &Path, id: Option<&str>) -> Result<Arc<dyn SessionStore>> {
        let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) else {
            return SessionRepository::create(self, cwd).await;
        };
        tokio::fs::create_dir_all(self.root()).await?;
        let path = self.root().join(format!("{id}.db"));
        Ok(Arc::new(
            SqliteSessionStorage::create_with_id(
                path,
                cwd.to_string_lossy().to_string(),
                Some(id.to_string()),
            )
            .await?,
        ))
    }

    async fn create_collapsed_child(
        &self,
        cwd: &Path,
        parent: &dyn SessionStore,
        entries: Vec<StoredSessionEntry>,
        collapse_node_id: String,
    ) -> Result<Arc<dyn SessionStore>> {
        Ok(Arc::new(
            theway_storage::session::create_collapsed_child(
                self,
                cwd,
                parent,
                entries,
                collapse_node_id,
            )
            .await?,
        ))
    }

    async fn resume(&self, explicit_id: Option<&str>) -> Result<Arc<dyn SessionStore>> {
        Ok(Arc::new(
            theway_storage::session::resume(self, explicit_id).await?,
        ))
    }

    async fn contains(&self, id: &str) -> Result<bool> {
        Ok(theway_storage::session::find_path_by_id(self, id)
            .await?
            .is_some())
    }

    async fn open(&self, id: &str) -> Result<Option<Arc<dyn SessionStore>>> {
        let Some(path) = theway_storage::session::find_path_by_id(self, id).await? else {
            return Ok(None);
        };
        Ok(Some(Arc::new(SqliteSessionRepo::open(self, path).await?)))
    }

    async fn list(&self) -> Result<Vec<SessionRecord>> {
        let entries = theway_storage::session::list_entries(self).await?;
        let entries = theway_storage::session::flatten_session_tree(&entries);
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            let (name, cwd, model) = match SqliteSessionRepo::open(self, &entry.path).await {
                Ok(session) => {
                    let metadata = session.get_metadata_json().await.unwrap_or_default();
                    (
                        theway_storage::session::session_name(&session).await,
                        metadata
                            .get("cwd")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        theway_storage::session::last_model_change(&session).await,
                    )
                }
                Err(_) => (None, String::new(), String::new()),
            };
            let last_activity_at = tokio::fs::metadata(&entry.path)
                .await
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            records.push(SessionRecord {
                id: entry.id,
                created_at: entry.created_at,
                preview: entry.preview,
                tree_prefix: entry.prefix,
                name,
                cwd,
                model,
                last_activity_at,
                automation: AutomationCounts {
                    cron_enabled: entry.automation.cron_enabled,
                    cron_total: entry.automation.cron_total,
                    trigger_enabled: entry.automation.trigger_enabled,
                    trigger_total: entry.automation.trigger_total,
                },
            });
        }
        Ok(records)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        theway_storage::session::delete_by_id(self, id).await?;
        Ok(())
    }

    async fn fork(
        &self,
        cwd: &Path,
        parent: &theway_core::Session,
        entries: Vec<StoredSessionEntry>,
    ) -> Result<Arc<dyn SessionStore>> {
        Ok(Arc::new(
            theway_storage::session::fork_session(self, cwd, parent, entries).await?,
        ))
    }

    async fn import(&self, archive_path: &Path, cwd: &Path) -> Result<SessionImport> {
        let summary = theway_storage::session_archive::import_session(
            self,
            archive_path,
            cwd,
            theway_storage::session_archive::ActivateTriggers::Off,
        )
        .await?;
        Ok(SessionImport {
            session_id: summary.session_id,
            session_path: summary.session_path,
            entry_count: summary.entry_count,
            triggers_imported: summary.triggers_imported,
            cron_imported: summary.cron_imported,
            automation_enabled: summary.automation_enabled,
            originally_enabled_triggers: summary.originally_enabled_triggers,
            originally_enabled_cron: summary.originally_enabled_cron,
        })
    }
}

pub fn automation_elsewhere_hint(
    records: &[SessionRecord],
    current_session_id: &str,
) -> Option<String> {
    let mut holders = records
        .iter()
        .filter(|record| record.id != current_session_id && record.automation.any_enabled())
        .collect::<Vec<_>>();
    let extra = holders.len().saturating_sub(1);
    let record = holders.pop()?;
    let short_id = record.id.chars().take(16).collect::<String>();
    let badge = record.automation.badge().unwrap_or_default();
    let more = if extra > 0 {
        format!(" (+{extra} more session(s))")
    } else {
        String::new()
    };
    Some(format!(
        "automation is session-scoped: session {short_id} has {badge} enabled{more}; resume it with `theway --resume-id {short_id}`"
    ))
}

/// Local filesystem/SQLite implementation of [`RuntimeStorage`].
#[derive(Clone, Copy, Default)]
pub struct LocalRuntimeStorage;

#[async_trait]
impl RuntimeStorage for LocalRuntimeStorage {
    async fn session_repository(&self, cwd: &Path) -> Result<Arc<dyn SessionRepository>> {
        Ok(Arc::new(theway_storage::session::open_repo(cwd).await))
    }

    fn job_transcript_store(&self, cwd: &Path) -> Arc<dyn JobTranscriptStore> {
        DiskTranscriptStore::new(cwd.join(".pi").join("subagent-jobs"))
    }

    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>> {
        Ok(dag_persist::load_session_runs(cwd, session_id).await)
    }

    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<dyn DagPersistSink> {
        DagPersistHandle::spawn(engine, cwd)
    }

    #[allow(private_interfaces)]
    fn spawn_dag_persist_for_sessions(
        &self,
        engine: Arc<DagEngine>,
        cwd: PathBuf,
        sessions: SessionExecutionRegistry,
    ) -> Arc<dyn DagPersistSink> {
        DagPersistHandle::spawn_with_sessions(engine, cwd, sessions)
    }

    async fn load_dynamic_triggers(
        &self,
        cwd: &Path,
        session_id: &str,
    ) -> Result<Vec<DynamicTriggerRule>> {
        let path = local_sidecar_path(cwd, session_id, SidecarKind::Trigger).await?;
        Ok(read_rules_file(&path)?)
    }

    async fn save_dynamic_triggers(
        &self,
        _cwd: &Path,
        session_id: &str,
        rules: &[DynamicTriggerRule],
    ) -> Result<()> {
        let path = local_sidecar_path(_cwd, session_id, SidecarKind::Trigger).await?;
        write_rules_file(&path, rules)?;
        Ok(())
    }

    async fn load_cron_jobs(&self, cwd: &Path, session_id: &str) -> Result<Vec<CronJob>> {
        let path = local_sidecar_path(cwd, session_id, SidecarKind::Cron).await?;
        Ok(read_jobs_file(&path)?)
    }

    async fn save_cron_jobs(&self, _cwd: &Path, session_id: &str, jobs: &[CronJob]) -> Result<()> {
        let path = local_sidecar_path(_cwd, session_id, SidecarKind::Cron).await?;
        write_jobs_file(&path, jobs)?;
        Ok(())
    }
}

/// Controller-backed [`RuntimeStorage`] (issue #85): delegates DAG run
/// persistence to a `StorageService` gRPC server. Session transcript access
/// still uses the local repo (the same controller-side SQLite directory in
/// the TUI's local server layout); the RPC path covers the externalized
/// runtime-state operations defined by `state.proto`.
#[derive(Clone)]
pub struct RemoteRuntimeStorage {
    addr: String,
    client: Arc<Mutex<GrpcClient>>,
}

impl RemoteRuntimeStorage {
    /// Connect to a controller-side `StorageService` server.
    pub async fn connect(addr: &str) -> Result<Self> {
        let client = GrpcClient::connect(addr)
            .await
            .with_context(|| format!("connect storage service {addr}"))?;
        Ok(Self {
            addr: addr.to_string(),
            client: Arc::new(Mutex::new(client)),
        })
    }

    /// Address of the controller storage server.
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

#[async_trait]
impl RuntimeStorage for RemoteRuntimeStorage {
    async fn session_repository(&self, cwd: &Path) -> Result<Arc<dyn SessionRepository>> {
        Ok(Arc::new(theway_storage::session::open_repo(cwd).await))
    }

    fn job_transcript_store(&self, _cwd: &Path) -> Arc<dyn JobTranscriptStore> {
        MemoryTranscriptStore::new()
    }

    async fn load_dag_runs(&self, cwd: &Path, session_id: &str) -> Result<Vec<PersistedRun>> {
        let _ = cwd;
        let mut client = self.client.lock().await;
        let result = client
            .state_load_dag_runs(&WireLoadDagRunsRequest {
                session_id: session_id.to_string(),
                run_id: None,
            })
            .await?;
        result
            .runs
            .iter()
            .map(|stored| {
                serde_json::from_str(&stored.snapshot).with_context(|| {
                    format!(
                        "parse remote DAG snapshot for run {} in session {}",
                        stored.run_id, stored.session_id
                    )
                })
            })
            .collect()
    }

    fn spawn_dag_persist(&self, engine: Arc<DagEngine>, cwd: PathBuf) -> Arc<dyn DagPersistSink> {
        RemoteDagPersistHandle::spawn(engine, cwd, self.clone())
    }

    async fn load_dynamic_triggers(
        &self,
        _cwd: &Path,
        session_id: &str,
    ) -> Result<Vec<DynamicTriggerRule>> {
        let mut client = self.client.lock().await;
        let result = client
            .state_load_trigger_rules(&WireLoadTriggerRulesRequest {
                session_id: session_id.to_string(),
            })
            .await?;
        result.rules.iter().map(trigger_from_wire).collect()
    }

    async fn save_dynamic_triggers(
        &self,
        _cwd: &Path,
        session_id: &str,
        rules: &[DynamicTriggerRule],
    ) -> Result<()> {
        let mut client = self.client.lock().await;
        client
            .state_save_trigger_rules(&WireSaveTriggerRulesRequest {
                session_id: session_id.to_string(),
                rules: rules.iter().map(trigger_to_wire).collect(),
            })
            .await?;
        Ok(())
    }

    async fn load_cron_jobs(&self, _cwd: &Path, session_id: &str) -> Result<Vec<CronJob>> {
        let mut client = self.client.lock().await;
        let result = client
            .state_load_cron_jobs(&WireLoadCronJobsRequest {
                session_id: session_id.to_string(),
            })
            .await?;
        result.jobs.iter().map(cron_from_wire).collect()
    }

    async fn save_cron_jobs(&self, _cwd: &Path, session_id: &str, jobs: &[CronJob]) -> Result<()> {
        let mut client = self.client.lock().await;
        client
            .state_save_cron_jobs(&WireSaveCronJobsRequest {
                session_id: session_id.to_string(),
                jobs: jobs.iter().map(cron_to_wire).collect(),
            })
            .await?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SidecarKind {
    Trigger,
    Cron,
}

async fn local_sidecar_path(cwd: &Path, session_id: &str, kind: SidecarKind) -> Result<PathBuf> {
    let repo = theway_storage::session::open_repo(cwd).await;
    let session_path = theway_storage::session::find_path_by_id(&repo, session_id)
        .await?
        .with_context(|| format!("session {session_id} not found in {}", cwd.display()))?;
    Ok(match kind {
        SidecarKind::Trigger => theway_storage::session::trigger_sidecar_path(&session_path),
        SidecarKind::Cron => theway_storage::session::cron_sidecar_path(&session_path),
    })
}

fn trigger_to_wire(rule: &DynamicTriggerRule) -> WireStoredTriggerRule {
    WireStoredTriggerRule {
        id: rule.id.clone(),
        condition: rule.condition.clone(),
        action: rule.action.clone(),
        enabled: rule.enabled,
        fire_once: rule.fire_once,
        fired_at: rule.fired_at.map(|dt| dt.to_rfc3339()),
        promote_to_chat: rule.promote_to_chat,
        created_at: rule.created_at.to_rfc3339(),
    }
}

fn trigger_from_wire(rule: &WireStoredTriggerRule) -> Result<DynamicTriggerRule> {
    Ok(DynamicTriggerRule {
        id: rule.id.clone(),
        condition: rule.condition.clone(),
        action: rule.action.clone(),
        enabled: rule.enabled,
        fire_once: rule.fire_once,
        fired_at: rule.fired_at.as_deref().map(parse_rfc3339).transpose()?,
        promote_to_chat: rule.promote_to_chat,
        created_at: parse_rfc3339(&rule.created_at)?,
    })
}

fn cron_to_wire(job: &CronJob) -> WireStoredCronJob {
    WireStoredCronJob {
        id: job.id.clone(),
        schedule: job.schedule.clone(),
        action: job.action.clone(),
        enabled: job.enabled,
        running_trace_id: job.running_trace_id.clone(),
        last_due_at: job.last_due_at.map(|dt| dt.to_rfc3339()),
        last_fired_at: job.last_fired_at.map(|dt| dt.to_rfc3339()),
        last_completed_at: job.last_completed_at.map(|dt| dt.to_rfc3339()),
        last_error: job.last_error.clone(),
        skipped_overlap_count: job.skipped_overlap_count,
        stateful: job.stateful,
        created_at: job.created_at.to_rfc3339(),
    }
}

fn cron_from_wire(job: &WireStoredCronJob) -> Result<CronJob> {
    Ok(CronJob {
        id: job.id.clone(),
        schedule: job.schedule.clone(),
        action: job.action.clone(),
        enabled: job.enabled,
        running_trace_id: job.running_trace_id.clone(),
        last_due_at: job.last_due_at.as_deref().map(parse_rfc3339).transpose()?,
        last_fired_at: job
            .last_fired_at
            .as_deref()
            .map(parse_rfc3339)
            .transpose()?,
        last_completed_at: job
            .last_completed_at
            .as_deref()
            .map(parse_rfc3339)
            .transpose()?,
        last_error: job.last_error.clone(),
        skipped_overlap_count: job.skipped_overlap_count,
        stateful: job.stateful,
        created_at: parse_rfc3339(&job.created_at)?,
    })
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("invalid RFC3339 timestamp: {value}"))
}

/// Debounced DAG persistence sink backed by the controller `StorageService`.
///
/// Mirrors [`DagPersistHandle`]'s coalescing loop, but writes each running run
/// as a `WireSaveDagRunRequest` over the remote storage RPC instead of writing
/// a local SQLite store.
pub struct RemoteDagPersistHandle {
    engine: Arc<DagEngine>,
    storage: RemoteRuntimeStorage,
    dirty: Arc<Notify>,
    task: ParkingMutex<Option<JoinHandle<()>>>,
}

impl RemoteDagPersistHandle {
    /// Create the handle, wire it into the engine, and start the debounce
    /// task. Keep the returned `Arc` alive for the process lifetime.
    pub fn spawn(engine: Arc<DagEngine>, cwd: PathBuf, storage: RemoteRuntimeStorage) -> Arc<Self> {
        let _ = cwd;
        let dirty = Arc::new(Notify::new());
        let handle = Arc::new(Self {
            engine,
            storage,
            dirty: dirty.clone(),
            task: ParkingMutex::new(None),
        });
        let task = tokio::spawn(handle.clone().run_loop());
        *handle.task.lock() = Some(task);
        handle.engine.set_persist_sink(Some(handle.clone()));
        handle
    }

    async fn run_loop(self: Arc<Self>) {
        loop {
            self.dirty.notified().await;
            // Coalesce within the same 500 ms debounce window as the local sink.
            loop {
                tokio::select! {
                    _ = self.dirty.notified() => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(500)) => break,
                }
            }
            if let Err(e) = self.save_all().await {
                tracing::warn!("remote dag persist: {e}");
            }
        }
    }

    async fn save_all(&self) -> Result<()> {
        let runs = self.engine.list_runs();
        for run in runs
            .into_iter()
            .filter(|run| run.status == DagStatus::Running)
        {
            let persisted = to_persisted(&run);
            let snapshot = serde_json::to_string(&persisted)?;
            let mut client = self.storage.client.lock().await;
            client
                .state_save_dag_run(&WireSaveDagRunRequest {
                    session_id: run.session_id.clone().unwrap_or_default(),
                    run_id: run.id.clone(),
                    snapshot,
                })
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl DagPersistSink for RemoteDagPersistHandle {
    fn notify_dirty(&self) {
        self.dirty.notify_one();
    }

    async fn flush(&self) {
        if let Err(e) = self.save_all().await {
            tracing::warn!("remote dag persist flush: {e}");
        }
    }
}

/// Convenience constructor for the composition root.
pub fn local_runtime_storage() -> Arc<dyn RuntimeStorage> {
    Arc::new(LocalRuntimeStorage)
}

/// Convenience constructor for controller-backed storage (issue #85).
pub async fn remote_runtime_storage(addr: &str) -> Result<Arc<dyn RuntimeStorage>> {
    Ok(Arc::new(RemoteRuntimeStorage::connect(addr).await?))
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("runtime_storage");

#[cfg(test)]
mod runtime_storage_extra_tests {
    //! Additional mirrored coverage lives in `tests/runtime_storage/extra/`.
    tests_bridge_macro::tests_bridge!("runtime_storage/extra");
}

#[cfg(test)]
mod runtime_storage_linecov_tests {
    //! Line-coverage completion tests live in `tests/runtime_storage/linecov/`.
    tests_bridge_macro::tests_bridge!("runtime_storage/linecov");
}
