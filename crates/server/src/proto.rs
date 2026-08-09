//! `WebStatus` ↔ `SessionState` conversion (transport layer).
//!
//! `WebStatus` (serde, `theway::wire`) is the internal model shared by the
//! `--web` JSON surface and the UI event loop; `SessionState` (prost, generated
//! from `proto/theway_grpc.proto` by this crate's build.rs) is the structured
//! wire model for gRPC. The gRPC server serializes `SessionState` as binary
//! protobuf; JSON channels keep using `WebStatus` until the protojson migration
//! (see docs/PROTOCOL.md).

/// Generated protobuf code for `proto/theway_grpc.proto` (package
/// `theway.grpc.v1`), produced by this crate's `build.rs` into its own OUT_DIR.
pub mod theway_grpc {
    tonic::include_proto!("theway.grpc.v1");
}

/// Generated code for `proto/health.proto` (standard `grpc.health.v1` health
/// checking protocol), produced by this crate's build.rs.
pub mod health {
    tonic::include_proto!("grpc.health.v1");
}

use theway::ui::feed::{self, WebFeedBlock};
use theway::wire::WebStatus;
use theway_core::runtime::graph_engineering::types::DagEvent;
use theway_core::runtime::subagents::registry::SubagentEvent;
use theway_grpc as wire;

/// Convert the internal snapshot into the structured wire model.
pub fn session_state(snapshot: &WebStatus) -> wire::SessionState {
    wire::SessionState {
        session_id: snapshot.session_id.clone(),
        model: snapshot.model.clone(),
        model_catalog: snapshot
            .model_catalog
            .iter()
            .map(|group| wire::ProviderGroup {
                provider: group.provider.clone(),
                has_credential: group.has_credential,
                models: group
                    .models
                    .iter()
                    .map(|entry| wire::ModelEntry {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                    })
                    .collect(),
            })
            .collect(),
        cwd: snapshot.cwd.clone(),
        busy: snapshot.busy,
        queued_count: snapshot.queued_count as u32,
        latest_trigger_poll: snapshot.latest_trigger_poll.as_ref().map(|status| {
            wire::TriggerPollStatus {
                checked_at: status.checked_at.clone(),
                trace_id: status.trace_id.clone(),
                source_label: status.source_label.clone(),
                event_label: status.event_label.clone(),
                summary: status.summary.clone(),
            }
        }),
        goal: snapshot.goal.as_ref().map(|goal| wire::GoalSnapshot {
            condition: goal.condition.clone(),
            status: goal.status.clone(),
            iterations: goal.iterations,
            last_reason: goal.last_reason.clone(),
        }),
        control_plane_prompt: snapshot.control_plane_prompt.as_ref().map(|prompt| {
            wire::ControlPlanePromptSnapshot {
                tool_name: prompt.tool_name.clone(),
                label: prompt.label.clone(),
                reason: prompt.reason.clone(),
                args_hash: prompt.args_hash.clone(),
                payload: prompt.payload.clone(),
            }
        }),
        sidebar: Some(wire::SidebarSnapshot {
            inbox_new: snapshot.sidebar.inbox_new as u32,
            skills: Some(wire::SkillsSnapshot {
                total: snapshot.sidebar.skills.total as u32,
                enabled: snapshot.sidebar.skills.enabled as u32,
                disabled: snapshot.sidebar.skills.disabled as u32,
                builtin: snapshot.sidebar.skills.builtin as u32,
                user: snapshot.sidebar.skills.user as u32,
                project: snapshot.sidebar.skills.project as u32,
                items: snapshot
                    .sidebar
                    .skills
                    .items
                    .iter()
                    .map(|skill| wire::SkillSnapshot {
                        name: skill.name.clone(),
                        source: skill.source.clone(),
                        file_path: skill.file_path.clone(),
                        enabled: skill.enabled,
                    })
                    .collect(),
            }),
            triggers: Some(wire::TriggersSnapshot {
                total: snapshot.sidebar.triggers.total as u32,
                enabled: snapshot.sidebar.triggers.enabled as u32,
                disabled: snapshot.sidebar.triggers.disabled as u32,
                rules: snapshot
                    .sidebar
                    .triggers
                    .rules
                    .iter()
                    .map(|rule| wire::TriggerRuleSnapshot {
                        id: rule.id.clone(),
                        full_id: rule.full_id.clone(),
                        enabled: rule.enabled,
                        mode: rule.mode.clone(),
                        condition: rule.condition.clone(),
                        action: rule.action.clone(),
                    })
                    .collect(),
            }),
            cron: Some(wire::CronSnapshot {
                total: snapshot.sidebar.cron.total as u32,
                enabled: snapshot.sidebar.cron.enabled as u32,
                disabled: snapshot.sidebar.cron.disabled as u32,
                jobs: snapshot
                    .sidebar
                    .cron
                    .jobs
                    .iter()
                    .map(|job| wire::CronJobSnapshot {
                        id: job.id.clone(),
                        enabled: job.enabled,
                        schedule: job.schedule.clone(),
                        action: job.action.clone(),
                        skipped_overlap_count: job.skipped_overlap_count,
                        last_error: job.last_error.clone(),
                    })
                    .collect(),
            }),
            mcp: Some(wire::McpSnapshot {
                servers: snapshot.sidebar.mcp.servers as u32,
                tools: snapshot.sidebar.mcp.tools as u32,
                notification_hooks: snapshot.sidebar.mcp.notification_hooks as u32,
                server_names: snapshot.sidebar.mcp.server_names.clone(),
                tool_names: snapshot.sidebar.mcp.tool_names.clone(),
            }),
            tools: Some(wire::ToolsSnapshot {
                total: snapshot.sidebar.tools.total as u32,
                names: snapshot.sidebar.tools.names.clone(),
            }),
            hooks: snapshot.sidebar.hooks.clone(),
            runtime: snapshot.sidebar.runtime.clone(),
        }),
        feed_blocks: snapshot.feed_blocks.iter().map(feed_block).collect(),
        feed_lines: snapshot.feed_lines.clone(),
        dags: snapshot.dags.iter().map(dag_run_wire).collect(),
        subagents: snapshot.subagents.iter().map(subagent_wire).collect(),
    }
}

/// Convert the internal session summary (session-resource-model) into the
/// structured wire model.
pub fn session_summary_wire(summary: &theway::wire::SessionSummary) -> wire::SessionSummary {
    wire::SessionSummary {
        session_id: summary.session_id.clone(),
        name: summary.name.clone(),
        cwd: summary.cwd.clone(),
        model: summary.model.clone(),
        created_at: summary.created_at.clone(),
        last_activity_at: summary.last_activity_at,
        graph_count: summary.graph_count,
        active_graph_count: summary.active_graph_count,
        busy: summary.busy,
        preview: summary.preview.clone(),
    }
}

/// Resolve a session id argument (full id or unique prefix, same semantics as the
/// repo-backed `SessionOps` impls) against a session list. Returns the full id, or
/// `None` when nothing or more than one session matches.
pub(crate) fn resolve_session_id(
    sessions: &[theway::wire::SessionSummary],
    id: &str,
) -> Option<String> {
    if let Some(exact) = sessions.iter().find(|s| s.session_id == id) {
        return Some(exact.session_id.clone());
    }
    let mut matches = sessions
        .iter()
        .filter(|s| !id.is_empty() && s.session_id.starts_with(id));
    let first = matches.next()?.session_id.clone();
    matches.next().map_or(Some(first), |_| None)
}

/// Convert one DAG run snapshot into the wire form.
pub fn dag_run_wire(run: &theway::wire::WebDagRunSnapshot) -> wire::DagRunSnapshot {
    wire::DagRunSnapshot {
        id: run.id.clone(),
        name: run.name.clone(),
        kind: run.kind.clone(),
        status: run.status.clone(),
        fail_fast: run.fail_fast,
        max_concurrency: run.max_concurrency as u32,
        direction: run.direction.clone(),
        created_at: run.created_at,
        completed_at: run.completed_at,
        error: run.error.clone(),
        nodes: run.nodes.iter().map(dag_node_wire).collect(),
    }
}

fn dag_node_wire(node: &theway::wire::WebDagNodeSnapshot) -> wire::DagNodeSnapshot {
    wire::DagNodeSnapshot {
        id: node.id.clone(),
        agent: node.agent.clone(),
        status: node.status.clone(),
        depends_on: node.depends_on.clone(),
        job_id: node.job_id.clone(),
        attempt: node.attempt,
        started_at: node.started_at,
        completed_at: node.completed_at,
        error: node.error.clone(),
        input_tokens: node.input_tokens,
        output_tokens: node.output_tokens,
        result: node.result.as_ref().map(|result| wire::NodeResultSnapshot {
            success: result.success,
            error: result.error.clone(),
            duration_ms: result.duration_ms,
            attempt: result.attempt,
            total_attempts: result.total_attempts,
        }),
        output_tail: node.output_tail.clone(),
        live_preview: node.live_preview.clone(),
    }
}

/// Convert an event-plane message into the wire `StreamEvent`.
pub fn stream_event_wire(event: &SubagentEvent) -> wire::StreamEvent {
    use wire::stream_event::Kind;
    let kind = match event {
        SubagentEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
        } => Kind::SubagentStarted(wire::SubagentStarted {
            id: id.clone(),
            agent: agent.clone(),
            source: source.clone(),
            run_id: run_id.clone(),
            node_id: node_id.clone(),
        }),
        SubagentEvent::Output { id, chunk } => Kind::SubagentOutput(wire::SubagentOutput {
            id: id.clone(),
            chunk: chunk.clone(),
        }),
        SubagentEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
        } => Kind::SubagentMetrics(wire::SubagentMetrics {
            id: id.clone(),
            tps: tps.unwrap_or(0.0),
            cps: cps.unwrap_or(0.0),
            chars: *chars,
            tokens_in: *tokens_in,
            tokens_out: *tokens_out,
            tools_called: *tools_called,
            turn: *turn,
        }),
        SubagentEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
        } => Kind::SubagentCompleted(wire::SubagentCompleted {
            id: id.clone(),
            status: status.as_str().to_string(),
            error: error.clone(),
            duration_ms: None,
            chars: *chars,
            tokens_in: *tokens_in,
            tokens_out: *tokens_out,
            tools_called: *tools_called,
        }),
    };
    wire::StreamEvent { kind: Some(kind) }
}

/// Convert a DAG engine event-plane message (node_status / run_status) into
/// the wire `StreamEvent`.
pub fn dag_event_wire(event: &DagEvent) -> wire::StreamEvent {
    use theway::wire::{dag_status_str, node_status_str};
    use wire::stream_event::Kind;
    let kind = match event {
        DagEvent::NodeStatus {
            run_id,
            node_id,
            status,
            error,
            .. // `session_id` has no wire field yet (proto change pending).
        } => Kind::NodeStatus(wire::NodeStatus {
            run_id: run_id.clone(),
            node_id: node_id.clone(),
            status: node_status_str(status).to_string(),
            error: error.clone(),
        }),
        DagEvent::RunStatus {
            run_id,
            status,
            error,
            .. // `session_id` has no wire field yet (proto change pending).
        } => Kind::RunStatus(wire::RunStatus {
            run_id: run_id.clone(),
            status: dag_status_str(status).to_string(),
            error: error.clone(),
        }),
    };
    wire::StreamEvent { kind: Some(kind) }
}

fn subagent_wire(job: &theway::wire::WebSubagentJobSnapshot) -> wire::SubagentJobSnapshot {
    wire::SubagentJobSnapshot {
        id: job.id.clone(),
        agent: job.agent.clone(),
        source: job.source.clone(),
        run_id: job.run_id.clone(),
        node_id: job.node_id.clone(),
        status: job.status.clone(),
        started_at: job.started_at,
        completed_at: job.completed_at,
        duration_ms: job.duration_ms,
        attempt: job.attempt,
        total_attempts: job.total_attempts,
        input_tokens: job.input_tokens,
        output_tokens: job.output_tokens,
        error: job.error.clone(),
        output_tail: job.output_tail.clone(),
        live_preview: job.live_preview.clone(),
        tps: job.tps,
        cps: job.cps,
        chars: job.chars,
        tools_called: job.tools_called,
        turn: job.turn,
    }
}

/// Convert one serde-tagged feed block into the proto oneof form.
fn feed_block(block: &WebFeedBlock) -> wire::FeedBlock {
    use wire::feed_block::Kind;
    let kind = match block {
        WebFeedBlock::User { text, timestamp } => Kind::User(wire::UserBlock {
            text: text.clone(),
            timestamp: timestamp.clone(),
        }),
        WebFeedBlock::Assistant { text, timestamp } => Kind::Assistant(wire::AssistantBlock {
            text: text.clone(),
            timestamp: timestamp.clone(),
        }),
        WebFeedBlock::Thinking { text, timestamp } => Kind::Thinking(wire::ThinkingBlock {
            text: text.clone(),
            timestamp: timestamp.clone(),
        }),
        WebFeedBlock::Tool {
            name,
            args,
            timestamp,
        } => Kind::Tool(wire::ToolBlock {
            name: name.clone(),
            args: args.clone(),
            timestamp: timestamp.clone(),
        }),
        WebFeedBlock::ToolResult {
            lines,
            is_error,
            timestamp,
        } => Kind::ToolResult(wire::ToolResultBlock {
            lines: lines.clone(),
            is_error: *is_error,
            timestamp: timestamp.clone(),
        }),
        WebFeedBlock::Plain {
            text,
            level,
            timestamp,
        } => Kind::Plain(wire::PlainBlock {
            text: text.clone(),
            level: level_str(level).to_string(),
            timestamp: timestamp.clone(),
        }),
    };
    wire::FeedBlock { kind: Some(kind) }
}

/// `feed::Level` serializes as snake_case variant names on the JSON surface.
fn level_str(level: &feed::Level) -> &'static str {
    match level {
        feed::Level::Output => "output",
        feed::Level::System => "system",
        feed::Level::Error => "error",
        feed::Level::Note => "note",
        feed::Level::Header => "header",
        feed::Level::Qr => "qr",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway::model_picker::{ModelEntry, ProviderGroup};
    use theway::ui::feed::{Level, TriggerPollStatus};
    use theway::wire::{
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

        let web = theway::wire::WebStatus::from_dag_run(&run);
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
        let summary = theway::wire::SessionSummary {
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
        let web = theway::wire::WebStatus::from_dag_run(&run);
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
}
