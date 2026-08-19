//! Tests for `turn/relay` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use theway_transport::feed::{Level, TriggerPollStatus, WireFeedBlock};
use theway_transport::wire::{
    WireContextUsage, WireCronJobSnapshot, WireCronSnapshot, WireMcpSnapshot, WireSidebarSnapshot,
    WireSkillSnapshot, WireSkillsSnapshot, WireStatus, WireToolsSnapshot, WireTriggerRuleSnapshot,
    WireTriggersSnapshot,
};

use super::*;

// ── pure helpers ───────────────────────────────────────────────────────────────

#[test]
fn tokens_are_long_random_and_url_safe() {
    let a = new_token();
    let b = new_token();
    assert_ne!(a, b, "tokens must be random");
    assert_eq!(a.len(), 40, "{a}");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "token must be URL-safe: {a}"
    );
}

#[test]
fn ws_url_derives_scheme_and_path_from_base() {
    assert_eq!(
        agent_ws_url("https://pie.0xfefe.me", "tok123").unwrap(),
        "wss://pie.0xfefe.me/relay/agent?token=tok123"
    );
    assert_eq!(
        agent_ws_url("http://127.0.0.1:8787/", "tok123").unwrap(),
        "ws://127.0.0.1:8787/relay/agent?token=tok123"
    );
    assert!(agent_ws_url("ftp://nope", "t").is_err());
}

#[test]
fn viewer_url_is_session_path_with_trailing_slash() {
    // The trailing slash matters: the shared HTML uses relative fetch paths, so
    // /session/<token> (no slash) would resolve them against /session/.
    assert_eq!(
        viewer_url("https://pie.0xfefe.me", "tok123"),
        "https://pie.0xfefe.me/session/tok123/"
    );
    assert_eq!(
        viewer_url("http://127.0.0.1:8787/", "tok123"),
        "http://127.0.0.1:8787/session/tok123/"
    );
}

#[test]
fn qr_lines_render_a_scannable_block_grid() {
    let lines = qr_lines("https://pie.0xfefe.me/session/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/")
        .expect("urls of this shape must encode");
    assert!(
        lines.len() > 10,
        "expected a QR-sized grid, got {}",
        lines.len()
    );
    let width = lines[0].chars().count();
    assert!(width > 10);
    assert!(
        lines.iter().all(|l| l.chars().count() == width),
        "all QR lines must be equal width"
    );
    let blocks: usize = lines
        .iter()
        .map(|l| l.chars().filter(|c| "█▀▄".contains(*c)).count())
        .sum();
    assert!(blocks > 50, "expected block characters, got {blocks}");
}

#[test]
fn frames_round_trip_as_tagged_json() {
    let hello = serde_json::to_string(&AgentFrame::Hello {
        agent_key: "k".into(),
    })
    .unwrap();
    assert!(hello.contains("\"type\":\"hello\""), "{hello}");

    let prompt: WorkerFrame = serde_json::from_str(r#"{"type":"prompt","text":"hi"}"#).unwrap();
    assert_eq!(prompt, WorkerFrame::Prompt { text: "hi".into() });
    let viewers: WorkerFrame = serde_json::from_str(r#"{"type":"viewers","count":3}"#).unwrap();
    assert_eq!(viewers, WorkerFrame::Viewers { count: 3 });
    let abort: WorkerFrame = serde_json::from_str(r#"{"type":"abort"}"#).unwrap();
    assert_eq!(abort, WorkerFrame::Abort);
    let resolve: WorkerFrame =
        serde_json::from_str(r#"{"type":"control_plane_resolve","approve":true}"#).unwrap();
    assert_eq!(resolve, WorkerFrame::ControlPlaneResolve { approve: true });
    let set_model: WorkerFrame =
        serde_json::from_str(r#"{"type":"set_model","model":"anthropic:claude-haiku-4-5"}"#)
            .unwrap();
    assert_eq!(
        set_model,
        WorkerFrame::SetModel {
            model: "anthropic:claude-haiku-4-5".into()
        }
    );
}

#[test]
fn start_rejects_non_http_base_url() {
    let (prompt_tx, _) = mpsc::unbounded_channel::<String>();
    let (abort_tx, _) = mpsc::unbounded_channel::<()>();
    let (resolve_tx, _) = mpsc::unbounded_channel::<bool>();
    let (model_tx, _) = mpsc::unbounded_channel::<String>();

    let err = start("ftp://nope", prompt_tx, abort_tx, resolve_tx, model_tx)
        .err()
        .expect("non-http base_url must fail");

    assert!(err.to_string().contains("base_url must be http(s)://"));
}

#[test]
fn relay_handle_status_line_reports_state_viewers_and_dropped() {
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connected,
        viewers: 7,
        dropped_snapshots: 2,
    }));
    let (_snapshot_tx, _) = mpsc::unbounded_channel::<WireStatus>();
    let handle = RelayHandle {
        url: "https://pie.0xfefe.me/session/tok/".into(),
        snapshot_tx: _snapshot_tx,
        cancel: CancellationToken::new(),
        shared,
    };

    let line = handle.status_line();

    assert!(line.contains("relay connected"));
    assert!(line.contains("viewers: 7"));
    assert!(line.contains("2 oversized snapshot(s) dropped"));

    handle.shutdown();
    assert!(handle.cancel.is_cancelled());
}

#[test]
fn relay_handle_status_line_covers_every_state_label() {
    let cases = [
        (RelayState::Connecting, "connecting"),
        (RelayState::Connected, "connected"),
        (RelayState::Reconnecting, "reconnecting"),
        (RelayState::Stopped, "stopped"),
    ];
    for (state, label) in cases {
        let shared = Arc::new(Mutex::new(RelayShared {
            state,
            viewers: 0,
            dropped_snapshots: 0,
        }));
        let (_snapshot_tx, _) = mpsc::unbounded_channel::<WireStatus>();
        let handle = RelayHandle {
            url: "https://pie.0xfefe.me/session/tok/".into(),
            snapshot_tx: _snapshot_tx,
            cancel: CancellationToken::new(),
            shared,
        };

        let line = handle.status_line();

        assert!(
            line.contains(&format!("relay {label}")),
            "expected {label:?} in {line}"
        );
    }
}

fn sample_wire_status(feed_lines: Vec<String>) -> WireStatus {
    WireStatus {
        session_id: "sess-relay".into(),
        model: "faux:faux".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: Some(TriggerPollStatus {
            checked_at: "now".into(),
            trace_id: "trace".into(),
            source_label: "source".into(),
            event_label: "event".into(),
            summary: "summary".into(),
        }),
        goal: None,
        control_plane_prompt: None,
        sidebar: WireSidebarSnapshot {
            inbox_new: 0,
            skills: WireSkillsSnapshot {
                total: 1,
                enabled: 1,
                disabled: 0,
                builtin: 0,
                user: 1,
                project: 0,
                items: vec![WireSkillSnapshot {
                    name: "skill".into(),
                    source: "user".into(),
                    file_path: "/skills/skill/SKILL.md".into(),
                    enabled: true,
                }],
            },
            triggers: WireTriggersSnapshot {
                total: 0,
                enabled: 0,
                disabled: 0,
                rules: Vec::<WireTriggerRuleSnapshot>::new(),
            },
            cron: WireCronSnapshot {
                total: 0,
                enabled: 0,
                disabled: 0,
                jobs: Vec::<WireCronJobSnapshot>::new(),
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
        },
        feed_blocks: vec![WireFeedBlock::Plain {
            text: "hello".into(),
            level: Level::System,
            timestamp: None,
        }],
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines,
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
    }
}

#[test]
fn snapshot_frame_serializes_and_drops_oversized_frames() {
    let small = sample_wire_status(vec!["line".into()]);
    let frame = snapshot_frame(&small).expect("small snapshot fits");
    assert!(frame.contains("\"type\":\"snapshot\""));
    assert!(frame.contains("line"));

    let oversized = sample_wire_status(vec!["x".repeat(MAX_SNAPSHOT_BYTES)]);
    assert!(snapshot_frame(&oversized).is_none());
}

// ── relay task against a local websocket server ────────────────────────────────

async fn wait_until(mut f: impl FnMut() -> bool, what: &str) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn relay_task_forwards_worker_frames_and_snapshots_until_shutdown() {
    // Arrange: local websocket server that plays the worker side of the relay.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}/relay/agent?token=tok");
    let (snapshot_read_tx, snapshot_read_rx) = oneshot::channel();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let hello = ws.next().await.unwrap().unwrap();
        let hello: AgentFrame = serde_json::from_str(hello.to_text().unwrap()).unwrap();
        assert_eq!(
            hello,
            AgentFrame::Hello {
                agent_key: "agent-key".into()
            }
        );

        ws.send(Message::text(r#"{"type":"prompt","text":"remote hi"}"#))
            .await
            .unwrap();
        ws.send(Message::text(r#"{"type":"abort"}"#)).await.unwrap();
        ws.send(Message::text(
            r#"{"type":"control_plane_resolve","approve":true}"#,
        ))
        .await
        .unwrap();
        ws.send(Message::text(
            r#"{"type":"set_model","model":"anthropic:claude"}"#,
        ))
        .await
        .unwrap();
        ws.send(Message::text(r#"{"type":"viewers","count":3}"#))
            .await
            .unwrap();

        // Snapshot frame arrives after the test pushes a WireStatus.
        let snap = ws.next().await.unwrap().unwrap();
        let snap: AgentFrame = serde_json::from_str(snap.to_text().unwrap()).unwrap();
        match snap {
            AgentFrame::Snapshot { data } => {
                assert_eq!(data["feed_lines"][0], "line-1");
            }
            other => panic!("expected snapshot frame, got {other:?}"),
        }
        snapshot_read_tx.send(()).unwrap();

        // Graceful shutdown frame after the test cancels.
        let bye = ws.next().await.unwrap().unwrap();
        let bye: AgentFrame = serde_json::from_str(bye.to_text().unwrap()).unwrap();
        assert_eq!(bye, AgentFrame::Shutdown);
    });

    let (prompt_tx, mut prompt_rx) = mpsc::unbounded_channel::<String>();
    let (abort_tx, mut abort_rx) = mpsc::unbounded_channel::<()>();
    let (resolve_tx, mut resolve_rx) = mpsc::unbounded_channel::<bool>();
    let (model_tx, mut model_rx) = mpsc::unbounded_channel::<String>();
    let (snapshot_tx, snapshot_rx) = mpsc::unbounded_channel::<WireStatus>();
    let cancel = CancellationToken::new();
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connecting,
        viewers: 0,
        dropped_snapshots: 0,
    }));

    // Act: run the relay task.
    let task = tokio::spawn(relay_task(
        ws_url,
        "agent-key".into(),
        snapshot_rx,
        prompt_tx,
        abort_tx,
        resolve_tx,
        model_tx,
        cancel.clone(),
        shared.clone(),
    ));

    // Assert: worker frames arrive on the matching app channels.
    assert_eq!(prompt_rx.recv().await, Some("remote hi".into()));
    assert_eq!(abort_rx.recv().await, Some(()));
    assert_eq!(resolve_rx.recv().await, Some(true));
    assert_eq!(model_rx.recv().await, Some("anthropic:claude".into()));
    wait_until(|| shared.lock().viewers == 3, "viewers count").await;
    assert_eq!(shared.lock().state, RelayState::Connected);

    // Act: push one snapshot through the channel.
    snapshot_tx
        .send(sample_wire_status(vec!["line-1".into()]))
        .unwrap();

    // Wait until the server has observed the snapshot frame, then shut down.
    tokio::time::timeout(Duration::from_secs(5), snapshot_read_rx)
        .await
        .expect("snapshot should reach the relay server")
        .unwrap();
    cancel.cancel();

    task.await.unwrap();
    server.await.unwrap();
    assert_eq!(shared.lock().state, RelayState::Stopped);
}

#[tokio::test]
async fn relay_task_marks_reconnecting_on_connect_error_and_stops_on_cancel() {
    // Arrange: a TCP listener that accepts and immediately drops the socket,
    // so the websocket handshake fails.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}/relay/agent?token=tok");
    let server = tokio::spawn(async move {
        let _ = listener.accept().await;
    });

    let (prompt_tx, _) = mpsc::unbounded_channel::<String>();
    let (abort_tx, _) = mpsc::unbounded_channel::<()>();
    let (resolve_tx, _) = mpsc::unbounded_channel::<bool>();
    let (model_tx, _) = mpsc::unbounded_channel::<String>();
    let (_snapshot_tx, snapshot_rx) = mpsc::unbounded_channel::<WireStatus>();
    let cancel = CancellationToken::new();
    let shared = Arc::new(Mutex::new(RelayShared {
        state: RelayState::Connecting,
        viewers: 0,
        dropped_snapshots: 0,
    }));

    // Act
    let task = tokio::spawn(relay_task(
        ws_url,
        "agent-key".into(),
        snapshot_rx,
        prompt_tx,
        abort_tx,
        resolve_tx,
        model_tx,
        cancel.clone(),
        shared.clone(),
    ));

    // Assert: connect error flips state to Reconnecting; cancel then exits.
    wait_until(
        || shared.lock().state == RelayState::Reconnecting,
        "reconnecting",
    )
    .await;
    cancel.cancel();
    task.await.unwrap();
    server.await.unwrap();
    assert_eq!(shared.lock().state, RelayState::Stopped);
}
