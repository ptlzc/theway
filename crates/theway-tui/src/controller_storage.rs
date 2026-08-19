//! Controller-side storage handlers for the `StorageService` gRPC server
//! (issue #85). The TUI owns the local SQLite session repo and the session
//! sidecar files; this module exposes that local storage through the
//! transport `SessionOps` / `StorageOps` seams so a daemon can use the same
//! controller storage over RPC.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use theway_contract::session::{SessionError, SessionReader};
use theway_storage::session;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::transport::{SessionOps, StorageOps};
use theway_transport::triggers::{CronJob, DynamicTriggerRule};
use theway_transport::wire::{
    SessionSummary, WireLoadCronJobsRequest, WireLoadCronJobsResult, WireLoadDagRunsRequest,
    WireLoadDagRunsResult, WireLoadTriggerRulesRequest, WireLoadTriggerRulesResult,
    WireSaveCronJobsRequest, WireSaveCronJobsResult, WireSaveDagRunRequest, WireSaveDagRunResult,
    WireSaveTriggerRulesRequest, WireSaveTriggerRulesResult, WireStoredCronJob, WireStoredDagRun,
    WireStoredTriggerRule,
};

/// Local `SessionOps` used by the controller-side `StorageService` server.
pub(crate) struct ControllerSessionOps {
    repo: Arc<SqliteSessionRepo>,
    cwd: PathBuf,
}

impl ControllerSessionOps {
    pub(crate) fn new(repo: Arc<SqliteSessionRepo>, cwd: PathBuf) -> Self {
        Self { repo, cwd }
    }
}

#[async_trait]
impl SessionOps for ControllerSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        let mut summaries = Vec::new();
        for path in self.repo.list().await.map_err(repo_err)? {
            let session = self.repo.open(&path).await.map_err(repo_err)?;
            let meta = session.get_metadata_json().await.map_err(repo_err)?;
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
            let name = session::session_name(&session).await.unwrap_or_default();
            let preview = session::first_user_text(&session).await;
            let model = session::last_model_change(&session).await;
            let last_activity_at = tokio::fs::metadata(&path)
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            summaries.push(SessionSummary {
                session_id,
                name,
                cwd,
                model,
                created_at,
                last_activity_at,
                graph_count: 0,
                active_graph_count: 0,
                busy: false,
                preview,
            });
        }
        Ok(summaries)
    }

    async fn create(&self) -> Result<String> {
        let session = self
            .repo
            .create(self.cwd.to_string_lossy())
            .await
            .map_err(repo_err)?;
        let meta = session.get_metadata_json().await.map_err(repo_err)?;
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
        let path = session::find_path_by_id(&self.repo, id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        let session = self.repo.open(&path).await.map_err(repo_err)?;
        session::append_session_name(&session, name)
            .await
            .map_err(repo_err)?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        session::delete_by_id(&self.repo, id).await?;
        Ok(Vec::new())
    }
}

/// Local `StorageOps` used by the controller-side `StorageService` server.
pub(crate) struct ControllerStorageOps {
    repo: Arc<SqliteSessionRepo>,
}

impl ControllerStorageOps {
    pub(crate) fn new(repo: Arc<SqliteSessionRepo>) -> Self {
        Self { repo }
    }

    fn dag_state_path(&self, session_id: &str) -> PathBuf {
        self.repo
            .root()
            .join(format!("controller-dag-{}.json", sanitize(session_id)))
    }

    async fn load_dag_state(&self, session_id: &str) -> Result<Vec<WireStoredDagRun>> {
        let path = self.dag_state_path(session_id);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    async fn write_dag_state(&self, session_id: &str, runs: &[WireStoredDagRun]) -> Result<()> {
        let path = self.dag_state_path(session_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let text = serde_json::to_string_pretty(runs)?;
        tokio::fs::write(&path, text)
            .await
            .with_context(|| format!("write {}", path.display()))
    }

    async fn sidecar_path(&self, session_id: &str, kind: SidecarKind) -> Result<PathBuf> {
        let path = session::find_path_by_id(&self.repo, session_id)
            .await?
            .with_context(|| format!("no session matches id {session_id}"))?;
        match kind {
            SidecarKind::Trigger => Ok(session::trigger_sidecar_path(&path)),
            SidecarKind::Cron => Ok(session::cron_sidecar_path(&path)),
        }
    }
}

#[derive(Clone, Copy)]
enum SidecarKind {
    Trigger,
    Cron,
}

#[async_trait]
impl StorageOps for ControllerStorageOps {
    async fn save_dag_run(&self, request: &WireSaveDagRunRequest) -> Result<WireSaveDagRunResult> {
        let mut runs = self.load_dag_state(&request.session_id).await?;
        if let Some(existing) = runs.iter_mut().find(|run| run.run_id == request.run_id) {
            existing.snapshot = request.snapshot.clone();
        } else {
            runs.push(WireStoredDagRun {
                session_id: request.session_id.clone(),
                run_id: request.run_id.clone(),
                snapshot: request.snapshot.clone(),
            });
        }
        self.write_dag_state(&request.session_id, &runs).await?;
        Ok(WireSaveDagRunResult { saved: true })
    }

    async fn load_dag_runs(
        &self,
        request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        let runs = self.load_dag_state(&request.session_id).await?;
        let runs = match request.run_id.as_deref() {
            Some(run_id) => runs
                .into_iter()
                .filter(|run| run.run_id == run_id)
                .collect(),
            None => runs,
        };
        Ok(WireLoadDagRunsResult { runs })
    }

    async fn save_trigger_rules(
        &self,
        request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        let path = self
            .sidecar_path(&request.session_id, SidecarKind::Trigger)
            .await?;
        let rules: Vec<DynamicTriggerRule> = request
            .rules
            .iter()
            .map(trigger_from_wire)
            .collect::<Result<_>>()?;
        write_trigger_rules(&path, &rules).await?;
        Ok(WireSaveTriggerRulesResult {
            count: rules.len() as u32,
        })
    }

    async fn load_trigger_rules(
        &self,
        request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        let path = self
            .sidecar_path(&request.session_id, SidecarKind::Trigger)
            .await?;
        let rules = read_trigger_rules(&path).await?;
        Ok(WireLoadTriggerRulesResult {
            rules: rules.iter().map(trigger_to_wire).collect(),
        })
    }

    async fn save_cron_jobs(
        &self,
        request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        let path = self
            .sidecar_path(&request.session_id, SidecarKind::Cron)
            .await?;
        let jobs: Vec<CronJob> = request
            .jobs
            .iter()
            .map(cron_from_wire)
            .collect::<Result<_>>()?;
        write_cron_jobs(&path, &jobs).await?;
        Ok(WireSaveCronJobsResult {
            count: jobs.len() as u32,
        })
    }

    async fn load_cron_jobs(
        &self,
        request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        let path = self
            .sidecar_path(&request.session_id, SidecarKind::Cron)
            .await?;
        let jobs = read_cron_jobs(&path).await?;
        Ok(WireLoadCronJobsResult {
            jobs: jobs.iter().map(cron_to_wire).collect(),
        })
    }
}

fn sanitize(session_id: &str) -> String {
    let clean: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if clean.is_empty() {
        "default".to_string()
    } else {
        clean
    }
}

fn repo_err(e: SessionError) -> anyhow::Error {
    anyhow::Error::msg(e.to_string())
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DynamicTriggerFile {
    version: u32,
    rules: Vec<DynamicTriggerRule>,
}

const DYNAMIC_TRIGGER_FILE_VERSION: u32 = 1;

async fn read_trigger_rules(path: &Path) -> Result<Vec<DynamicTriggerRule>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) if text.trim().is_empty() => Ok(Vec::new()),
        Ok(text) => {
            let file: DynamicTriggerFile =
                serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
            Ok(file.rules)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

async fn write_trigger_rules(path: &Path, rules: &[DynamicTriggerRule]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = DynamicTriggerFile {
        version: DYNAMIC_TRIGGER_FILE_VERSION,
        rules: rules.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file)?;
    tokio::fs::write(path, text)
        .await
        .with_context(|| format!("write {}", path.display()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CronJobsFile {
    #[serde(default)]
    jobs: Vec<CronJob>,
}

async fn read_cron_jobs(path: &Path) -> Result<Vec<CronJob>> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => {
            let file: CronJobsFile =
                toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
            Ok(file.jobs)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

async fn write_cron_jobs(path: &Path, jobs: &[CronJob]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let file = CronJobsFile {
        jobs: jobs.to_vec(),
    };
    let text = toml::to_string_pretty(&file)?;
    tokio::fs::write(path, text)
        .await
        .with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("controller_storage");
