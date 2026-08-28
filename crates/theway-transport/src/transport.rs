//! Transport servers.
//!
//! Protocol layer for the theway agent runtime: the HTTP/SSE + WebSocket
//! (axum) and gRPC (tonic) servers plus the proto wire codecs. The servers
//! program against the public channel/state surface of [`TransportEndpoints`]
//! and the [`crate::host::TransportHost`] trait, and never touch kernel
//! internals — the kernel (`theway-daemon`'s `TurnHost`) implements
//! `TransportHost` and drives the serialized transport event loop.
//!
//! Entry points (assembled by the daemon binary):
//! - [`crate::http::run_web`] / [`crate::grpc::run_grpc`] — full drivers (bind,
//!   channels, spawn server, run the event loop).
//! - [`crate::http::serve_web`] / [`crate::grpc::serve_grpc`] — spawn just the
//!   protocol server on a bound listener.

// ──────────────────────────────────────────────────────────────────────────
// Transport endpoints + shared host surface
// ──────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

use crate::wire::{
    SessionSummary, ToolError, WireAgentEvent, WireCommand, WireDaemonConfig, WireDagEvent,
    WireDagRunSnapshot, WireGraphCheckpoint, WireLoadCronJobsRequest, WireLoadCronJobsResult,
    WireLoadDagRunsRequest, WireLoadDagRunsResult, WireLoadTriggerRulesRequest,
    WireLoadTriggerRulesResult, WireNodeOutput, WirePathContext, WireSaveCronJobsRequest,
    WireSaveCronJobsResult, WireSaveDagRunRequest, WireSaveDagRunResult,
    WireSaveTriggerRulesRequest, WireSaveTriggerRulesResult, WireStatus, WireStatusUpdate,
    WireToolEditRequest, WireToolEditResult, WireToolExecFrame, WireToolExecRequest,
    WireToolFindRequest, WireToolFindResult, WireToolGrepRequest, WireToolGrepResult,
    WireToolListDirRequest, WireToolListDirResult, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolWriteRequest, WireToolWriteResult,
};

/// Read/control access to subagent jobs. Protocol servers depend on this seam,
/// not on the runtime registry that happens to back it in the daemon.
pub trait JobOps: Send + Sync {
    fn node_output(&self, run_id: &str, node_id: &str) -> WireNodeOutput;
    fn interrupt_node(&self, run_id: &str, node_id: &str) -> bool;
    fn steer_node(&self, run_id: &str, node_id: &str, text: String) -> bool;
}

/// DAG control/checkpoint access owned by the host application. All values
/// crossing this seam are transport DTOs or serialized snapshots.
pub trait GraphOps: Send + Sync {
    fn cancel_run(&self, run_id: &str, reason: Option<&str>);
    fn retry(&self, run_id: &str, node_ids: Option<&[String]>) -> Vec<String>;
    fn skip(&self, run_id: &str, node_id: &str) -> bool;
    fn checkpoints(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) -> Result<Vec<WireGraphCheckpoint>>;
    fn restore(&self, session_id: &str, snapshot: &str) -> Result<String>;
    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot>;
}

/// Empty operation seams for clients/tests that only exercise unrelated
/// protocol services.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableJobOps;

impl JobOps for UnavailableJobOps {
    fn node_output(&self, _run_id: &str, _node_id: &str) -> WireNodeOutput {
        WireNodeOutput::default()
    }

    fn interrupt_node(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }

    fn steer_node(&self, _run_id: &str, _node_id: &str, _text: String) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableGraphOps;

impl GraphOps for UnavailableGraphOps {
    fn cancel_run(&self, _run_id: &str, _reason: Option<&str>) {}

    fn retry(&self, _run_id: &str, _node_ids: Option<&[String]>) -> Vec<String> {
        Vec::new()
    }

    fn skip(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }

    fn checkpoints(
        &self,
        _session_id: &str,
        _run_id: Option<&str>,
    ) -> Result<Vec<WireGraphCheckpoint>> {
        Ok(Vec::new())
    }

    fn restore(&self, _session_id: &str, _snapshot: &str) -> Result<String> {
        Err(anyhow::anyhow!("graph operations are unavailable"))
    }

    fn list(&self, _session_id: &str) -> Vec<WireDagRunSnapshot> {
        Vec::new()
    }
}

#[async_trait]
pub trait SessionOps: Send + Sync {
    /// Every session in the cwd-scoped repo, oldest → newest, enriched with live graph
    /// counts from the shared DAG engine.
    async fn list(&self) -> Result<Vec<SessionSummary>>;

    /// Create a new session (cwd inherited from the current one). Returns the new id.
    /// `session_id` may be supplied by the caller; `None` lets the daemon generate one.
    /// `metadata` is opaque KV stored with the session summary.
    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String>;

    /// Update arbitrary session metadata KV.
    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()>;

    /// Rename a session (full id or unique prefix). Recorded as a `session_info` entry in
    /// the transcript, so it survives export/import.
    async fn rename(&self, id: &str, name: &str) -> Result<()>;

    /// Delete a session (full id or unique prefix) plus its automation sidecars.
    ///
    /// Delete protection: when the session still has active (running) DAG runs, nothing is
    /// deleted and their run ids are returned — `Ok(non_empty)` means "refused, here is
    /// what is still running". `Ok(empty)` means the session was deleted. Error semantics
    /// (mapping the refusal onto an RPC/HTTP error) are the caller's job.
    async fn delete(&self, id: &str) -> Result<Vec<String>>;
}

/// Exec-command frame stream returned by [`ToolOps::exec_command`]: zero or
/// more output chunks (interleaved stdout/stderr), then a terminal
/// [`WireToolExecFrame::Exit`] frame.
pub type ToolExecStream = Pin<Box<dyn Stream<Item = WireToolExecFrame> + Send>>;

/// File/tool operation handler (issue #75): the daemon-side executor behind
/// the gRPC `ToolService` and the JSON-RPC tool methods. Controllers forward
/// local FS/process work through the transport surfaces; the daemon kernel
/// implements this seam against its execution environment (the `ToolExecutor`
/// seam + memory/skills directories), so the transport crate itself stays
/// free of FS/process policy.
#[async_trait]
pub trait ToolOps: Send + Sync {
    /// Read a file as UTF-8 text with line pagination (1-based `offset`).
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError>;
    /// Write (create/overwrite) a file; missing parent directories are created.
    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError>;
    /// Search-and-replace edit; the result carries the replacement count.
    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError>;
    /// Run a shell command line; the stream ends with the exit frame.
    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError>;
    /// List one directory level (name / kind / size per entry).
    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError>;
    /// Regex content search under a root (gitignore-aware on the daemon side).
    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError>;
    /// Filename-glob search under a root (gitignore-aware on the daemon side).
    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError>;
    /// Save a cross-session memory entry (name + content + metadata).
    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError>;
    /// List memory entries (name / description / type / path).
    async fn memory_list(
        &self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError>;
    /// Read one memory entry's content.
    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError>;
    /// Forget (delete) one memory entry; `removed` reports whether it existed.
    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError>;
    /// Two-phase skill install (preview unless `confirm`), same safety model
    /// as the `install_skill` agent tool.
    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError>;
}

/// Runtime state externalization handler (issue #84): the transport-side seam
/// behind `state.proto` `StorageService`. It mirrors the daemon-side
/// `RuntimeStorage` boundary (#79/#80) without coupling the transport crate to
/// filesystem/SQLite layouts. The daemon kernel implements this seam against
/// its storage adapter; a controller/storage side can drive the same surface
/// through gRPC or JSON-RPC.
#[async_trait]
pub trait StorageOps: Send + Sync {
    /// Persist one DAG run snapshot (opaque JSON `PersistedRun`).
    async fn save_dag_run(&self, request: &WireSaveDagRunRequest) -> Result<WireSaveDagRunResult>;

    /// Load stored DAG runs for a session, optionally one run.
    async fn load_dag_runs(
        &self,
        request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult>;

    /// Replace the session's stored trigger rules.
    async fn save_trigger_rules(
        &self,
        request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult>;

    /// Load the session's stored trigger rules.
    async fn load_trigger_rules(
        &self,
        request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult>;

    /// Replace the session's stored cron jobs.
    async fn save_cron_jobs(
        &self,
        request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult>;

    /// Load the session's stored cron jobs.
    async fn load_cron_jobs(
        &self,
        request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult>;
}

/// Placeholder [`StorageOps`] for daemon builds that have not wired the
/// runtime-storage seam yet: every operation fails with a clear message. The
/// daemon kernel replaces this with the storage-backed implementation in the
/// issue #84/#85 phase; until then the RPC surface exists end-to-end and
/// reports the gap cleanly instead of failing at startup.
#[derive(Clone, Copy, Default)]
pub struct UnavailableStorageOps;

/// Single failure message for every [`UnavailableStorageOps`] operation.
pub const STORAGE_OPS_UNAVAILABLE: &str =
    "runtime state storage is not wired to this daemon yet (issue #84)";

#[async_trait]
impl StorageOps for UnavailableStorageOps {
    async fn save_dag_run(&self, _request: &WireSaveDagRunRequest) -> Result<WireSaveDagRunResult> {
        Err(anyhow::anyhow!(STORAGE_OPS_UNAVAILABLE))
    }

    async fn load_dag_runs(
        &self,
        _request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        Err(anyhow::anyhow!(STORAGE_OPS_UNAVAILABLE))
    }

    async fn save_trigger_rules(
        &self,
        _request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        Err(anyhow::anyhow!(STORAGE_OPS_UNAVAILABLE))
    }

    async fn load_trigger_rules(
        &self,
        _request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        Err(anyhow::anyhow!(STORAGE_OPS_UNAVAILABLE))
    }

    async fn save_cron_jobs(
        &self,
        _request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        Err(anyhow::anyhow!(STORAGE_OPS_UNAVAILABLE))
    }

    async fn load_cron_jobs(
        &self,
        _request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        Err(anyhow::anyhow!(STORAGE_OPS_UNAVAILABLE))
    }
}

/// Placeholder [`ToolOps`] for daemon builds that have not wired the
/// execution-environment seam yet: every operation fails with
/// [`ToolError::Other`]. The daemon kernel replaces this with the real
/// executor-backed implementation in the issue #70 P3 phase; until then the
/// tool-operation RPC surface (gRPC `ToolService` + JSON-RPC tool methods)
/// exists end-to-end and reports the gap cleanly instead of failing at
/// startup.
#[derive(Clone, Copy, Default)]
pub struct UnavailableToolOps;

/// Single failure message for every [`UnavailableToolOps`] operation.
pub const TOOL_OPS_UNAVAILABLE: &str =
    "tool operations are not wired to this daemon's execution environment yet (issue #70 P3)";

#[async_trait]
impl ToolOps for UnavailableToolOps {
    async fn read_file(
        &self,
        _request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn write_file(
        &self,
        _request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn edit_file(
        &self,
        _request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn exec_command(
        &self,
        _request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn list_dir(
        &self,
        _request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn grep(&self, _request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn find(&self, _request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn memory_save(
        &self,
        _request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn memory_list(
        &self,
        _request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn memory_read(
        &self,
        _request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn memory_forget(
        &self,
        _request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }

    async fn skill_install(
        &self,
        _request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        Err(ToolError::Other(anyhow::anyhow!(TOOL_OPS_UNAVAILABLE)))
    }
}

#[derive(Clone)]
pub struct SlashCompleter {
    commands: Vec<String>,
}

impl SlashCompleter {
    /// Build from an explicit slash-command list (server assembles it from its
    /// command registry + skills; the completer itself stays registry-free).
    pub fn from_commands(commands: Vec<String>) -> Self {
        let mut commands = commands;
        commands.sort();
        commands.dedup();
        Self { commands }
    }

    /// Completions for the current input. Returns matching `/command` strings when `line` is a
    /// bare slash token (`/`, `/he`, …) with no whitespace yet; otherwise empty.
    pub fn matches(&self, line: &str) -> Vec<String> {
        let Some(token) = slash_token(line) else {
            return Vec::new();
        };
        let matches: Vec<String> = self
            .commands
            .iter()
            .filter(|c| c.starts_with(token))
            .cloned()
            .collect();
        // Nothing left to complete when the only match is what the user already typed.
        if matches.len() == 1 && matches[0] == token {
            return Vec::new();
        }
        matches
    }
}

/// Extract the slash token at the start of `line` (after leading whitespace). Returns `None`
/// unless the trimmed line begins with `/` and contains no interior whitespace (i.e. the user
/// is still typing the command name, not its arguments).
fn slash_token(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    if trimmed[1..].contains(char::is_whitespace) {
        return None;
    }
    Some(trimmed)
}

pub enum TransportMode {
    Web,
    Grpc,
}

impl TransportMode {
    pub fn label(self) -> &'static str {
        match self {
            TransportMode::Web => "web",
            TransportMode::Grpc => "grpc",
        }
    }
}

/// Public channel/state surface shared with the daemon kernel.
///
/// Built by [`TransportHost::transport_endpoints`](crate::host::TransportHost::transport_endpoints):
/// the server side takes the senders / shared state it needs to build its
/// `HttpState` / `GrpcState`, while the receiver half (`command_rx`) plus
/// `snapshot_tx`/`latest` feed the event loop
/// ([`TransportHost::run_transport_loop`](crate::host::TransportHost::run_transport_loop)).
pub struct TransportEndpoints {
    /// Browser/client commands into the serialized event loop.
    pub command_tx: mpsc::UnboundedSender<WireCommand>,
    /// Event-loop side of the command queue.
    pub command_rx: mpsc::UnboundedReceiver<WireCommand>,
    /// Snapshot publications. The authoritative state stays in `latest`;
    /// routine publications carry only transcript deltas.
    pub snapshot_tx: broadcast::Sender<WireStatusUpdate>,
    /// Latest snapshot (served by `GET /state` / `GetState`).
    pub latest: Arc<Mutex<WireStatus>>,
    /// Event plane (graph mode): subagent started/output/metrics/completed.
    pub events: broadcast::Sender<WireAgentEvent>,
    /// Event plane (graph mode): DAG engine node_status / run_status.
    pub dag_events: broadcast::Sender<WireDagEvent>,
    /// Slash-command completer backing `POST /complete`.
    pub completer: SlashCompleter,
    /// Subagent job read/control operations.
    pub job_ops: Arc<dyn JobOps>,
    /// DAG orchestration operations.
    pub graph_ops: Arc<dyn GraphOps>,
    /// session-resource-model: session lifecycle ops (list/create/rename/delete) for the
    /// gRPC/HTTP session surfaces. Sync query/mutation only; clients address sessions
    /// explicitly by session id.
    pub session_ops: Arc<dyn crate::transport::SessionOps>,
    /// File/tool operation handler (issue #75): backs the gRPC `ToolService`
    /// and the JSON-RPC tool methods. The daemon kernel implements the seam
    /// against its execution environment.
    pub tool_ops: Arc<dyn crate::transport::ToolOps>,
    /// Runtime state storage handler (issue #84): backs the gRPC
    /// `StorageService` and the JSON-RPC state methods. The daemon kernel
    /// implements the seam against the `RuntimeStorage` adapter.
    pub storage_ops: Arc<dyn crate::transport::StorageOps>,
    /// Shared daemon path context (issue #68): served by `GetPathContext`,
    /// optimistically updated by `SetSkillDirs` before the event loop applies
    /// the change authoritatively. Built once in
    /// [`transport_endpoints`](crate::host::TransportHost::transport_endpoints)
    /// and shared with the kernel-side copy.
    pub path_context: std::sync::Arc<std::sync::RwLock<WirePathContext>>,
    /// Shared authoritative daemon configuration view served by `GetConfig`.
    /// The event loop updates it after applying a patch. Built once in
    /// [`transport_endpoints`](crate::host::TransportHost::transport_endpoints)
    /// and shared with the kernel-side copy.
    pub daemon_config: std::sync::Arc<std::sync::RwLock<WireDaemonConfig>>,
    /// Owning session id (checkpoint scope / mount key).
    pub session_id: String,
    /// Abort handle for the registry→events forwarder task spawned in
    /// [`transport_endpoints`](crate::host::TransportHost::transport_endpoints). Clone it
    /// before moving `TransportEndpoints` into
    /// [`run_transport_loop`](crate::host::TransportHost::run_transport_loop).
    pub agent_fwd: tokio::task::AbortHandle,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completer() -> SlashCompleter {
        SlashCompleter::from_commands(vec!["/help".into(), "/model".into()])
    }

    #[test]
    fn lists_commands_and_aliases_for_bare_slash() {
        let m = completer().matches("/");
        assert!(m.contains(&"/help".to_string()));
        assert!(m.contains(&"/model".to_string()));
    }

    #[test]
    fn filters_by_prefix() {
        let m = completer().matches("/mo");
        assert_eq!(m, vec!["/model".to_string()]);
    }

    #[test]
    fn no_completion_once_argument_typed() {
        assert!(completer().matches("/skill test").is_empty());
        assert!(completer().matches("hello").is_empty());
    }

    #[test]
    fn exact_unique_match_is_not_offered() {
        // Already fully typed and unique — nothing left to complete.
        assert!(completer().matches("/thinking").is_empty());
    }
}
