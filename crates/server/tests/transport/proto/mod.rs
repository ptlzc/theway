//! Tests for `proto` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use crate::model_picker::{ModelEntry, ProviderGroup};
use crate::ui::feed::{Level, TriggerPollStatus};
use crate::wire::{
    WebCronSnapshot, WebMcpSnapshot, WebSidebarSnapshot, WebSkillsSnapshot, WebToolsSnapshot,
    WebTriggersSnapshot,
};

fn fixture_snapshot() -> WebStatus {
    WebStatus {
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
        busy: true,
        queued_count: 2,
        latest_trigger_poll: Some(TriggerPollStatus {
            checked_at: "t0".into(),
            trace_id: "tr-1".into(),
            source_label: "src".into(),
            event_label: "evt".into(),
            summary: "ok".into(),
        }),
        goal: None,
        control_plane_prompt: None,
        sidebar: WebSidebarSnapshot {
            inbox_new: 1,
            skills: WebSkillsSnapshot {
                total: 2,
                enabled: 1,
                disabled: 1,
                builtin: 1,
                user: 1,
                project: 0,
                items: Vec::new(),
            },
            triggers: WebTriggersSnapshot {
                total: 0,
                enabled: 0,
                disabled: 0,
                rules: Vec::new(),
            },
            cron: WebCronSnapshot {
                total: 0,
                enabled: 0,
                disabled: 0,
                jobs: Vec::new(),
            },
            mcp: WebMcpSnapshot {
                servers: 0,
                tools: 0,
                notification_hooks: 0,
                server_names: Vec::new(),
                tool_names: Vec::new(),
            },
            tools: WebToolsSnapshot {
                total: 1,
                names: vec!["read".into()],
            },
            hooks: Vec::new(),
            runtime: vec!["ok".into()],
        },
        feed_blocks: vec![
            WebFeedBlock::User {
                text: "hi".into(),
                timestamp: None,
            },
            WebFeedBlock::Plain {
                text: "note".into(),
                level: Level::Note,
                timestamp: Some("ts".into()),
            },
        ],
        feed_lines: vec!["line".into()],
        dags: Vec::new(),
        subagents: Vec::new(),
    }
}

#[test]
fn converts_full_snapshot_to_session_state() {
    let state = session_state(&fixture_snapshot());
    assert_eq!(state.session_id, "sess-1");
    assert_eq!(state.model, "provider:model");
    assert_eq!(state.cwd, "/tmp/theway");
    assert!(state.busy);
    assert_eq!(state.queued_count, 2);
    assert_eq!(state.model_catalog.len(), 1);
    assert_eq!(state.model_catalog[0].provider, "anthropic");
    assert_eq!(state.model_catalog[0].models[0].id, "claude-x");
    let poll = state.latest_trigger_poll.as_ref().unwrap();
    assert_eq!(poll.trace_id, "tr-1");
    assert_eq!(poll.summary, "ok");
    let sidebar = state.sidebar.as_ref().unwrap();
    assert_eq!(sidebar.inbox_new, 1);
    assert_eq!(sidebar.skills.as_ref().unwrap().total, 2);
    assert_eq!(sidebar.tools.as_ref().unwrap().names, vec!["read"]);
    assert_eq!(sidebar.runtime, vec!["ok"]);
    assert_eq!(state.feed_blocks.len(), 2);
    let user = state.feed_blocks[0].kind.as_ref().unwrap();
    match user {
        wire::feed_block::Kind::User(b) => assert_eq!(b.text, "hi"),
        other => panic!("expected user block, got {other:?}"),
    }
    let plain = state.feed_blocks[1].kind.as_ref().unwrap();
    match plain {
        wire::feed_block::Kind::Plain(b) => {
            assert_eq!(b.text, "note");
            assert_eq!(b.level, "note");
            assert_eq!(b.timestamp.as_deref(), Some("ts"));
        }
        other => panic!("expected plain block, got {other:?}"),
    }
    assert_eq!(state.feed_lines, vec!["line"]);
    // graph mode planes are empty until P1/P2.
    assert!(state.dags.is_empty());
    assert!(state.subagents.is_empty());
}

#[test]
fn dag_run_converts_to_wire_shape() {
    use theway_core::runtime::graph_engineering::types::{
        DagNode, DagRun, DagStatus, Direction, NodeStatus, RunKind,
    };

    let run = DagRun {
        id: "dag-1".into(),
        name: "test-run".into(),
        nodes: vec![DagNode {
            id: "impl-a".into(),
            agent: "executor-coder".into(),
            task: "do the thing".into(),
            depends_on: vec!["explore".into()],
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            status: NodeStatus::Running,
            job_id: Some("job-7".into()),
            attempt: 1,
            started_at: Some(1000),
            completed_at: None,
            error: None,
            input_tokens: Some(120),
            output_tokens: Some(80),
            result: None,
            output: Some("partial output".into()),
            live_preview: Some("live".into()),
            last_active_at: None,
        }],
        status: DagStatus::Running,
        kind: RunKind::Dag,
        max_concurrency: 4,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 999,
        session_id: Some("sess-1".into()),
        completed_at: None,
        last_activity_at: 1000,
        error: None,
    };

    let web = crate::wire::WebStatus::from_dag_run(&run);
    assert_eq!(web.id, "dag-1");
    assert_eq!(web.kind, "dag");
    assert_eq!(web.status, "running");
    assert_eq!(web.direction, "TD");
    assert_eq!(web.nodes.len(), 1);
    let node = &web.nodes[0];
    assert_eq!(node.status, "running");
    assert_eq!(node.job_id.as_deref(), Some("job-7"));
    assert_eq!(node.attempt, 1);
    assert_eq!(node.output_tail.as_deref(), Some("partial output"));
    assert_eq!(node.live_preview.as_deref(), Some("live"));
    assert_eq!(node.input_tokens, Some(120));
    // task text stays off the wire model.
    assert_eq!(node.depends_on, vec!["explore"]);

    let state = session_state(&{
        let mut snap = fixture_snapshot();
        snap.dags = vec![web];
        snap
    });
    assert_eq!(state.dags.len(), 1);
    let w = &state.dags[0];
    assert_eq!(w.name, "test-run");
    assert_eq!(w.kind, "dag");
    assert_eq!(w.max_concurrency, 4);
    assert_eq!(w.status, "running");
    assert_eq!(w.nodes.len(), 1);
    assert_eq!(w.nodes[0].agent, "executor-coder");
    assert_eq!(w.nodes[0].job_id.as_deref(), Some("job-7"));
    assert_eq!(w.nodes[0].output_tail.as_deref(), Some("partial output"));
    assert_eq!(w.nodes[0].input_tokens, Some(120));
}

#[test]
fn session_summary_converts_to_wire_shape() {
    let summary = crate::wire::SessionSummary {
        session_id: "sess-1".into(),
        name: "main".into(),
        cwd: "/tmp/theway".into(),
        model: "provider:model".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_activity_at: 1234,
        graph_count: 3,
        active_graph_count: 1,
        busy: true,
        preview: Some("last prompt".into()),
    };
    let w = session_summary_wire(&summary);
    assert_eq!(w.session_id, "sess-1");
    assert_eq!(w.name, "main");
    assert_eq!(w.cwd, "/tmp/theway");
    assert_eq!(w.model, "provider:model");
    assert_eq!(w.created_at, "2026-08-01T00:00:00Z");
    assert_eq!(w.last_activity_at, 1234);
    assert_eq!(w.graph_count, 3);
    assert_eq!(w.active_graph_count, 1);
    assert!(w.busy);
    assert_eq!(w.preview.as_deref(), Some("last prompt"));

    // preview stays optional on both sides.
    let mut no_preview = summary;
    no_preview.preview = None;
    assert!(session_summary_wire(&no_preview).preview.is_none());
}

#[test]
fn goal_run_round_trips_kind_and_dag_event_wire() {
    use theway_core::runtime::graph_engineering::engine::DagEngine;

    let engine = DagEngine::new();
    let id = engine.plan_goal("finish the migration", Some("sess-1".into()));
    let run = engine.get_run(&id).expect("goal run");
    let web = crate::wire::WebStatus::from_dag_run(&run);
    assert_eq!(web.kind, "goal");
    let state = session_state(&{
        let mut snap = fixture_snapshot();
        snap.dags = vec![web];
        snap
    });
    assert_eq!(state.dags[0].kind, "goal");

    // Engine event → wire: run_status (running) + node_status.
    let event = dag_event_wire(&DagEvent::RunStatus {
        run_id: id.clone(),
        session_id: String::new(),
        status: theway_core::runtime::graph_engineering::types::DagStatus::Running,
        error: None,
    });
    match event.kind {
        Some(wire::stream_event::Kind::RunStatus(run)) => {
            assert_eq!(run.run_id, id);
            assert_eq!(run.status, "running");
            assert!(run.error.is_none());
        }
        other => panic!("expected RunStatus, got {other:?}"),
    }
    let event = dag_event_wire(&DagEvent::NodeStatus {
        run_id: id.clone(),
        session_id: String::new(),
        node_id: "main".into(),
        status: theway_core::runtime::graph_engineering::types::NodeStatus::Running,
        error: Some("not yet".into()),
    });
    match event.kind {
        Some(wire::stream_event::Kind::NodeStatus(node)) => {
            assert_eq!(node.run_id, id);
            assert_eq!(node.node_id, "main");
            assert_eq!(node.status, "running");
            assert_eq!(node.error.as_deref(), Some("not yet"));
        }
        other => panic!("expected NodeStatus, got {other:?}"),
    }
}
