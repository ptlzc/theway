//! gRPC client half of the transport crate (TUI / future local clients).
//!
//! [`GrpcClient`] wraps the generated tonic client with the typed calls a UI
//! needs (state, frames, commands, session + graph control) and the daemon
//! discovery helpers ([`read_port_file`], [`probe`], [`spawn_daemon`],
//! [`wait_ready`]) that implement the `daemon-client` capability: find the
//! daemon via a per-cwd discovery file (`<THEWAY_DIR>/daemon-port-<cwd-hash>`,
//! carrying `<port> <pid>`) or the default port 44777, verify it is alive with
//! a `get_state` health probe, and spawn one on demand.
//!
//! Loopback-only, same trust model as the daemon itself: no auth is performed
//! beyond the loopback bind the daemon uses.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Child;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::{Stream, StreamExt as _};
use tonic::codec::Streaming;
use tonic::transport::Channel;

use crate::proto::theway_grpc;
use crate::proto::theway_grpc::command_service_client::CommandServiceClient;
use crate::proto::theway_grpc::event_service_client::EventServiceClient;
use crate::proto::theway_grpc::graph_engine_service_client::GraphEngineServiceClient;
use crate::proto::theway_grpc::session_service_client::SessionServiceClient;
use crate::proto::theway_grpc::settings_service_client::SettingsServiceClient;
use crate::proto::theway_grpc::tool_service_client::ToolServiceClient;
use crate::proto::theway_grpc::{
    self as proto, ApproveRequest, CreateSessionRequest, DeleteSessionRequest, Empty,
    GetNodeOutputRequest, GraphCancelRequest, GraphListRequest, GraphRetryRequest,
    GraphSkipRequest, RenameSessionRequest, SendMessageRequest, SessionState, SetModelRequest,
    SetSkillDirsRequest, StreamFrame, SwitchSessionRequest,
};
use crate::wire::{
    SessionSummary, WireDaemonConfig, WirePathContext, WirePromptImage, WireToolEditRequest,
    WireToolEditResult, WireToolExecFrame, WireToolExecRequest, WireToolExecResult,
    WireToolFindRequest, WireToolFindResult, WireToolGrepRequest, WireToolGrepResult,
    WireToolListDirRequest, WireToolListDirResult, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolWriteRequest, WireToolWriteResult,
};

/// Default daemon port when no port file exists (`thewayd` binds this when
/// started without `--port`).
pub const DEFAULT_PORT: u16 = 44777;

/// Base directory: `${THEWAY_DIR:-$HOME/.theway}` — re-export of the single
/// implementation in `theway_contract::config` (issue #64), kept here so the
/// `client::base_dir` path stays stable for daemon/client discovery.
pub use theway_contract::config::base_dir;

/// Published daemon endpoint read from the discovery file: `<port> <pid>`.
///
/// `pid` is the daemon that wrote the file; clients compare it against a
/// spawned child's pid ([`wait_ready`]) or check liveness ([`pid_alive`]) so a
/// leftover entry from a dead daemon can never shadow a fresh spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortEntry {
    pub port: u16,
    pub pid: Option<u32>,
}

/// FNV-1a 64-bit over path bytes. Deterministic and dependency-free; used only
/// to turn a cwd path into a short, filesystem-safe discovery-file suffix.
fn fnv64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Canonical (symlink-resolved) cwd used as the discovery-file key; falls back
/// to the input path when canonicalization fails (dir may not exist yet).
fn canonical_cwd(cwd: &Path) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
}

/// Per-cwd daemon discovery file: `<base>/daemon-port-<fnv64(canonical cwd)>`,
/// written by `thewayd` on bind (actual bound port — meaningful when `--port 0`
/// asked for random).
///
/// One file per cwd: concurrent daemons in different worktrees no longer
/// clobber each other, and a stale entry can only ever shadow its own cwd.
/// Both sides (daemon + client) derive the name from their cwd, so they agree
/// by construction; the legacy global `<base>/daemon-port` is no longer read.
pub fn port_file_path(cwd: &Path) -> PathBuf {
    let canonical = canonical_cwd(cwd);
    base_dir().join(format!(
        "daemon-port-{:016x}",
        fnv64(canonical.as_os_str().as_encoded_bytes())
    ))
}

/// Read the published daemon entry, if any. `Ok(None)` = no port file (or empty).
/// A single-token file (pre-pid format) parses with `pid: None`.
pub fn read_port_file(cwd: &Path) -> Result<Option<PortEntry>> {
    let path = port_file_path(cwd);
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let mut parts = contents.split_whitespace();
            let port = parts
                .next()
                .with_context(|| format!("parse daemon port file {}: empty", path.display()))?
                .parse::<u16>()
                .with_context(|| format!("parse daemon port file {}", path.display()))?;
            let pid = match parts.next() {
                Some(raw) => Some(
                    raw.parse::<u32>()
                        .with_context(|| format!("parse daemon pid in {}", path.display()))?,
                ),
                None => None,
            };
            Ok(Some(PortEntry { port, pid }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read daemon port file {}", path.display())),
    }
}

/// Liveness check for a recorded daemon pid: `/proc/<pid>` on Linux. Outside
/// Linux we cannot verify process existence without a libc dep, so treat the
/// entry as live (best effort — the `get_state` probe remains the final arbiter).
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        true
    }
}

/// Remove the discovery file for `cwd` only if the entry still names `pid` —
/// never delete a successor daemon's entry (the file may have been overwritten).
pub fn remove_port_file_if_owner(cwd: &Path, pid: u32) {
    if matches!(read_port_file(cwd), Ok(Some(entry)) if entry.pid == Some(pid)) {
        let _ = std::fs::remove_file(port_file_path(cwd));
    }
}

/// Typed client for the six `theway.grpc.v1` domain services.
///
/// Cheap to clone (the underlying channel is `Arc`-shared); command calls take
/// `&mut self` because tonic's generated unary methods do. `stream_events`
/// returns the raw frame stream — the caller selects on it (snapshot frames
/// replace the full state, event frames are increments).
#[derive(Clone, Debug)]
pub struct GrpcClient {
    session: SessionServiceClient<Channel>,
    command: CommandServiceClient<Channel>,
    graph: GraphEngineServiceClient<Channel>,
    events: EventServiceClient<Channel>,
    settings: SettingsServiceClient<Channel>,
    tools: ToolServiceClient<Channel>,
    addr: String,
}

/// Tool exec frame stream returned by [`GrpcClient::tool_exec`] (issue #75):
/// zero or more output chunks followed by the terminal exit frame; transport
/// failures surface per item.
pub type ToolExecClientStream = Pin<Box<dyn Stream<Item = Result<WireToolExecFrame>> + Send>>;

impl GrpcClient {
    /// Connect to `host:port` (no scheme). Fails fast when nothing listens.
    pub async fn connect(addr: &str) -> Result<Self> {
        let channel = Channel::from_shared(format!("http://{addr}"))
            .with_context(|| format!("connect to daemon at {addr}"))?
            .connect()
            .await
            .with_context(|| format!("connect to daemon at {addr}"))?;
        Ok(Self {
            session: SessionServiceClient::new(channel.clone()),
            command: CommandServiceClient::new(channel.clone()),
            graph: GraphEngineServiceClient::new(channel.clone()),
            events: EventServiceClient::new(channel.clone()),
            settings: SettingsServiceClient::new(channel.clone()),
            tools: ToolServiceClient::new(channel),
            addr: addr.to_string(),
        })
    }

    /// Address this client is connected to (`host:port`).
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Full structured state (health probe: a live daemon answers this).
    pub async fn get_state(&mut self) -> Result<SessionState> {
        let state = self
            .session
            .get_state(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("get_state: {e}"))?
            .into_inner();
        Ok(state)
    }

    /// Open the snapshot/event frame stream. The stream ends when the daemon
    /// dies or the event loop exits; the caller is responsible for reconnect.
    pub async fn stream_events(&mut self) -> Result<Streaming<StreamFrame>> {
        let response = self
            .events
            .stream_events(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("stream_events: {e}"))?;
        Ok(response.into_inner())
    }

    /// Submit a message. `interrupt` = stop the current turn and run now
    /// (INTERRUPT), otherwise queue after the current turn (QUEUE).
    pub async fn send_message(
        &mut self,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        let accepted = self
            .command
            .send_message(SendMessageRequest {
                text,
                images: images
                    .into_iter()
                    .map(|image| proto::Image {
                        data: image.data,
                        name: image.name,
                    })
                    .collect(),
                mode: if interrupt {
                    theway_grpc::MessageMode::Interrupt
                } else {
                    theway_grpc::MessageMode::Queue
                }
                .into(),
                session_id: None,
            })
            .await
            .map_err(|e| anyhow::anyhow!("send_message: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Switch the daemon's active model.
    pub async fn set_model(&mut self, spec: &str) -> Result<bool> {
        let accepted = self
            .command
            .set_model(SetModelRequest {
                spec: spec.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("set_model: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Stop the in-flight turn (same as a local Ctrl-C). Does not cancel DAG runs.
    pub async fn cancel(&mut self) -> Result<bool> {
        let accepted = self
            .command
            .cancel(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("cancel: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Resolve a pending control-plane prompt (approve / deny).
    pub async fn approve(&mut self, approve: bool) -> Result<bool> {
        let accepted = self
            .command
            .approve(ApproveRequest { approve })
            .await
            .map_err(|e| anyhow::anyhow!("approve: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Switch the daemon to another session (aborts an in-flight turn).
    pub async fn switch_session(&mut self, id: &str) -> Result<bool> {
        let accepted = self
            .session
            .switch_session(SwitchSessionRequest {
                session_id: id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("switch_session: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// List sessions (oldest → newest) plus the daemon's current session id.
    pub async fn list_sessions(&mut self) -> Result<(Vec<SessionSummary>, String)> {
        let response = self
            .session
            .list_sessions(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("list_sessions: {e}"))?
            .into_inner();
        let sessions = response
            .sessions
            .iter()
            .map(|s| SessionSummary {
                session_id: s.session_id.clone(),
                name: s.name.clone(),
                cwd: s.cwd.clone(),
                model: s.model.clone(),
                created_at: s.created_at.clone(),
                last_activity_at: s.last_activity_at,
                graph_count: s.graph_count,
                active_graph_count: s.active_graph_count,
                busy: s.busy,
                preview: s.preview.clone(),
            })
            .collect();
        Ok((sessions, response.current_session_id))
    }

    /// Create a session (becoming current flows through the daemon's event loop).
    pub async fn create_session(&mut self, name: Option<String>) -> Result<SessionSummary> {
        let response = self
            .session
            .create_session(CreateSessionRequest { name })
            .await
            .map_err(|e| anyhow::anyhow!("create_session: {e}"))?
            .into_inner();
        let session = response
            .session
            .context("create_session returned no session summary")?;
        Ok(SessionSummary {
            session_id: session.session_id,
            name: session.name,
            cwd: session.cwd,
            model: session.model,
            created_at: session.created_at,
            last_activity_at: session.last_activity_at,
            graph_count: session.graph_count,
            active_graph_count: session.active_graph_count,
            busy: session.busy,
            preview: session.preview,
        })
    }

    /// Rename a session (full id or unique prefix).
    pub async fn rename_session(&mut self, id: &str, name: &str) -> Result<bool> {
        let accepted = self
            .session
            .rename_session(RenameSessionRequest {
                session_id: id.to_string(),
                name: name.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("rename_session: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Delete a session. `Ok(non_empty)` = refused, these run ids still running;
    /// `Ok(empty)` = deleted.
    pub async fn delete_session(&mut self, id: &str) -> Result<Vec<String>> {
        let response = self
            .session
            .delete_session(DeleteSessionRequest {
                session_id: id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("delete_session: {e}"))?;
        Ok(response.into_inner().running_run_ids)
    }

    // ── path context (issue #68) ───────────────────────────────────────

    /// Daemon path context: home / base / work_dir plus the current skill
    /// search directories.
    pub async fn get_path_context(&mut self) -> Result<WirePathContext> {
        let response = self
            .session
            .get_path_context(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("get_path_context: {e}"))?
            .into_inner();
        Ok(crate::proto::wire_path_context_from_proto(&response))
    }

    /// Replace the extra skill directories dynamically. `Ok(true)` = the
    /// daemon queued the command; the event loop applies it authoritatively
    /// (hot-reload).
    pub async fn set_skill_dirs(&mut self, dirs: &[String]) -> Result<bool> {
        let accepted = self
            .session
            .set_skill_dirs(SetSkillDirsRequest {
                dirs: dirs.to_vec(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("set_skill_dirs: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    // ── settings / config (issue #72) ─────────────────────────────────

    /// Current daemon configuration view (fields the daemon knows about).
    pub async fn get_config(&mut self) -> Result<WireDaemonConfig> {
        let response = self
            .settings
            .get_config(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("get_config: {e}"))?
            .into_inner();
        Ok(crate::proto::daemon_config_from_proto(&response))
    }

    /// Push a partial configuration update. `Ok(true)` = the daemon queued
    /// the command; the serialized event loop applies it authoritatively.
    pub async fn set_config(&mut self, config: &WireDaemonConfig) -> Result<bool> {
        let accepted = self
            .settings
            .set_config(crate::proto::daemon_config_to_proto(config))
            .await
            .map_err(|e| anyhow::anyhow!("set_config: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Same operation as [`set_config`](Self::set_config), on the `Configure`
    /// method (kept so JSON-RPC / WS / MCP clients can align on one verb).
    pub async fn configure(&mut self, config: &WireDaemonConfig) -> Result<bool> {
        let accepted = self
            .settings
            .configure(crate::proto::daemon_config_to_proto(config))
            .await
            .map_err(|e| anyhow::anyhow!("configure: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    // ── tool operations (issue #75) ───────────────────────────────────

    /// Read a file in the daemon's execution environment (1-based line
    /// pagination, same window semantics as the `read` agent tool).
    pub async fn tool_read(&mut self, request: &WireToolReadRequest) -> Result<WireToolReadResult> {
        let response = self
            .tools
            .read_file(crate::tools::read_file_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_read: {e}"))?;
        Ok(crate::tools::read_file_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Write (create/overwrite) a file in the daemon's execution environment.
    pub async fn tool_write(
        &mut self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult> {
        let response = self
            .tools
            .write_file(crate::tools::write_file_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_write: {e}"))?;
        Ok(crate::tools::write_file_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Search-and-replace edit in the daemon's execution environment.
    pub async fn tool_edit(&mut self, request: &WireToolEditRequest) -> Result<WireToolEditResult> {
        let response = self
            .tools
            .edit_file(crate::tools::edit_file_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_edit: {e}"))?;
        Ok(crate::tools::edit_file_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Run a shell command line in the daemon's execution environment; the
    /// returned stream yields zero or more output chunks (interleaved
    /// stdout/stderr) followed by the terminal exit frame.
    pub async fn tool_exec(
        &mut self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecClientStream> {
        let response = self
            .tools
            .exec_command(crate::tools::exec_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_exec: {e}"))?;
        let frames = response.into_inner().map(|item| {
            item.map(|frame| crate::tools::exec_frame_from_proto(&frame))
                .map_err(|e| anyhow::anyhow!("tool_exec stream: {e}"))
        });
        Ok(Box::pin(frames))
    }

    /// Unary shape of [`tool_exec`](Self::tool_exec): collect the whole
    /// frame stream into one result (the JSON-RPC surface returns this).
    pub async fn tool_exec_collect(
        &mut self,
        request: &WireToolExecRequest,
    ) -> Result<WireToolExecResult> {
        let mut stream = self.tool_exec(request).await?;
        let mut result = WireToolExecResult {
            output: String::new(),
            code: -1,
            timed_out: false,
            duration_ms: 0,
        };
        while let Some(frame) = stream.next().await {
            match frame? {
                WireToolExecFrame::Output { text } => result.output.push_str(&text),
                WireToolExecFrame::Exit {
                    code,
                    timed_out,
                    duration_ms,
                } => {
                    result.code = code;
                    result.timed_out = timed_out;
                    result.duration_ms = duration_ms;
                }
            }
        }
        Ok(result)
    }

    /// List one directory level in the daemon's execution environment.
    pub async fn tool_list_dir(
        &mut self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult> {
        let response = self
            .tools
            .list_dir(crate::tools::list_dir_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_list_dir: {e}"))?;
        Ok(crate::tools::list_dir_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Regex content search under a root in the daemon's execution
    /// environment (gitignore-aware on the daemon side).
    pub async fn tool_grep(&mut self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult> {
        let response = self
            .tools
            .grep(crate::tools::grep_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_grep: {e}"))?;
        Ok(crate::tools::grep_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Filename-glob search under a root in the daemon's execution
    /// environment.
    pub async fn tool_find(&mut self, request: &WireToolFindRequest) -> Result<WireToolFindResult> {
        let response = self
            .tools
            .find(crate::tools::find_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_find: {e}"))?;
        Ok(crate::tools::find_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Save a cross-session memory entry on the daemon side.
    pub async fn tool_memory_save(
        &mut self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult> {
        let response = self
            .tools
            .memory_save(crate::tools::memory_save_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_save: {e}"))?;
        Ok(crate::tools::memory_save_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// List the daemon-side memory entries.
    pub async fn tool_memory_list(
        &mut self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult> {
        let response = self
            .tools
            .memory_list(crate::tools::memory_list_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_list: {e}"))?;
        Ok(crate::tools::memory_list_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Read one daemon-side memory entry's content.
    pub async fn tool_memory_read(
        &mut self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult> {
        let response = self
            .tools
            .memory_read(crate::tools::memory_read_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_read: {e}"))?;
        Ok(crate::tools::memory_read_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Forget (delete) one daemon-side memory entry.
    pub async fn tool_memory_forget(
        &mut self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult> {
        let response = self
            .tools
            .memory_forget(crate::tools::memory_forget_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_memory_forget: {e}"))?;
        Ok(crate::tools::memory_forget_response_from_proto(
            &response.into_inner(),
        ))
    }

    /// Two-phase skill install on the daemon side: without `confirm` the
    /// call is a read-only preview and installs nothing.
    pub async fn tool_skill_install(
        &mut self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult> {
        let response = self
            .tools
            .skill_install(crate::tools::skill_install_request_to_proto(request))
            .await
            .map_err(|e| anyhow::anyhow!("tool_skill_install: {e}"))?;
        Ok(crate::tools::skill_install_response_from_proto(
            &response.into_inner(),
        ))
    }

    // ── graph control (DAG + goal runs) ────────────────────────────────

    pub async fn graph_cancel(&mut self, run_id: &str) -> Result<bool> {
        let accepted = self
            .graph
            .graph_cancel(GraphCancelRequest {
                run_id: run_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_cancel: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    pub async fn graph_retry(
        &mut self,
        run_id: &str,
        node_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let response = self
            .graph
            .graph_retry(GraphRetryRequest {
                run_id: run_id.to_string(),
                node_id: node_id.map(str::to_string),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_retry: {e}"))?;
        Ok(response.into_inner().reset_node_ids)
    }

    pub async fn graph_skip(&mut self, run_id: &str, node_id: &str) -> Result<bool> {
        let skipped = self
            .graph
            .graph_skip(GraphSkipRequest {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_skip: {e}"))?;
        Ok(skipped.into_inner().skipped)
    }

    /// One session's graph runs (DagRunSnapshot shape).
    pub async fn graph_list(&mut self, session_id: &str) -> Result<Vec<proto::DagRunSnapshot>> {
        let response = self
            .graph
            .graph_list(GraphListRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_list: {e}"))?;
        Ok(response.into_inner().runs)
    }

    /// A DAG node's full output text from an offset.
    pub async fn get_node_output(
        &mut self,
        run_id: &str,
        node_id: &str,
        offset: u64,
    ) -> Result<proto::GetNodeOutputResponse> {
        let response = self
            .graph
            .get_node_output(GetNodeOutputRequest {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
                offset,
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_node_output: {e}"))?;
        Ok(response.into_inner())
    }
}

/// Health probe: `get_state` with a short timeout. `Ok` = a live daemon answered;
/// `Err` = unreachable or unresponsive (never hangs — the timeout bounds it).
pub async fn probe(addr: &str, timeout: Duration) -> Result<SessionState> {
    let mut client = GrpcClient::connect(addr).await?;
    let state = tokio::time::timeout(timeout, client.get_state())
        .await
        .with_context(|| {
            format!("daemon at {addr} did not answer get_state within {timeout:?}")
        })??;
    Ok(state)
}

/// Candidate address list for discovery: the per-cwd port-file port first
/// (when present and its pid is alive), then the default port.
pub fn candidate_addrs(cwd: &Path) -> Vec<String> {
    let mut addrs = Vec::new();
    if let Ok(Some(entry)) = read_port_file(cwd) {
        if entry.pid.map(pid_alive).unwrap_or(true) {
            addrs.push(format!("127.0.0.1:{}", entry.port));
        }
    }
    addrs.push(format!("127.0.0.1:{DEFAULT_PORT}"));
    addrs
}

/// Discover a running daemon for `cwd`: probe the per-cwd port-file address,
/// then the default port. Returns the first address that answers `get_state`,
/// or `None`.
pub async fn discover(timeout: Duration, cwd: &Path) -> Result<Option<String>> {
    for addr in candidate_addrs(cwd) {
        if probe(&addr, timeout).await.is_ok() {
            return Ok(Some(addr));
        }
    }
    Ok(None)
}

/// Locate the `thewayd` binary: sibling of the current executable first
/// (dev/install layouts put both bins in the same target dir), then `PATH`.
pub fn daemon_binary() -> Option<PathBuf> {
    let exe_name = format!("thewayd{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent()?.join(&exe_name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(&exe_name))
            .find(|candidate| candidate.is_file())
    })
}

/// Spawn a daemon on demand: `thewayd --port 0 --cwd <cwd>` plus any extra
/// launch args (model/session selection, approval flags, …). Inherits cwd (overridden
/// by `--cwd`) and the environment. The caller must [`wait_ready`] for it.
pub fn spawn_daemon(cwd: &Path, extra_args: &[String]) -> Result<Child> {
    let binary =
        daemon_binary().context("thewayd binary not found (sibling of theway or on PATH)")?;
    let mut command = std::process::Command::new(&binary);
    command.arg("--port").arg("0").arg("--cwd").arg(cwd);
    for arg in extra_args {
        command.arg(arg);
    }
    // Note: stderr/stdout inherit so daemon diagnostics land on the terminal.
    let child = command
        .spawn()
        .with_context(|| format!("spawn {}", binary.display()))?;
    Ok(child)
}

/// Wait for a spawned daemon (`pid`) to become ready: poll the per-cwd port
/// file until it carries an entry whose pid matches the child, then
/// `get_state` until it answers (or the timeout expires). A leftover entry
/// from a dead daemon (different pid) is ignored — the pre-existing-file race
/// that broke cold starts is gone.
pub async fn wait_ready(timeout: Duration, cwd: &Path, pid: u32) -> Result<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    // The port file is written by the daemon on bind — poll for it first. Only
    // the entry naming our child counts; anything else is stale/foreign.
    let port = loop {
        if let Ok(Some(entry)) = read_port_file(cwd) {
            if entry.pid == Some(pid) {
                break entry.port;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "daemon (pid {pid}) did not publish {} within {timeout:?}",
                port_file_path(cwd).display()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let addr = format!("127.0.0.1:{port}");
    // Then probe until the gRPC surface answers (bind → serve is nearly
    // immediate, but a slow machine may need a few tries).
    loop {
        if probe(&addr, Duration::from_millis(500)).await.is_ok() {
            return Ok(addr);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("daemon at {addr} did not become ready within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
// Test files live in `tests/transport/client/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("client");
