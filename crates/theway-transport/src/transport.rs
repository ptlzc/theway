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

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

use crate::wire::{SessionSummary, WireCommand, WireDaemonConfig, WirePathContext, WireStatus};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};

#[async_trait]
pub trait SessionOps: Send + Sync {
    /// Every session in the cwd-scoped repo, oldest → newest, enriched with live graph
    /// counts from the shared DAG engine.
    async fn list(&self) -> Result<Vec<SessionSummary>>;

    /// Create a new session (cwd inherited from the current one). Returns the new id;
    /// *becoming current* is a separate `SwitchSession` command through the event loop.
    async fn create(&self) -> Result<String>;

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
    /// Full `WireStatus` snapshots broadcast to SSE / WS / gRPC subscribers.
    pub snapshot_tx: broadcast::Sender<WireStatus>,
    /// Latest snapshot (served by `GET /state` / `GetState`).
    pub latest: Arc<Mutex<WireStatus>>,
    /// Event plane (graph mode): subagent started/output/metrics/completed.
    pub events: broadcast::Sender<AgentJobEvent>,
    /// Event plane (graph mode): DAG engine node_status / run_status.
    pub dag_events: broadcast::Sender<DagEvent>,
    /// Slash-command completer backing `POST /complete`.
    pub completer: SlashCompleter,
    /// Subagent job registry (GetNodeOutput / snapshot source).
    pub registry: AgentJobRegistry,
    /// DAG orchestration engine (graph cancel/retry/skip/checkpoint/restore).
    pub dag_engine: Arc<DagEngine>,
    /// session-resource-model: session lifecycle ops (list/create/rename/delete) for the
    /// gRPC/HTTP session surfaces. Sync query/mutation only — *switching* the current
    /// session goes through `WireCommand::SwitchSession` on the serialized event loop.
    pub session_ops: Arc<dyn crate::transport::SessionOps>,
    /// Shared daemon path context (issue #68): served by `GetPathContext`,
    /// optimistically updated by `SetSkillDirs` before the event loop applies
    /// the change authoritatively. Built once in
    /// [`transport_endpoints`](crate::host::TransportHost::transport_endpoints)
    /// and shared with the kernel-side copy.
    pub path_context: std::sync::Arc<std::sync::RwLock<WirePathContext>>,
    /// Shared daemon configuration view (issue #72): served by `GetConfig`,
    /// optimistically merged by `SetConfig` / `Configure` before the event
    /// loop applies the same patch authoritatively. Built once in
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
