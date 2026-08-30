//! Test fakes for the transport servers (session-resource-model N4).
//!
//! [`FakeSessionOps`] is an in-memory [`crate::transport::SessionOps`] so the gRPC/HTTP
//! session tests exercise the transport surface without a real session repo on disk.
//! Delete protection is simulated by mapping a session id to its "running" run ids.
//!
//! [`FakeToolOps`] is the in-memory [`crate::transport::ToolOps`] twin (issue #75):
//! files / dirs / memory entries live in maps, `exec_command` replays a
//! configurable frame script, and grep/find run over the stored files — enough
//! behavior to round-trip the tool-operation RPC surfaces without a real FS.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::transport::{StorageOps, ToolExecStream, ToolOps};
use crate::wire::{
    SessionSummary, ToolError, WireCronSnapshot, WireLoadCronJobsRequest, WireLoadCronJobsResult,
    WireLoadDagRunsRequest, WireLoadDagRunsResult, WireLoadTriggerRulesRequest,
    WireLoadTriggerRulesResult, WireMcpSnapshot, WireSaveCronJobsRequest, WireSaveCronJobsResult,
    WireSaveDagRunRequest, WireSaveDagRunResult, WireSaveTriggerRulesRequest,
    WireSaveTriggerRulesResult, WireSidebarSnapshot, WireSkillsSnapshot, WireStoredCronJob,
    WireStoredDagRun, WireStoredTriggerRule, WireToolDirEntry, WireToolEditRequest,
    WireToolEditResult, WireToolExecFrame, WireToolExecRequest, WireToolFindRequest,
    WireToolFindResult, WireToolGrepFileCount, WireToolGrepMatch, WireToolGrepRequest,
    WireToolGrepResult, WireToolListDirRequest, WireToolListDirResult, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolSkillSource, WireToolWriteRequest, WireToolWriteResult,
    WireToolsSnapshot, WireTriggersSnapshot,
};
use anyhow::Result;
use async_trait::async_trait;

/// In-memory `SessionOps`: sessions live in a `Vec` (oldest → newest, like the repo-backed
/// impl), ids for `create` come from a counter.
#[derive(Default)]
pub struct FakeSessionOps {
    inner: Mutex<FakeInner>,
}

#[derive(Default)]
struct FakeInner {
    sessions: Vec<SessionSummary>,
    counter: u64,
    /// session_id → running run ids; non-empty refuses `delete`.
    running: HashMap<String, Vec<String>>,
}

impl FakeSessionOps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed an existing session; returns its id.
    pub fn add_session(&self, id: &str) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.push(summary(id));
        id.to_string()
    }

    /// Mark a session as having running graphs (blocks `delete`, ids reported back).
    pub fn set_running(&self, session_id: &str, run_ids: &[&str]) {
        let mut inner = self.inner.lock().unwrap();
        inner.running.insert(
            session_id.to_string(),
            run_ids.iter().map(|s| s.to_string()).collect(),
        );
    }
}

fn summary(id: &str) -> SessionSummary {
    SessionSummary {
        session_id: id.to_string(),
        name: String::new(),
        cwd: "/tmp/theway".to_string(),
        model: "provider:model".to_string(),
        created_at: String::new(),
        last_activity_at: 0,
        last_activity_at_rfc3339: None,
        graph_count: 0,
        active_graph_count: 0,
        busy: false,
        preview: None,
        tree_prefix: String::new(),
        metadata: HashMap::new(),
    }
}

#[async_trait]
impl crate::transport::SessionOps for FakeSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        Ok(self.inner.lock().unwrap().sessions.clone())
    }

    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String> {
        let mut inner = self.inner.lock().unwrap();
        let id = match session_id.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => {
                if inner.sessions.iter().any(|s| s.session_id == id) {
                    anyhow::bail!("session id already exists: {id}");
                }
                id.to_string()
            }
            None => {
                inner.counter += 1;
                format!("sess-new-{}", inner.counter)
            }
        };
        let mut summary = summary(&id);
        summary.metadata = metadata.clone();
        inner.sessions.push(summary);
        Ok(id)
    }

    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.sessions.iter_mut().find(|s| s.session_id == id) else {
            anyhow::bail!("no session matches id {id}");
        };
        session.metadata = metadata.clone();
        Ok(())
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("session name must not be empty");
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.sessions.iter_mut().find(|s| s.session_id == id) else {
            anyhow::bail!("no session matches id {id}");
        };
        session.name = name.to_string();
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        let mut inner = self.inner.lock().unwrap();
        let Some(pos) = inner.sessions.iter().position(|s| s.session_id == id) else {
            anyhow::bail!("no session matches id {id}");
        };
        if let Some(runs) = inner.running.get(id)
            && !runs.is_empty()
        {
            return Ok(runs.clone());
        }
        inner.sessions.remove(pos);
        inner.running.remove(id);
        Ok(Vec::new())
    }
}

// ── FakeStorageOps (issue #84) ───────────────────────────────────────

/// In-memory `StorageOps` for the transport tests. DAG run snapshots,
/// trigger rules and cron jobs live in maps keyed by session id; the behavior
/// is enough to round-trip the `StorageService` RPC and JSON-RPC state methods
/// without a real storage backend.
#[derive(Default)]
pub struct FakeStorageOps {
    inner: Mutex<FakeStorageInner>,
}

#[derive(Default)]
struct FakeStorageInner {
    dag_runs: HashMap<(String, String), String>,
    trigger_rules: HashMap<String, Vec<WireStoredTriggerRule>>,
    cron_jobs: HashMap<String, Vec<WireStoredCronJob>>,
}

impl FakeStorageOps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a stored DAG run snapshot for a session.
    pub fn put_dag_run(&self, session_id: &str, run_id: &str, snapshot: &str) {
        self.inner.lock().unwrap().dag_runs.insert(
            (session_id.to_string(), run_id.to_string()),
            snapshot.to_string(),
        );
    }

    /// Seed stored trigger rules for a session.
    pub fn put_trigger_rules(&self, session_id: &str, rules: Vec<WireStoredTriggerRule>) {
        self.inner
            .lock()
            .unwrap()
            .trigger_rules
            .insert(session_id.to_string(), rules);
    }

    /// Seed stored cron jobs for a session.
    pub fn put_cron_jobs(&self, session_id: &str, jobs: Vec<WireStoredCronJob>) {
        self.inner
            .lock()
            .unwrap()
            .cron_jobs
            .insert(session_id.to_string(), jobs);
    }
}

#[async_trait]
impl StorageOps for FakeStorageOps {
    async fn save_dag_run(&self, request: &WireSaveDagRunRequest) -> Result<WireSaveDagRunResult> {
        self.inner.lock().unwrap().dag_runs.insert(
            (request.session_id.clone(), request.run_id.clone()),
            request.snapshot.clone(),
        );
        Ok(WireSaveDagRunResult { saved: true })
    }

    async fn load_dag_runs(
        &self,
        request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        let inner = self.inner.lock().unwrap();
        let runs = match request.run_id.as_deref() {
            Some(run_id) => inner
                .dag_runs
                .get(&(request.session_id.clone(), run_id.to_string()))
                .map(|snapshot| WireStoredDagRun {
                    session_id: request.session_id.clone(),
                    run_id: run_id.to_string(),
                    snapshot: snapshot.clone(),
                })
                .into_iter()
                .collect(),
            None => inner
                .dag_runs
                .iter()
                .filter(|((session_id, _), _)| *session_id == request.session_id)
                .map(|((session_id, run_id), snapshot)| WireStoredDagRun {
                    session_id: session_id.clone(),
                    run_id: run_id.clone(),
                    snapshot: snapshot.clone(),
                })
                .collect(),
        };
        Ok(WireLoadDagRunsResult { runs })
    }

    async fn save_trigger_rules(
        &self,
        request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        let count = request.rules.len() as u32;
        self.inner
            .lock()
            .unwrap()
            .trigger_rules
            .insert(request.session_id.clone(), request.rules.clone());
        Ok(WireSaveTriggerRulesResult { count })
    }

    async fn load_trigger_rules(
        &self,
        request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        let rules = self
            .inner
            .lock()
            .unwrap()
            .trigger_rules
            .get(&request.session_id)
            .cloned()
            .unwrap_or_default();
        Ok(WireLoadTriggerRulesResult { rules })
    }

    async fn save_cron_jobs(
        &self,
        request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        let count = request.jobs.len() as u32;
        self.inner
            .lock()
            .unwrap()
            .cron_jobs
            .insert(request.session_id.clone(), request.jobs.clone());
        Ok(WireSaveCronJobsResult { count })
    }

    async fn load_cron_jobs(
        &self,
        request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        let jobs = self
            .inner
            .lock()
            .unwrap()
            .cron_jobs
            .get(&request.session_id)
            .cloned()
            .unwrap_or_default();
        Ok(WireLoadCronJobsResult { jobs })
    }
}

/// Minimal sidebar used by snapshot fixtures (transport tests + client tests).
pub fn empty_sidebar_snapshot() -> WireSidebarSnapshot {
    WireSidebarSnapshot {
        inbox_new: crate::inbox::new_count(&crate::inbox::default_inbox_path()),
        skills: WireSkillsSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            builtin: 0,
            user: 0,
            project: 0,
            items: Vec::new(),
        },
        triggers: WireTriggersSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            rules: Vec::new(),
        },
        cron: WireCronSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            jobs: Vec::new(),
        },
        mcp: WireMcpSnapshot {
            servers: 0,
            tools: 0,
            notification_hooks: 0,
            server_names: Vec::new(),
            tool_names: Vec::new(),
        },
        tools: WireToolsSnapshot {
            total: 0,
            names: Vec::new(),
        },
        hooks: Vec::new(),
        runtime: Vec::new(),
        commands: Vec::new(),
        runtime_revision: 0,
    }
}

/// Test `CommandOps` wired directly to an event-loop command channel (the
/// same path the real daemon composition uses).
#[derive(Clone)]
pub struct ChannelCommandOps {
    commands: tokio::sync::mpsc::UnboundedSender<crate::wire::WireCommand>,
}

impl ChannelCommandOps {
    pub fn new(commands: tokio::sync::mpsc::UnboundedSender<crate::wire::WireCommand>) -> Self {
        Self { commands }
    }
}

#[async_trait]
impl crate::CommandOps for ChannelCommandOps {
    async fn submit(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<crate::wire::WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        Ok(self
            .commands
            .send(crate::wire::WireCommand::Submit {
                session_id: session_id.to_string(),
                text: text.to_string(),
                images,
                interrupt,
            })
            .is_ok())
    }

    async fn trigger_now(&self, id: &str) -> Result<bool> {
        Ok(self
            .commands
            .send(crate::wire::WireCommand::TriggerRuleNow { id: id.to_string() })
            .is_ok())
    }

    async fn abort(&self, session_id: &str) -> Result<bool> {
        Ok(self
            .commands
            .send(crate::wire::WireCommand::Abort {
                session_id: session_id.to_string(),
            })
            .is_ok())
    }

    async fn resolve_control_plane(&self, session_id: &str, approve: bool) -> Result<bool> {
        Ok(self
            .commands
            .send(crate::wire::WireCommand::ResolveControlPlane {
                session_id: session_id.to_string(),
                approve,
            })
            .is_ok())
    }

    async fn set_model(&self, session_id: &str, spec: &str) -> Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .commands
            .send(crate::wire::WireCommand::SetModel {
                session_id: session_id.to_string(),
                spec: spec.to_string(),
                response: tx,
            })
            .is_err()
        {
            return Ok(false);
        }
        Ok(rx.await.unwrap_or(false))
    }

    async fn set_thinking(&self, session_id: &str, level: &str) -> Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .commands
            .send(crate::wire::WireCommand::SetThinking {
                session_id: session_id.to_string(),
                level: level.to_string(),
                response: tx,
            })
            .is_err()
        {
            return Ok(false);
        }
        Ok(rx.await.unwrap_or(false))
    }
}

/// Test `SettingsOps` backed by the same shared path/config views the real
/// daemon composition uses.
#[derive(Clone)]
pub struct SharedSettingsOps {
    path_context: std::sync::Arc<std::sync::RwLock<crate::wire::WirePathContext>>,
    daemon_config: std::sync::Arc<std::sync::RwLock<crate::wire::WireDaemonConfig>>,
    commands: tokio::sync::mpsc::UnboundedSender<crate::wire::WireCommand>,
}

impl SharedSettingsOps {
    pub fn new(
        path_context: std::sync::Arc<std::sync::RwLock<crate::wire::WirePathContext>>,
        daemon_config: std::sync::Arc<std::sync::RwLock<crate::wire::WireDaemonConfig>>,
        commands: tokio::sync::mpsc::UnboundedSender<crate::wire::WireCommand>,
    ) -> Self {
        Self {
            path_context,
            daemon_config,
            commands,
        }
    }
}

#[async_trait]
impl crate::SettingsOps for SharedSettingsOps {
    async fn get_config(&self) -> Result<crate::wire::WireDaemonConfig> {
        Ok(self.daemon_config.read().unwrap().clone())
    }

    async fn set_config(&self, config: &crate::wire::WireDaemonConfig) -> Result<bool> {
        Ok(self
            .commands
            .send(crate::wire::WireCommand::Configure {
                config: config.clone(),
            })
            .is_ok())
    }

    async fn configure(&self, config: &crate::wire::WireDaemonConfig) -> Result<bool> {
        self.set_config(config).await
    }

    async fn get_path_context(&self) -> Result<crate::wire::WirePathContext> {
        Ok(self.path_context.read().unwrap().clone())
    }

    async fn set_skill_dirs(&self, dirs: &[String]) -> Result<bool> {
        self.path_context.write().unwrap().skills_dirs = dirs.to_vec();
        Ok(self
            .commands
            .send(crate::wire::WireCommand::SetSkillDirs {
                dirs: dirs.to_vec(),
            })
            .is_ok())
    }
}

/// Test `SessionObservabilityOps` that mirrors the old legacy fallback:
/// resource snapshot first, then the live per-session / latest projection.
#[derive(Clone)]
pub struct LiveSessionObservability {
    session_ops: std::sync::Arc<dyn crate::transport::SessionOps>,
    session_states: std::sync::Arc<
        parking_lot::Mutex<std::collections::HashMap<String, crate::wire::WireStatus>>,
    >,
    latest: std::sync::Arc<parking_lot::Mutex<crate::wire::WireStatus>>,
    current_session_id: String,
}

impl LiveSessionObservability {
    pub fn new(
        session_ops: std::sync::Arc<dyn crate::transport::SessionOps>,
        session_states: std::sync::Arc<
            parking_lot::Mutex<std::collections::HashMap<String, crate::wire::WireStatus>>,
        >,
        latest: std::sync::Arc<parking_lot::Mutex<crate::wire::WireStatus>>,
        current_session_id: impl Into<String>,
    ) -> Self {
        Self {
            session_ops,
            session_states,
            latest,
            current_session_id: current_session_id.into(),
        }
    }
}

#[async_trait]
impl crate::SessionObservabilityOps for LiveSessionObservability {
    async fn authoritative_snapshot(
        &self,
        session_id: &str,
    ) -> Result<crate::wire::WireSessionSnapshot> {
        if let Ok(snapshot) = self.session_ops.session_snapshot(session_id).await {
            return Ok(snapshot);
        }
        let live = self
            .session_states
            .lock()
            .get(session_id)
            .cloned()
            .or_else(|| (session_id == self.current_session_id).then(|| self.latest.lock().clone()))
            .ok_or_else(|| anyhow::anyhow!("session {session_id} is not available"))?;
        Ok(crate::wire::WireSessionSnapshot::from(&live))
    }

    async fn list_session_messages(
        &self,
        _request: &crate::session_observability::ListSessionMessagesRequest,
    ) -> Result<crate::session_observability::SessionMessagePage> {
        anyhow::bail!("list_session_messages is not wired in this fixture")
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/testing/tool_ops.rs"
));
