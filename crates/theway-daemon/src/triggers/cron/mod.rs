//! Local crontab-style scheduler.
//!
//! Cron jobs are a time-based source parallel to event triggers: the hook emits a normal
//! runtime [`Trigger`](theway_core::Trigger) envelope, then the cron action hook maps
//! that accepted trigger into an `InjectAndRun` parent turn. Storage intentionally contains
//! only schedule/action text and never provider credentials.
//!
//! Split by domain: [`tools`] (model-facing CRUD tools), [`hook`] (notification hook,
//! action hook, trigger listener, loop-state helpers), [`errors`] (error types, schedule
//! parser, audit/render helpers).

mod errors;
mod hook;
mod tools;

#[allow(unused_imports)]
pub use errors::{AddCronJobError, CronScheduleError, CronStorageError, cron_control_plane_audit};
#[allow(unused_imports)]
pub use hook::{
    CronNotificationHook, cron_action_hook, cron_trigger_listener, strip_loop_protocol_tags,
};
#[allow(unused_imports)]
pub use tools::{ListCronJobsTool, NewCronJobTool, RemoveCronJobTool, SetCronJobStateTool};

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use theway_core::AgentHarness;
use uuid::Uuid;

use errors::CronExpression;

// The `CronJob` data model lives in the pure leaf contract crate
// (`theway_contract::triggers`, re-exported by `theway_transport::triggers`)
// so the session-archive surface shares one type identity; this module
// re-exports it for `crate::triggers::cron::CronJob` paths and adds the
// daemon-side schedule helpers.
pub use theway_contract::triggers::CronJob;

/// Daemon-side schedule helpers on the shared `CronJob` data model (`theway-contract`).
pub trait CronJobExt {
    fn next_run_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>>;
}

impl CronJobExt for CronJob {
    fn next_run_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        CronExpression::parse(&self.schedule)
            .ok()?
            .next_after(after)
    }
}

// The bridged test mirror (`tests/triggers/cron/mod.rs`) pulls everything it needs via
// `use super::*`; names it uses but this module's own code does not are re-imported here
// for test builds only.
#[cfg(test)]
use crate::trigger_engine::event::TriggerEvent;
#[cfg(test)]
use crate::trigger_engine::execution::{
    BeforeTriggerActionContext, BeforeTriggerActionHook, TriggerAction, TriggerDelivery,
};
#[cfg(test)]
use chrono::{Local, Timelike};
#[cfg(test)]
use hook::{
    compose_stateful_prompt, cron_trigger_for_job, extract_tag_all, extract_tag_block,
    loop_state_path, read_loop_state, write_loop_state,
};
#[cfg(test)]
use tokio_util::sync::CancellationToken;

const MAX_ACTION_PREVIEW_CHARS: usize = 120;
const MAX_ACTION_BYTES: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct CronRegistry {
    inner: Arc<Mutex<CronRegistryState>>,
}

#[derive(Clone, Default)]
enum CronPersistence {
    #[default]
    None,
    Path(PathBuf),
    Runtime {
        storage: Arc<dyn theway_daemon::runtime_storage::RuntimeStorage>,
        cwd: PathBuf,
        session_id: String,
    },
}

impl std::fmt::Debug for CronPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Path(path) => f.debug_tuple("Path").field(path).finish(),
            Self::Runtime {
                cwd, session_id, ..
            } => f
                .debug_struct("Runtime")
                .field("cwd", cwd)
                .field("session_id", session_id)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct CronRegistryState {
    jobs: Vec<CronJob>,
    storage: CronPersistence,
}

impl CronRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_from_path(&self, path: impl Into<PathBuf>) -> Result<(), CronStorageError> {
        let path = path.into();
        let mut jobs = read_jobs_file(&path)?;
        for job in &jobs {
            CronExpression::parse(&job.schedule)?;
        }
        let cleared_stale_running = clear_stale_running_state(&mut jobs);
        if cleared_stale_running {
            write_jobs_file(&path, &jobs)?;
        }
        let mut state = self.inner.lock();
        state.jobs = jobs;
        state.storage = CronPersistence::Path(path);
        Ok(())
    }

    /// Load jobs from the runtime storage seam (issue #86). Mutations are
    /// persisted back through the same seam; remote storage saves are
    /// fire-and-forget RPC writes.
    pub async fn load_from_storage(
        &self,
        storage: Arc<dyn theway_daemon::runtime_storage::RuntimeStorage>,
        cwd: PathBuf,
        session_id: String,
    ) -> Result<(), CronStorageError> {
        let mut jobs = storage
            .load_cron_jobs(&cwd, &session_id)
            .await
            .map_err(|e| CronStorageError::Io(e.to_string()))?;
        for job in &jobs {
            CronExpression::parse(&job.schedule)?;
        }
        if clear_stale_running_state(&mut jobs) {
            storage
                .save_cron_jobs(&cwd, &session_id, &jobs)
                .await
                .map_err(|e| CronStorageError::Io(e.to_string()))?;
        }
        let mut state = self.inner.lock();
        state.jobs = jobs;
        state.storage = CronPersistence::Runtime {
            storage,
            cwd,
            session_id,
        };
        Ok(())
    }

    pub fn storage_path(&self) -> Option<PathBuf> {
        match &self.inner.lock().storage {
            CronPersistence::Path(path) => Some(path.clone()),
            _ => None,
        }
    }

    pub fn list(&self) -> Vec<CronJob> {
        self.inner.lock().jobs.clone()
    }

    /// Convenience wrapper (non-stateful). Production paths pass `stateful` explicitly
    /// via [`Self::add_job_full`]; unit tests use this shorthand.
    #[allow(dead_code)]
    pub fn add_job(&self, schedule: &str, action: &str) -> Result<CronJob, AddCronJobError> {
        self.add_job_full(schedule, action, false)
    }

    pub fn add_job_full(
        &self,
        schedule: &str,
        action: &str,
        stateful: bool,
    ) -> Result<CronJob, AddCronJobError> {
        let schedule = schedule.trim();
        let action = action.trim();
        if action.is_empty() {
            return Err(AddCronJobError::EmptyAction);
        }
        if action.len() > MAX_ACTION_BYTES {
            return Err(AddCronJobError::ActionTooLarge {
                max_bytes: MAX_ACTION_BYTES,
            });
        }
        CronExpression::parse(schedule)?;
        let job = CronJob {
            id: format!("cron-{}", Uuid::new_v4().simple()),
            schedule: schedule.to_string(),
            action: action.to_string(),
            enabled: true,
            running_trace_id: None,
            last_due_at: None,
            last_fired_at: None,
            last_completed_at: None,
            last_error: None,
            skipped_overlap_count: 0,
            stateful,
            created_at: Utc::now(),
        };
        self.insert_job(job)
    }

    fn insert_job(&self, job: CronJob) -> Result<CronJob, AddCronJobError> {
        let mut state = self.inner.lock();
        let mut next = state.jobs.clone();
        next.push(job.clone());
        persist_jobs(&state.storage, &next)?;
        state.jobs = next;
        Ok(job)
    }

    pub fn remove_job(&self, id: &str) -> Result<Option<CronJob>, CronStorageError> {
        let id = id.trim();
        let mut state = self.inner.lock();
        let Some(pos) = state.jobs.iter().position(|job| job.id == id) else {
            return Ok(None);
        };
        let mut next = state.jobs.clone();
        let removed = next.remove(pos);
        persist_jobs(&state.storage, &next)?;
        state.jobs = next;
        Ok(Some(removed))
    }

    pub fn set_job_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<CronJob>, CronStorageError> {
        let id = id.trim();
        let mut state = self.inner.lock();
        let Some(pos) = state.jobs.iter().position(|job| job.id == id) else {
            return Ok(None);
        };
        let mut next = state.jobs.clone();
        next[pos].enabled = enabled;
        if !enabled {
            next[pos].running_trace_id = None;
        }
        let updated = next[pos].clone();
        persist_jobs(&state.storage, &next)?;
        state.jobs = next;
        Ok(Some(updated))
    }

    fn due_jobs(&self, since: DateTime<Utc>, now: DateTime<Utc>) -> Vec<(CronJob, DateTime<Utc>)> {
        let mut state = self.inner.lock();
        let mut next = state.jobs.clone();
        let mut due = Vec::new();
        for job in &mut next {
            if !job.enabled {
                continue;
            }
            let Ok(expr) = CronExpression::parse(&job.schedule) else {
                job.last_error = Some("invalid schedule".into());
                continue;
            };
            let Some(due_at) = expr.next_after(since) else {
                job.last_error = Some("no next run within 5 years".into());
                continue;
            };
            if due_at > now {
                continue;
            }
            if job.running_trace_id.is_some() {
                job.skipped_overlap_count = job.skipped_overlap_count.saturating_add(1);
                job.last_due_at = Some(due_at);
                job.last_error = Some("skipped: previous run still active".into());
                continue;
            }
            let trace_id = format!("cron-{}", Uuid::new_v4().simple());
            job.running_trace_id = Some(trace_id.clone());
            job.last_due_at = Some(due_at);
            job.last_fired_at = Some(now);
            job.last_error = None;
            due.push((job.clone(), due_at));
        }
        // Ticks run every TICK_SECS for every session; only persist real state changes so
        // idle sessions don't accrete empty/rewritten sidecar files.
        if next != state.jobs {
            let _ = persist_jobs(&state.storage, &next);
            state.jobs = next;
        }
        due
    }

    /// Job currently running under `trace_id`, if any. Must be called before
    /// `mark_completed`, which clears the trace binding.
    pub fn job_for_trace(&self, trace_id: &str) -> Option<CronJob> {
        self.inner
            .lock()
            .jobs
            .iter()
            .find(|job| job.running_trace_id.as_deref() == Some(trace_id))
            .cloned()
    }

    pub fn mark_completed(&self, trace_id: &str, error: Option<String>) {
        let mut state = self.inner.lock();
        let Some(pos) = state
            .jobs
            .iter()
            .position(|job| job.running_trace_id.as_deref() == Some(trace_id))
        else {
            return;
        };
        let mut next = state.jobs.clone();
        next[pos].running_trace_id = None;
        next[pos].last_completed_at = Some(Utc::now());
        next[pos].last_error = error;
        let _ = persist_jobs(&state.storage, &next);
        state.jobs = next;
    }

    #[allow(dead_code)]
    pub fn clear_for_tests(&self) {
        *self.inner.lock() = CronRegistryState::default();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CronJobsFile {
    #[serde(default)]
    jobs: Vec<CronJob>,
}

fn persist_jobs(storage: &CronPersistence, jobs: &[CronJob]) -> Result<(), CronStorageError> {
    match storage {
        CronPersistence::Path(path) => write_jobs_file(path, jobs),
        CronPersistence::Runtime {
            storage,
            cwd,
            session_id,
        } => {
            let storage = storage.clone();
            let cwd = cwd.clone();
            let session_id = session_id.clone();
            let jobs = jobs.to_vec();
            tokio::spawn(async move {
                if let Err(e) = storage.save_cron_jobs(&cwd, &session_id, &jobs).await {
                    tracing::warn!(error = %e, "cron remote persist failed");
                }
            });
            Ok(())
        }
        CronPersistence::None => Ok(()),
    }
}

pub(crate) fn read_jobs_file(path: &Path) -> Result<Vec<CronJob>, CronStorageError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let file: CronJobsFile =
                toml::from_str(&text).map_err(|err| CronStorageError::Parse(err.to_string()))?;
            Ok(file.jobs)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(CronStorageError::Io(err.to_string())),
    }
}

pub(crate) fn write_jobs_file(path: &Path, jobs: &[CronJob]) -> Result<(), CronStorageError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| CronStorageError::Io(err.to_string()))?;
    }
    let file = CronJobsFile {
        jobs: jobs.to_vec(),
    };
    let text = toml::to_string_pretty(&file)
        .map_err(|err| CronStorageError::Serialize(err.to_string()))?;
    std::fs::write(path, text).map_err(|err| CronStorageError::Io(err.to_string()))
}

fn clear_stale_running_state(jobs: &mut [CronJob]) -> bool {
    let mut changed = false;
    for job in jobs {
        if job.running_trace_id.is_some() {
            job.running_trace_id = None;
            job.last_error = Some("cleared stale running state on startup".into());
            changed = true;
        }
    }
    changed
}

pub fn global_cron_registry() -> &'static CronRegistry {
    static CELL: once_cell::sync::OnceCell<CronRegistry> = once_cell::sync::OnceCell::new();
    CELL.get_or_init(CronRegistry::new)
}

type HarnessCell = Arc<OnceCell<Arc<AgentHarness>>>;

#[cfg(test)]
// Test files live in `tests/triggers/cron/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("triggers/cron");
