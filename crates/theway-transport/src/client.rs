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
use std::process::Child;
use std::time::Duration;

use anyhow::{Context as _, Result};
use tonic::codec::Streaming;
use tonic::transport::Channel;

use crate::proto::theway_grpc;
use crate::proto::theway_grpc::theway_grpc_client::ThewayGrpcClient;
use crate::proto::theway_grpc::{
    self as proto, ApproveRequest, CreateSessionRequest, DeleteSessionRequest, Empty,
    GetNodeOutputRequest, GraphCancelRequest, GraphListRequest, GraphRetryRequest,
    GraphSkipRequest, RenameSessionRequest, SendMessageRequest, SessionState, SetModelRequest,
    StreamFrame, SwitchSessionRequest,
};
use crate::wire::{SessionSummary, WirePromptImage};

/// Default daemon port when no port file exists (`thewayd` binds this when
/// started without `--port`).
pub const DEFAULT_PORT: u16 = 44777;

/// Base directory: `${THEWAY_DIR:-$HOME/.theway}`. Mirror of the server's
/// `config::base_dir()` — kept local so the client half has no server
/// dependency (they must agree on the same file, which they do by contract).
pub fn base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("THEWAY_DIR") {
        return PathBuf::from(p);
    }
    std::env::var("HOME")
        .map(|home| PathBuf::from(home).join(".theway"))
        .unwrap_or_else(|_| PathBuf::from(".theway"))
}

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

/// Typed client for the `theway.grpc.v1.ThewayGrpc` service.
///
/// Cheap to clone (the underlying channel is `Arc`-shared); command calls take
/// `&mut self` because tonic's generated unary methods do. `stream_events`
/// returns the raw frame stream — the caller selects on it (snapshot frames
/// replace the full state, event frames are increments).
#[derive(Clone, Debug)]
pub struct GrpcClient {
    inner: ThewayGrpcClient<Channel>,
    addr: String,
}

impl GrpcClient {
    /// Connect to `host:port` (no scheme). Fails fast when nothing listens.
    pub async fn connect(addr: &str) -> Result<Self> {
        let inner = ThewayGrpcClient::connect(format!("http://{addr}"))
            .await
            .with_context(|| format!("connect to daemon at {addr}"))?;
        Ok(Self {
            inner,
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
            .inner
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
            .inner
            .stream_events(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("stream_events: {e}"))?;
        Ok(response.into_inner())
    }

    /// Submit a message. `interrupt` = stop the current turn and run now
    /// (INTERRUPT), otherwise queue after the current turn (GUIDE).
    pub async fn send_message(
        &mut self,
        text: String,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        let accepted = self
            .inner
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
                    theway_grpc::MessageMode::Guide
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
            .inner
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
            .inner
            .cancel(Empty {})
            .await
            .map_err(|e| anyhow::anyhow!("cancel: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Resolve a pending control-plane prompt (approve / deny).
    pub async fn approve(&mut self, approve: bool) -> Result<bool> {
        let accepted = self
            .inner
            .approve(ApproveRequest { approve })
            .await
            .map_err(|e| anyhow::anyhow!("approve: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Switch the daemon to another session (aborts an in-flight turn).
    pub async fn switch_session(&mut self, id: &str) -> Result<bool> {
        let accepted = self
            .inner
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
            .inner
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
            .inner
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
            .inner
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
            .inner
            .delete_session(DeleteSessionRequest {
                session_id: id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("delete_session: {e}"))?;
        Ok(response.into_inner().running_run_ids)
    }

    // ── graph control (DAG + goal runs) ────────────────────────────────

    pub async fn graph_cancel(&mut self, run_id: &str) -> Result<bool> {
        let accepted = self
            .inner
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
            .inner
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
            .inner
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
            .inner
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
            .inner
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
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("client");
