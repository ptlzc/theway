//! Tests for `grpc` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::testing::{
    ChannelCommandOps, FakeSessionOps, FakeStorageOps, FakeToolOps, LiveSessionObservability,
    SharedSettingsOps, empty_sidebar_snapshot,
};
use crate::wire::{
    WireAgentEvent, WireContextUsage, WireDaemonConfig, WireDagEvent, WireDagRunSnapshot,
    WireFeedBlockPatch, WireNodeOutput, WirePathContext, WireStatusUpdate,
};
use std::collections::HashMap;
use std::time::Duration;

mod stream;
mod extensions;
mod storage;

#[derive(Default)]
struct TestJobOps {
    nodes: Mutex<HashMap<(String, String), WireNodeOutput>>,
}

impl TestJobOps {
    fn insert(&self, run_id: &str, node_id: &str, output: WireNodeOutput) {
        self.nodes
            .lock()
            .insert((run_id.to_string(), node_id.to_string()), output);
    }
}

impl JobOps for TestJobOps {
    fn node_output(&self, run_id: &str, node_id: &str) -> WireNodeOutput {
        self.nodes
            .lock()
            .get(&(run_id.to_string(), node_id.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn interrupt_node(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }

    fn steer_node(&self, _run_id: &str, _node_id: &str, _text: String) -> bool {
        false
    }
}

#[derive(Default)]
struct TestGraphOps {
    runs: Mutex<HashMap<String, Vec<WireDagRunSnapshot>>>,
}

impl GraphOps for TestGraphOps {
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
    ) -> anyhow::Result<Vec<crate::wire::WireGraphCheckpoint>> {
        Ok(Vec::new())
    }

    fn restore(&self, _session_id: &str, _snapshot: &str) -> anyhow::Result<String> {
        anyhow::bail!("not configured")
    }

    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.runs.lock().get(session_id).cloned().unwrap_or_default()
    }
}

fn fixture_snapshot(feed_line: &str) -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec![feed_line.into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: crate::wire::WireExtensionSnapshot::default(),
        system_context: String::new(),
    }
}

fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<WireCommand>) {
    let (state, command_rx, _ops, _tools) = grpc_state_with_ops();
    (state, command_rx)
}

/// Same fixture plus handles on the fake SessionOps (seeded with the owning
/// session) and the fake ToolOps (issue #75) so RPC tests can seed and
/// inspect the resource sets.
fn grpc_state_with_ops() -> (
    GrpcState,
    mpsc::UnboundedReceiver<WireCommand>,
    Arc<FakeSessionOps>,
    Arc<FakeToolOps>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let latest = Arc::new(Mutex::new(fixture_snapshot("ready")));
    let (event_tx, _) = broadcast::channel::<WireAgentEvent>(16);
    let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
    let (dag_event_tx, _) = broadcast::channel::<WireDagEvent>(16);
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("test-session");
    let tool_ops = Arc::new(FakeToolOps::new());
    let graph_ops: Arc<dyn crate::GraphOps> = Arc::new(TestGraphOps::default());
    let storage_ops: Arc<dyn crate::StorageOps> = Arc::new(FakeStorageOps::new());
    let path_context = Arc::new(std::sync::RwLock::new(WirePathContext::default()));
    let daemon_config = Arc::new(std::sync::RwLock::new(WireDaemonConfig::default()));
    let session_states = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let external_ops: Arc<dyn crate::ExternalProtocolOps> =
        Arc::new(crate::CompositeExternalProtocolOps::new(
            Arc::new(ChannelCommandOps::new(command_tx.clone())),
            session_ops.clone(),
            Arc::new(LiveSessionObservability::new(
                session_ops.clone(),
                session_states.clone(),
                latest.clone(),
                "test-session",
            )),
            graph_ops.clone(),
            tool_ops.clone(),
            storage_ops.clone(),
            Arc::new(SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                command_tx.clone(),
            )),
        ));
    (
        GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            session_states,
            events: event_tx,
            dag_events: dag_event_tx,
            job_ops: Arc::new(TestJobOps::default()),
            graph_ops,
            session_ops: session_ops.clone(),
            session_id: Arc::new(std::sync::RwLock::new("test-session".into())),
            path_context,
            daemon_config,
            tool_ops: tool_ops.clone(),
            storage_ops,
            external_ops,
            agent_fwd,
        },
        command_rx,
        session_ops,
        tool_ops,
    )
}

/// Rebuild the composed external service after a test mutates one of the
/// shared views (`path_context` / `daemon_config`).
fn rebind_external_ops(state: &mut GrpcState) {
    let current = state.session_id.read().unwrap().clone();
    state.external_ops = Arc::new(crate::CompositeExternalProtocolOps::new(
        Arc::new(ChannelCommandOps::new(state.commands.clone())),
        state.session_ops.clone(),
        Arc::new(LiveSessionObservability::new(
            state.session_ops.clone(),
            state.session_states.clone(),
            state.latest.clone(),
            current.clone(),
        )),
        state.graph_ops.clone(),
        state.tool_ops.clone(),
        state.storage_ops.clone(),
        Arc::new(SharedSettingsOps::new(
            state.path_context.clone(),
            state.daemon_config.clone(),
            state.commands.clone(),
        )),
    ));
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/state.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/commands.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/stream_events.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/node_output.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/subscribers.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/transport.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/session_isolation.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/graph.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/sessions.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/paths.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/config.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/tools.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/grpc/sections/shared_service.rs"));

