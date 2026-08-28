//! Tests for `proto` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::wire::{
    ModelEntry, ProviderGroup, WireAgentEvent, WireContextUsage, WireDagEvent, WireDagNodeSnapshot,
    WireDagRunSnapshot,
};

mod coverage;
mod extensions;
mod session_activation;
mod session_cumulative_usage;
use crate::feed::{Level, TriggerPollStatus};
use crate::wire::{
    WireCronSnapshot, WireMcpSnapshot, WireSidebarSnapshot, WireSkillsSnapshot, WireToolsSnapshot,
    WireTriggersSnapshot,
};

fn fixture_snapshot() -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
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
        sidebar: WireSidebarSnapshot {
            inbox_new: 1,
            skills: WireSkillsSnapshot {
                total: 2,
                enabled: 1,
                disabled: 1,
                builtin: 1,
                user: 1,
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
                total: 1,
                names: vec!["read".into()],
            },
            hooks: Vec::new(),
            runtime: vec!["ok".into()],
            commands: vec!["/commit".into(), "/review".into()],
            runtime_revision: 0,
        },
        feed_blocks: vec![
            WireFeedBlock::User {
                text: "hi".into(),
                timestamp: None,
            },
            WireFeedBlock::Plain {
                text: "note".into(),
                level: Level::Note,
                timestamp: Some("ts".into()),
            },
        ],
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec!["line".into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot::default(),
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
fn incremental_feed_block_patches_round_trip_through_proto() {
    let authoritative = fixture_snapshot();
    let mut delta_status = fixture_snapshot();
    delta_status.feed_blocks.clear();
    delta_status.feed_blocks_base = 2;
    delta_status.feed_block_patches = vec![crate::wire::WireFeedBlockPatch {
        index: 1,
        block: WireFeedBlock::Thinking {
            text: "summary".into(),
            timestamp: Some("10:00".into()),
        },
    }];
    let update = crate::wire::WireStatusUpdate::delta_from_status(delta_status, 2, 1);
    let delta = update.feed_delta().unwrap();

    let proto = incremental_session_state(&authoritative, delta, 0);
    assert_eq!(proto.feed_blocks_base, 2);
    assert!(proto.feed_blocks.is_empty());
    assert_eq!(proto.feed_block_patches.len(), 1);
    assert_eq!(proto.feed_block_patches[0].index, 1);

    let restored = wire_status(&proto);
    assert_eq!(restored.feed_blocks_base, 2);
    assert_eq!(restored.feed_block_patches, delta.feed_block_patches);
}

#[test]
fn dag_run_wire_shape_maps_to_proto() {
    let web = WireDagRunSnapshot {
        id: "dag-1".into(),
        name: "test-run".into(),
        kind: "dag".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 4,
        direction: "TD".into(),
        created_at: 999,
        completed_at: None,
        error: None,
        nodes: vec![WireDagNodeSnapshot {
            id: "impl-a".into(),
            agent: "executor-coder".into(),
            status: "running".into(),
            depends_on: vec!["explore".into()],
            job_id: Some("job-7".into()),
            attempt: 1,
            started_at: Some(1000),
            completed_at: None,
            error: None,
            input_tokens: Some(120),
            output_tokens: Some(80),
            result: None,
            output_tail: Some("partial output".into()),
            live_preview: Some("live".into()),
        }],
    };

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
        metadata: std::collections::HashMap::new(),
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
fn path_context_round_trips_wire_and_proto() {
    use crate::wire::WirePathContext;

    let ctx = WirePathContext {
        home: "/home/user".into(),
        base: "/home/user/.theway".into(),
        work_dir: "/home/user/projects/theway".into(),
        skills_dirs: vec![
            "/home/user/.agents/skills".into(),
            "/tmp/extra-skills".into(),
        ],
    };
    let proto = wire_path_context_to_proto(&ctx);
    assert_eq!(proto.home, "/home/user");
    assert_eq!(proto.base, "/home/user/.theway");
    assert_eq!(proto.work_dir, "/home/user/projects/theway");
    assert_eq!(
        proto.skills_dirs,
        vec!["/home/user/.agents/skills", "/tmp/extra-skills"]
    );
    assert_eq!(wire_path_context_from_proto(&proto), ctx);

    // Default (all-empty) context round-trips too.
    let empty = WirePathContext::default();
    let proto_empty = wire_path_context_to_proto(&empty);
    assert!(proto_empty.skills_dirs.is_empty());
    assert_eq!(wire_path_context_from_proto(&proto_empty), empty);
}

#[test]
fn goal_run_round_trips_kind_and_dag_event_wire() {
    let id = "goal-1".to_string();
    let web = WireDagRunSnapshot {
        id: id.clone(),
        name: "finish the migration".into(),
        kind: "goal".into(),
        status: "running".into(),
        fail_fast: false,
        max_concurrency: 1,
        direction: "TD".into(),
        created_at: 1,
        completed_at: None,
        error: None,
        nodes: Vec::new(),
    };
    assert_eq!(web.kind, "goal");
    let state = session_state(&{
        let mut snap = fixture_snapshot();
        snap.dags = vec![web];
        snap
    });
    assert_eq!(state.dags[0].kind, "goal");

    let event = dag_event_wire(&WireDagEvent::RunStatus {
        run_id: id.clone(),
        session_id: "sess-1".into(),
        status: "running".into(),
        error: None,
    });
    assert_eq!(event.session_id, "sess-1");
    match event.kind {
        Some(wire::stream_event::Kind::RunStatus(run)) => {
            assert_eq!(run.run_id, id);
            assert_eq!(run.status, "running");
            assert!(run.error.is_none());
        }
        other => panic!("expected RunStatus, got {other:?}"),
    }
    let event = dag_event_wire(&WireDagEvent::NodeStatus {
        run_id: id.clone(),
        session_id: "sess-1".into(),
        node_id: "main".into(),
        status: "running".into(),
        error: Some("not yet".into()),
    });
    assert_eq!(event.session_id, "sess-1");
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

#[test]
fn stream_event_wire_carries_session_ownership() {
    let events = [
        WireAgentEvent::Started {
            id: "job-1".into(),
            agent: "researcher".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: "sess-1".into(),
        },
        WireAgentEvent::Output {
            id: "job-1".into(),
            chunk: "hi".into(),
            session_id: "sess-1".into(),
        },
        WireAgentEvent::Metrics {
            id: "job-1".into(),
            tps: Some(12.5),
            cps: None,
            chars: 100,
            tokens_in: 20,
            tokens_out: 30,
            tools_called: 2,
            turn: 1,
            session_id: "sess-1".into(),
        },
        WireAgentEvent::Completed {
            id: "job-1".into(),
            status: "succeeded".into(),
            error: None,
            chars: 100,
            tokens_in: 20,
            tokens_out: 30,
            tools_called: 2,
            session_id: "sess-1".into(),
        },
    ];

    for event in events {
        let wire_event = stream_event_wire(&event);
        assert_eq!(wire_event.session_id, "sess-1");
    }
}

// ── settings / config (issue #72) ─────────────────────────────────────

#[test]
fn daemon_config_round_trips_wire_and_proto() {
    use crate::wire::WireDaemonConfig;

    let config = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        base_url: Some("https://api.example.com".into()),
        thinking: Some(true),
        thinking_level: Some("high".into()),
        builtin_skills: vec!["git".into(), "web".into()],
        skills_dirs: vec!["/home/user/.agents/skills".into()],
        trigger_poll_secs: Some(60),
        tui_max_feed_lines: Some(8000),
        tool_service_addr: None,
        storage_service_addr: None,
        clear_fields: vec!["tool_service_addr".into()],
    };
    let proto = daemon_config_to_proto(&config);
    assert_eq!(proto.provider.as_deref(), Some("anthropic"));
    assert_eq!(proto.model.as_deref(), Some("claude-x"));
    assert_eq!(proto.base_url.as_deref(), Some("https://api.example.com"));
    assert_eq!(proto.thinking, Some(true));
    assert_eq!(proto.builtin_skills, vec!["git", "web"]);
    assert_eq!(proto.skills_dirs, vec!["/home/user/.agents/skills"]);
    assert_eq!(proto.trigger_poll_secs, Some(60));
    assert_eq!(proto.tui_max_feed_lines, Some(8000));
    assert_eq!(proto.thinking_level.as_deref(), Some("high"));
    assert_eq!(proto.clear_fields, vec!["tool_service_addr"]);
    assert_eq!(daemon_config_from_proto(&proto), config);

    // Default (all-absent) config round-trips too: no field gains presence.
    let empty = WireDaemonConfig::default();
    let proto_empty = daemon_config_to_proto(&empty);
    assert!(proto_empty.provider.is_none());
    assert!(proto_empty.model.is_none());
    assert!(proto_empty.base_url.is_none());
    assert!(proto_empty.thinking.is_none());
    assert!(proto_empty.thinking_level.is_none());
    assert!(proto_empty.builtin_skills.is_empty());
    assert!(proto_empty.skills_dirs.is_empty());
    assert!(proto_empty.trigger_poll_secs.is_none());
    assert!(proto_empty.tui_max_feed_lines.is_none());
    assert_eq!(daemon_config_from_proto(&proto_empty), empty);
}

#[test]
fn daemon_config_merge_replaces_present_fields_only() {
    use crate::wire::WireDaemonConfig;

    let mut current = WireDaemonConfig {
        provider: Some("anthropic".into()),
        model: Some("claude-x".into()),
        skills_dirs: vec!["/old".into()],
        ..Default::default()
 };
    let patch = WireDaemonConfig {
        model: Some("claude-y".into()),
        trigger_poll_secs: Some(30),
        ..Default::default()
 };
    let touched = current.merge_from(&patch);

    // Present fields replaced, absent ones kept; repeated-empty does not clear.
    assert_eq!(touched, 2);
    assert_eq!(current.provider.as_deref(), Some("anthropic"));
    assert_eq!(current.model.as_deref(), Some("claude-y"));
    assert_eq!(current.skills_dirs, vec!["/old"]);
    assert_eq!(current.trigger_poll_secs, Some(30));

    // Non-empty repeated fields replace the list.
    let dirs = WireDaemonConfig {
        skills_dirs: vec!["/new".into()],
        ..Default::default()
 };
    let touched = current.merge_from(&dirs);
    assert_eq!(touched, 1);
    assert_eq!(current.skills_dirs, vec!["/new"]);

    // Empty patch touches nothing.
    assert_eq!(current.merge_from(&WireDaemonConfig::default()), 0);
}

#[test]
fn daemon_config_merge_supports_explicit_clear_and_set_wins() {
    use crate::wire::WireDaemonConfig;

    let mut current = WireDaemonConfig {
        thinking: Some(true),
        skills_dirs: vec!["/old".into()],
        tool_service_addr: Some("http://old".into()),
        ..Default::default()
    };
    let patch = WireDaemonConfig {
        thinking: Some(false),
        clear_fields: vec![
            "thinking".into(),
            "skills_dirs".into(),
            "tool_service_addr".into(),
        ],
        ..Default::default()
    };

    current.merge_from(&patch);

    assert_eq!(current.thinking, Some(false), "set wins over clear");
    assert!(current.skills_dirs.is_empty());
    assert!(current.tool_service_addr.is_none());
    assert!(current.clear_fields.is_empty(), "snapshots never retain patch intent");
}

#[test]
fn daemon_config_reports_unknown_clear_fields() {
    use crate::wire::WireDaemonConfig;

    let config = WireDaemonConfig {
        clear_fields: vec!["skills_dirs".into(), "typo".into()],
        ..Default::default()
    };

    assert_eq!(config.unknown_clear_fields(), vec!["typo"]);
}
