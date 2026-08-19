//! Tests for `client` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::wire::WireContextUsage;
use crate::grpc::{serve_grpc, GrpcState};
use crate::proto::{session_state, wire_status};
use crate::testing::{FakeSessionOps, FakeStorageOps, FakeToolOps, empty_sidebar_snapshot};
use crate::feed::WireFeedBlock;
use crate::wire::{ModelEntry, ProviderGroup, WireDaemonConfig, WirePathContext, WireStatus};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};

fn fixture_status(feed_line: &str) -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        model_catalog: vec![ProviderGroup {
            provider: "anthropic".into(),
            has_credential: true,
            models: vec![ModelEntry {
                id: "claude-x".into(),
                name: "Claude X".into(),
            }],
        }],
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: vec![WireFeedBlock::User {
            text: feed_line.into(),
            timestamp: None,
        }],
        feed_lines: vec![feed_line.into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
    }
}

fn grpc_state() -> (GrpcState, mpsc::UnboundedReceiver<crate::wire::WireCommand>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<crate::wire::WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
    let latest = Arc::new(parking_lot::Mutex::new(fixture_status("ready")));
    let (event_tx, _) = broadcast::channel::<AgentJobEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<DagEvent>(16);
    let registry = AgentJobRegistry::new();
    let agent_fwd = {
        let mut rx = registry.subscribe();
        let fwd_tx = event_tx.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let _ = fwd_tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("AgentJobEvent broadcast lagged by {n}, skipping");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .abort_handle()
    };
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    (
        GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            registry,
            dag_engine: Arc::new(DagEngine::new()),
            session_ops,
            session_id: Arc::new(std::sync::RwLock::new("sess-1".into())),
            path_context: Arc::new(std::sync::RwLock::new(WirePathContext::default())),
            daemon_config: Arc::new(std::sync::RwLock::new(WireDaemonConfig::default())),
            tool_ops: Arc::new(FakeToolOps::new()),
            storage_ops: Arc::new(FakeStorageOps::new()),
            agent_fwd,
        },
        command_rx,
    )
}

/// Spawn an in-process gRPC server on a random port and connect a client to it.
/// Returns the client, the event-loop command channel, and the snapshot sender
/// (fixture publishes on demand — there is no running event loop in tests).
async fn client_and_server(
) -> (
    GrpcClient,
    mpsc::UnboundedReceiver<crate::wire::WireCommand>,
    broadcast::Sender<WireStatus>,
) {
    client_and_server_with_path_context(WirePathContext::default()).await
}

/// `client_and_server` variant seeded with an explicit startup path context
/// (issue #68: home/base/work_dir fixed at startup plus initial skills_dirs).
async fn client_and_server_with_path_context(
    path_context: WirePathContext,
) -> (
    GrpcClient,
    mpsc::UnboundedReceiver<crate::wire::WireCommand>,
    broadcast::Sender<WireStatus>,
) {
    let (mut state, command_rx) = grpc_state();
    state.path_context = Arc::new(std::sync::RwLock::new(path_context));
    let snapshot_tx = state.snapshots.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_grpc(listener, state);
    let client = GrpcClient::connect(&addr.to_string()).await.unwrap();
    // The server task lives for the rest of the test; aborting the client's
    // channel on drop is enough to end the stream asserts.
    let _server = server;
    (client, command_rx, snapshot_tx)
}

#[tokio::test]
async fn client_get_state_returns_structured_state() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;
    let state = client.get_state().await.unwrap();
    assert_eq!(state.session_id, "sess-1");
    assert_eq!(state.cwd, "/tmp/theway");
    assert_eq!(state.feed_lines, vec!["ready"]);
}

#[tokio::test]
async fn client_send_message_queues_submit_command() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    let accepted = client
        .send_message(
            "hello daemon".into(),
            vec![crate::wire::WirePromptImage {
                data: "aGVsbG8=".into(),
                name: Some("clip.png".into()),
            }],
            false,
        )
        .await
        .unwrap();
    assert!(accepted);
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Submit {
            text,
            images,
            interrupt,
        } => {
            assert_eq!(text, "hello daemon");
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].name.as_deref(), Some("clip.png"));
            assert!(!interrupt);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_interrupt_mode_maps_to_interrupt_flag() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    client
        .send_message("stop and run".into(), vec![], true)
        .await
        .unwrap();
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Submit { interrupt, .. } => assert!(interrupt),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_cancel_set_model_approve_switch_session_round_trip() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    assert!(client.cancel().await.unwrap());
    assert!(matches!(
        command_rx.recv().await.unwrap(),
        crate::wire::WireCommand::Abort
    ));

    assert!(client.set_model("anthropic:claude-x").await.unwrap());
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetModel { spec } => assert_eq!(spec, "anthropic:claude-x"),
        other => panic!("unexpected command: {other:?}"),
    }

    assert!(client.approve(true).await.unwrap());
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::ResolveControlPlane { approve } => assert!(approve),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn client_switch_session_queues_command_and_rebinds() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    client
        .switch_session("sess-1")
        .await
        .unwrap();
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SwitchSession { id } => assert_eq!(id, "sess-1"),
        other => panic!("unexpected command: {other:?}"),
    }
}

// ── path context (issue #68) ─────────────────────────────────────────

fn startup_path_context() -> WirePathContext {
    WirePathContext {
        home: "/home/dev".into(),
        base: "/home/dev/.theway".into(),
        work_dir: "/home/dev/projects/theway".into(),
        skills_dirs: vec!["/home/dev/.agents/skills".into()],
    }
}

#[tokio::test]
async fn client_get_path_context_returns_startup_paths_and_skill_dirs() {
    let ctx = startup_path_context();
    let (mut client, _command_rx, _snapshot_tx) =
        client_and_server_with_path_context(ctx.clone()).await;

    // Read-only snapshot: startup home/base/work_dir + the initial skills_dirs.
    let got = client.get_path_context().await.unwrap();
    assert_eq!(got, ctx);
}

#[tokio::test]
async fn client_set_skill_dirs_queues_command_and_updates_path_context() {
    let ctx = startup_path_context();
    let (mut client, mut command_rx, _snapshot_tx) =
        client_and_server_with_path_context(ctx.clone()).await;

    let accepted = client
        .set_skill_dirs(&["/skills/a".to_string(), "/skills/b".to_string()])
        .await
        .unwrap();
    assert!(accepted);

    // The event loop receives WireCommand::SetSkillDirs for the authoritative
    // apply (extras replacement + hot-reload).
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetSkillDirs { dirs } => {
            assert_eq!(dirs, vec!["/skills/a", "/skills/b"])
        }
        other => panic!("unexpected command: {other:?}"),
    }

    // Optimistic update: the follow-up read reflects the new dirs while
    // home/base/work_dir stay startup-fixed.
    let got = client.get_path_context().await.unwrap();
    assert_eq!(got.skills_dirs, vec!["/skills/a", "/skills/b"]);
    assert_eq!(got.home, ctx.home);
    assert_eq!(got.base, ctx.base);
    assert_eq!(got.work_dir, ctx.work_dir);

    // Clearing: an empty list is a valid update.
    let accepted = client.set_skill_dirs(&[]).await.unwrap();
    assert!(accepted);
    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::SetSkillDirs { dirs } => assert!(dirs.is_empty()),
        other => panic!("unexpected command: {other:?}"),
    }
    let got = client.get_path_context().await.unwrap();
    assert!(got.skills_dirs.is_empty());
}

#[tokio::test]
async fn client_stream_events_receives_snapshot_frames() {
    let (mut client, _command_rx, snapshot_tx) = client_and_server().await;
    let mut stream = client.stream_events().await.unwrap();
    // The fixture publishes on demand (no event loop in tests).
    snapshot_tx.send(fixture_status("streamed")).unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timed out waiting for frame")
        .expect("stream ended")
        .unwrap();
    match frame.payload {
        Some(crate::proto::theway_grpc::stream_frame::Payload::Snapshot(state)) => {
            assert_eq!(state.session_id, "sess-1");
            assert_eq!(state.feed_lines, vec!["streamed"]);
        }
        other => panic!("expected snapshot payload, got {other:?}"),
    }
}

#[tokio::test]
async fn client_connect_to_dead_port_fails_promptly() {
    // Bind a listener, note the port, drop it — nothing listens anymore.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let err = GrpcClient::connect(&addr).await.unwrap_err().to_string();
    assert!(err.contains("connect"), "{err}");
}

#[tokio::test]
async fn probe_reports_live_daemon() {
    let (client, _command_rx, _snapshot_tx) = client_and_server().await;
    let addr = client.addr().to_string();
    let state = probe(&addr, Duration::from_secs(2)).await.unwrap();
    assert_eq!(state.session_id, "sess-1");
}

#[tokio::test]
async fn probe_fails_on_dead_port() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    assert!(probe(&addr, Duration::from_millis(300)).await.is_err());
}

// ── port-file discovery ───────────────────────────────────────────

/// THEWAY_DIR is process-global; all port-file tests serialize on this lock.
static THEWAY_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_theway_dir(dir: &std::path::Path) {
    // SAFETY: tests are single-threaded per test and serialized on
    // THEWAY_DIR_LOCK; no other thread reads THEWAY_DIR concurrently.
    unsafe { std::env::set_var("THEWAY_DIR", dir) };
}

fn clear_theway_dir() {
    // SAFETY: see with_theway_dir.
    unsafe { std::env::remove_var("THEWAY_DIR") };
}

#[test]
fn port_file_round_trips_the_bound_port() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    assert_eq!(read_port_file(&cwd).unwrap(), None, "no port file yet");
    std::fs::write(port_file_path(&cwd), "44777 1234").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 44777, pid: Some(1234) })
    );
    std::fs::write(port_file_path(&cwd), "0 1").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 0, pid: Some(1) })
    );
    // Pre-pid format (single token) still parses, pid unknown.
    std::fs::write(port_file_path(&cwd), "44777").unwrap();
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 44777, pid: None })
    );
    drop(dir);
    clear_theway_dir();
}

#[test]
fn port_file_with_garbage_is_an_error() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();
    std::fs::write(port_file_path(&cwd), "not-a-port").unwrap();
    assert!(read_port_file(&cwd).is_err());
    std::fs::write(port_file_path(&cwd), "44777 not-a-pid").unwrap();
    assert!(read_port_file(&cwd).is_err());
    drop(dir);
    clear_theway_dir();
}

#[test]
fn candidate_addrs_prefers_live_port_file_then_default() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    // No port file → default only.
    assert_eq!(
        candidate_addrs(&cwd),
        vec![format!("127.0.0.1:{DEFAULT_PORT}")]
    );

    // Entry whose pid is dead → skipped, default only (the stale-entry case
    // that used to break cold starts). Linux-only: outside Linux pid_alive
    // cannot verify, so the entry is probed as a best effort.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", u32::MAX)).unwrap();
    if cfg!(target_os = "linux") {
        assert_eq!(
            candidate_addrs(&cwd),
            vec![format!("127.0.0.1:{DEFAULT_PORT}")]
        );
    }

    // Entry whose pid is alive (ours) → port-file address first, default second.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", std::process::id())).unwrap();
    assert_eq!(
        candidate_addrs(&cwd),
        vec!["127.0.0.1:43001".to_string(), format!("127.0.0.1:{DEFAULT_PORT}")]
    );
    drop(dir);
    clear_theway_dir();
}

#[test]
fn remove_port_file_removes_only_the_owners_entry() {
    let _guard = THEWAY_DIR_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    with_theway_dir(dir.path());
    let cwd = std::env::temp_dir();

    // Own entry → removed.
    std::fs::write(port_file_path(&cwd), format!("43001 {}", std::process::id())).unwrap();
    remove_port_file_if_owner(&cwd, std::process::id());
    assert_eq!(read_port_file(&cwd).unwrap(), None);

    // Foreign entry (a successor daemon) → untouched.
    std::fs::write(port_file_path(&cwd), "43001 424242").unwrap();
    remove_port_file_if_owner(&cwd, std::process::id());
    assert_eq!(
        read_port_file(&cwd).unwrap(),
        Some(PortEntry { port: 43001, pid: Some(424242) })
    );
    drop(dir);
    clear_theway_dir();
}

// ── wire_status (proto → wire) round-trip ─────────────────────────

#[test]
fn session_state_wire_status_round_trips() {
    let status = fixture_status("hello");
    let state = session_state(&status);
    let back = wire_status(&state);
    assert_eq!(back.session_id, "sess-1");
    assert_eq!(back.model, "provider:model");
    assert_eq!(back.cwd, "/tmp/theway");
    assert_eq!(back.feed_lines, vec!["hello"]);
    assert_eq!(back.model_catalog.len(), 1);
    assert_eq!(back.model_catalog[0].provider, "anthropic");
    assert_eq!(back.model_catalog[0].models[0].id, "claude-x");
    assert_eq!(
        back.feed_blocks.len(),
        1,
        "feed blocks must round-trip through the oneof"
    );
    match &back.feed_blocks[0] {
        WireFeedBlock::User { text, .. } => assert_eq!(text, "hello"),
        other => panic!("expected User block, got {other:?}"),
    }
    // Sidebar (non-optional in the wire model) survives the proto round-trip.
    assert_eq!(back.sidebar.skills.total, status.sidebar.skills.total);
}

#[test]
fn session_state_round_trips_feed_block_kinds() {
    let status = WireStatus {
        feed_blocks: vec![
            WireFeedBlock::Assistant {
                text: "answer".into(),
                timestamp: None,
            },
            WireFeedBlock::Thinking {
                text: "pondering".into(),
                timestamp: None,
            },
            WireFeedBlock::Tool {
                name: "read".into(),
                args: "(path=\"x\")".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["ok".into()],
                is_error: false,
                timestamp: None,
            },
            WireFeedBlock::Plain {
                text: "note".into(),
                level: crate::feed::Level::System,
                timestamp: None,
            },
        ],
        ..fixture_status("x")
    };
    let back = wire_status(&session_state(&status));
    let kinds: Vec<&str> = back
        .feed_blocks
        .iter()
        .map(|b| match b {
            WireFeedBlock::User { .. } => "user",
            WireFeedBlock::Assistant { .. } => "assistant",
            WireFeedBlock::Thinking { .. } => "thinking",
            WireFeedBlock::Tool { .. } => "tool",
            WireFeedBlock::ToolResult { .. } => "tool_result",
            WireFeedBlock::Plain { .. } => "plain",
        })
        .collect();
    assert_eq!(
        kinds,
        ["assistant", "thinking", "tool", "tool_result", "plain"]
    );
    match &back.feed_blocks[4] {
        WireFeedBlock::Plain { level, .. } => {
            assert_eq!(*level, crate::feed::Level::System);
        }
        other => panic!("expected Plain block, got {other:?}"),
    }
}

// ── settings / config (issue #72) ─────────────────────────────────────

#[tokio::test]
async fn client_get_config_returns_daemon_view() {
    let (mut client, _command_rx, _snapshot_tx) = client_and_server().await;
    // Fresh fixture starts with an empty config view.
    let config = client.get_config().await.unwrap();
    assert_eq!(config, WireDaemonConfig::default());
}

#[tokio::test]
async fn client_set_config_queues_configure_command() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let patch = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        tui_max_feed_lines: Some(8000),
        ..Default::default()
 };
    assert!(client.set_config(&patch).await.unwrap());

    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Configure { config } => {
            assert_eq!(config, patch);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    // GetConfig remains authoritative until the daemon event loop applies it.
    let config = client.get_config().await.unwrap();
    assert_eq!(config, WireDaemonConfig::default());
}

#[tokio::test]
async fn client_configure_alias_reaches_the_event_loop() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let patch = WireDaemonConfig {
        skills_dirs: vec!["/skills/a".into()],
        ..Default::default()
 };
    assert!(client.configure(&patch).await.unwrap());

    match command_rx.recv().await.unwrap() {
        crate::wire::WireCommand::Configure { config } => {
            assert_eq!(config.skills_dirs, vec!["/skills/a"]);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

// ── tool operations (issue #75) ──────────────────────────────────────

/// `client_and_server` variant that also hands back the fake `ToolOps`
/// behind the server so tool tests can seed files / exec scripts.
async fn client_and_server_with_tools() -> (GrpcClient, Arc<FakeToolOps>) {
    let (mut state, _command_rx) = grpc_state();
    let tools = Arc::new(FakeToolOps::new());
    state.tool_ops = tools.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = serve_grpc(listener, state);
    let client = GrpcClient::connect(&addr.to_string()).await.unwrap();
    (client, tools)
}

#[tokio::test]
async fn client_tool_write_read_edit_round_trip() {
    use crate::wire::{WireToolEditRequest, WireToolReadRequest, WireToolWriteRequest};

    let (mut client, tools) = client_and_server_with_tools().await;

    let written = client
        .tool_write(&WireToolWriteRequest {
            path: "/work/a.txt".into(),
            content: "one\ntwo\nthree\n".into(),
        })
        .await
        .unwrap();
    assert_eq!(written.bytes_written, "one\ntwo\nthree\n".len() as u64);

    let read = client
        .tool_read(&WireToolReadRequest {
            path: "/work/a.txt".into(),
            offset: Some(2),
            limit: None,
        })
        .await
        .unwrap();
    // The window reaches EOF, so the file's trailing newline is preserved.
    assert_eq!(read.content, "two\nthree\n");
    assert_eq!(read.total_lines, 3);
    assert!(!read.truncated);

    let edited = client
        .tool_edit(&WireToolEditRequest {
            path: "/work/a.txt".into(),
            old_string: "two".into(),
            new_string: "TWO".into(),
            replace_all: false,
            range_start: None,
            range_end: None,
        })
        .await
        .unwrap();
    assert_eq!(edited.replacements, 1);
    assert_eq!(
        tools.file_content("/work/a.txt").as_deref(),
        Some("one\nTWO\nthree\n")
    );
}

#[tokio::test]
async fn client_tool_read_missing_surfaces_not_found() {
    use crate::wire::WireToolReadRequest;

    let (mut client, _tools) = client_and_server_with_tools().await;
    let err = client
        .tool_read(&WireToolReadRequest {
            path: "/work/missing.txt".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("tool_read"), "{message}");
    assert!(message.contains("not found"), "{message}");
}

#[tokio::test]
async fn client_tool_exec_collect_concatenates_frames() {
    use crate::wire::WireToolExecRequest;

    let (mut client, tools) = client_and_server_with_tools().await;
    tools.set_exec_frames(vec![
        crate::wire::WireToolExecFrame::Output {
            text: "part1 ".into(),
        },
        crate::wire::WireToolExecFrame::Output {
            text: "part2\n".into(),
        },
        crate::wire::WireToolExecFrame::Exit {
            code: 4,
            timed_out: true,
            duration_ms: 99,
        },
    ]);

    let result = client
        .tool_exec_collect(&WireToolExecRequest {
            command: "slow-cmd".into(),
            cwd: None,
            timeout_ms: Some(100),
        })
        .await
        .unwrap();
    assert_eq!(result.output, "part1 part2\n");
    assert_eq!(result.code, 4);
    assert!(result.timed_out);
    assert_eq!(result.duration_ms, 99);

    // The request reached the handler intact.
    let last = tools.last_exec().unwrap();
    assert_eq!(last.command, "slow-cmd");
    assert_eq!(last.timeout_ms, Some(100));
}

#[tokio::test]
async fn client_tool_exec_streams_frames_individually() {
    use crate::wire::{WireToolExecFrame, WireToolExecRequest};

    let (mut client, tools) = client_and_server_with_tools().await;
    tools.set_exec_frames(vec![
        WireToolExecFrame::Output {
            text: "chunk\n".into(),
        },
        WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 1,
        },
    ]);

    let mut stream = client
        .tool_exec(&WireToolExecRequest {
            command: "echo chunk".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let first = stream.next().await.expect("frame").unwrap();
    assert_eq!(
        first,
        WireToolExecFrame::Output {
            text: "chunk\n".into()
        }
    );
    let last = stream.next().await.expect("frame").unwrap();
    assert!(matches!(
        last,
        WireToolExecFrame::Exit {
            code: 0,
            timed_out: false,
            duration_ms: 1,
        }
    ));
    assert!(stream.next().await.is_none(), "stream ends after exit");
}

#[tokio::test]
async fn client_tool_list_dir_grep_find_round_trip() {
    use crate::wire::{WireToolFindRequest, WireToolGrepRequest, WireToolListDirRequest};

    let (mut client, tools) = client_and_server_with_tools().await;
    tools.seed_dir(
        "/work",
        vec![crate::wire::WireToolDirEntry {
            name: "main.rs".into(),
            kind: "file".into(),
            size: 10,
        }],
    );
    tools.put_file("/work/main.rs", "fn main() {}\n");

    let listed = client
        .tool_list_dir(&WireToolListDirRequest {
            path: "/work".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].kind, "file");

    let grep = client
        .tool_grep(&WireToolGrepRequest {
            pattern: "fn main".into(),
            path: Some("/work".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(grep.matches.len(), 1);
    assert_eq!(grep.matches[0].line_number, 1);

    let find = client
        .tool_find(&WireToolFindRequest {
            pattern: "*.rs".into(),
            path: Some("/work".into()),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(find.paths, vec!["/work/main.rs"]);
}

#[tokio::test]
async fn client_tool_memory_round_trip() {
    use crate::wire::{
        WireToolMemoryForgetRequest, WireToolMemoryListRequest, WireToolMemoryReadRequest,
        WireToolMemorySaveRequest,
    };

    let (mut client, _tools) = client_and_server_with_tools().await;

    let saved = client
        .tool_memory_save(&WireToolMemorySaveRequest {
            name: "prefs".into(),
            content: "dark mode".into(),
            description: Some("ui preferences".into()),
            memory_type: Some("preference".into()),
        })
        .await
        .unwrap();
    assert_eq!(saved.name, "prefs");

    let listed = client
        .tool_memory_list(&WireToolMemoryListRequest {})
        .await
        .unwrap();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].description.as_deref(), Some("ui preferences"));

    let read = client
        .tool_memory_read(&WireToolMemoryReadRequest {
            name: "prefs".into(),
        })
        .await
        .unwrap();
    assert_eq!(read.content, "dark mode");

    let forgot = client
        .tool_memory_forget(&WireToolMemoryForgetRequest {
            name: "prefs".into(),
        })
        .await
        .unwrap();
    assert!(forgot.removed);
}

#[tokio::test]
async fn client_tool_skill_install_preview_then_confirm() {
    use crate::wire::{WireToolSkillInstallRequest, WireToolSkillSource};

    let (mut client, tools) = client_and_server_with_tools().await;

    let preview = client
        .tool_skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Url("https://example.com/skills/git-flow.md".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap();
    assert!(!preview.installed);
    assert_eq!(preview.name, "git-flow");

    let installed = client
        .tool_skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Url("https://example.com/skills/git-flow.md".into()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap();
    assert!(installed.installed);
    assert!(!installed.existing);

    // Both requests reached the handler (preview + confirm, in order).
    let requests = tools.skill_installs();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0].confirm);
    assert!(requests[1].confirm);
}

// ── runtime state storage (issue #84) ──────────────────────────────

#[tokio::test]
async fn client_state_storage_round_trips_dag_trigger_cron() {
    use crate::wire::{
        WireLoadCronJobsRequest, WireLoadDagRunsRequest, WireLoadTriggerRulesRequest,
        WireSaveCronJobsRequest, WireSaveDagRunRequest, WireSaveTriggerRulesRequest,
        WireStoredCronJob, WireStoredTriggerRule,
    };

    let (mut state, _command_rx) = grpc_state();
    let storage = Arc::new(FakeStorageOps::new());
    state.storage_ops = storage.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = serve_grpc(listener, state);
    let mut client = GrpcClient::connect(&addr.to_string()).await.unwrap();

    // DAG run save/load.
    let saved = client
        .state_save_dag_run(&WireSaveDagRunRequest {
            session_id: "sess-1".into(),
            run_id: "dag-1".into(),
            snapshot: r#"{"id":"dag-1"}"#.into(),
        })
        .await
        .unwrap();
    assert!(saved.saved);
    let loaded = client
        .state_load_dag_runs(&WireLoadDagRunsRequest {
            session_id: "sess-1".into(),
            run_id: None,
        })
        .await
        .unwrap();
    assert_eq!(loaded.runs.len(), 1);
    assert_eq!(loaded.runs[0].run_id, "dag-1");

    // Trigger rules save/load.
    let saved = client
        .state_save_trigger_rules(&WireSaveTriggerRulesRequest {
            session_id: "sess-1".into(),
            rules: vec![WireStoredTriggerRule {
                id: "tr-1".into(),
                condition: "file changes".into(),
                action: "run tests".into(),
                enabled: true,
                fire_once: false,
                fired_at: None,
                promote_to_chat: true,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(saved.count, 1);
    let loaded = client
        .state_load_trigger_rules(&WireLoadTriggerRulesRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap();
    assert_eq!(loaded.rules.len(), 1);
    assert_eq!(loaded.rules[0].id, "tr-1");

    // Cron jobs save/load.
    let saved = client
        .state_save_cron_jobs(&WireSaveCronJobsRequest {
            session_id: "sess-1".into(),
            jobs: vec![WireStoredCronJob {
                id: "cron-1".into(),
                schedule: "*/5 * * * *".into(),
                action: "backup".into(),
                enabled: true,
                running_trace_id: None,
                last_due_at: None,
                last_fired_at: None,
                last_completed_at: None,
                last_error: None,
                skipped_overlap_count: 0,
                stateful: false,
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
        })
        .await
        .unwrap();
    assert_eq!(saved.count, 1);
    let loaded = client
        .state_load_cron_jobs(&WireLoadCronJobsRequest {
            session_id: "sess-1".into(),
        })
        .await
        .unwrap();
    assert_eq!(loaded.jobs.len(), 1);
    assert_eq!(loaded.jobs[0].id, "cron-1");
}
