//! `WireStatus` ↔ `SessionState` conversion (transport layer).
//!
//! `WireStatus` (serde, `crate::wire`) is the internal model shared by the
//! `--http` JSON surface and the UI event loop; `SessionState` (prost, generated
//! from the seven domain proto files — `commands.proto`, `session.proto`,
//! `graph_engine.proto`, `events.proto`, `settings.proto`, `tools.proto`,
//! `state.proto` — plus `health.proto` by this crate's build.rs) is the
//! structured wire model for gRPC. The gRPC server serializes `SessionState`
//! as binary protobuf; JSON channels keep using `WireStatus` until the
//! protojson migration (see docs/PROTOCOL.md). The tool-operation
//! (`tools.proto`) codecs live in [`crate::tools`]; the runtime-state
//! (`state.proto`) codecs live in [`crate::state`].

/// Generated protobuf code for package `theway.grpc.v1`, produced by this
/// crate's `build.rs` into its own OUT_DIR. Each domain proto file carries its
/// messages, enums, and service in the same package: `commands.proto` /
/// `session.proto` / `graph_engine.proto` / `events.proto` / `settings.proto`
/// / `tools.proto` / `state.proto`.
pub mod theway_grpc {
    tonic::include_proto!("theway.grpc.v1");
}

/// Generated code for `crates/theway-transport/proto/health.proto` (standard
/// `grpc.health.v1` health checking protocol), produced by this crate's build.rs.
pub mod health {
    tonic::include_proto!("grpc.health.v1");
}

use crate::feed::{self, WireFeedBlock};
use crate::wire::{
    WireAgentEvent, WireDagEvent, WireExtensionCatalogEntry, WireExtensionCommandDescriptor,
    WireExtensionContribution, WireExtensionDiagnostic, WireExtensionSnapshot,
};
use crate::wire::{WireFeedDelta, WirePathContext, WireStatus};
use theway_grpc as wire;

/// Convert the internal snapshot into the structured wire model.
pub fn session_state(snapshot: &WireStatus) -> wire::SessionState {
    session_state_with_feed(
        snapshot,
        &snapshot.feed_blocks,
        &[],
        &snapshot.feed_lines,
        0,
        0,
    )
}

/// Project an authoritative snapshot into a per-subscriber incremental frame.
/// Non-feed fields remain complete; transcript fields contain only the rows
/// and block patches after that subscriber's cursors.
pub(crate) fn incremental_session_state(
    snapshot: &WireStatus,
    delta: &WireFeedDelta,
    feed_lines_start: usize,
) -> wire::SessionState {
    let feed_lines_base = delta.feed_lines_base as usize;
    let suffix_start = feed_lines_start.saturating_sub(feed_lines_base);
    session_state_with_feed(
        snapshot,
        &[],
        &delta.feed_block_patches,
        &delta.feed_lines[suffix_start..],
        delta.feed_blocks_base,
        feed_lines_start as u64,
    )
}

fn session_state_with_feed(
    snapshot: &WireStatus,
    feed_blocks: &[WireFeedBlock],
    feed_block_patches: &[crate::wire::WireFeedBlockPatch],
    feed_lines: &[String],
    feed_blocks_base: u64,
    feed_lines_base: u64,
) -> wire::SessionState {
    wire::SessionState {
        session_id: snapshot.session_id.clone(),
        model: snapshot.model.clone(),
        thinking_level: snapshot.thinking_level.clone(),
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
            commands: snapshot.sidebar.commands.clone(),
            runtime_revision: snapshot.sidebar.runtime_revision,
        }),
        feed_blocks: feed_blocks.iter().map(feed_block).collect(),
        feed_blocks_base,
        feed_block_patches: feed_block_patches
            .iter()
            .map(|patch| wire::FeedBlockPatch {
                index: patch.index,
                block: Some(feed_block(&patch.block)),
            })
            .collect(),
        feed_lines: feed_lines.to_vec(),
        feed_lines_base,
        dags: snapshot.dags.iter().map(dag_run_wire).collect(),
        subagents: snapshot.subagents.iter().map(subagent_wire).collect(),
        context_usage: Some(wire::ContextUsage {
            input_tokens: snapshot.usage.input_tokens,
            output_tokens: snapshot.usage.output_tokens,
            cache_read_tokens: snapshot.usage.cache_read_tokens,
            cache_write_tokens: snapshot.usage.cache_write_tokens,
            total_tokens: snapshot.usage.total_tokens,
            context_window: snapshot.usage.context_window.min(u32::MAX as u64) as u32,
        }),
        session_context_usage: Some(wire::ContextUsage {
            input_tokens: snapshot.session_usage.input_tokens,
            output_tokens: snapshot.session_usage.output_tokens,
            cache_read_tokens: snapshot.session_usage.cache_read_tokens,
            cache_write_tokens: snapshot.session_usage.cache_write_tokens,
            total_tokens: snapshot.session_usage.total_tokens,
            context_window: snapshot.session_usage.context_window.min(u32::MAX as u64) as u32,
        }),
        tui_max_feed_lines: snapshot.tui_max_feed_lines.map(|n| n as u32),
        extensions: Some(extension_snapshot_proto(&snapshot.extensions)),
    }
}

pub fn extension_snapshot_proto(snapshot: &WireExtensionSnapshot) -> wire::ExtensionSnapshot {
    wire::ExtensionSnapshot {
        revision: snapshot.revision,
        reload_pending: snapshot.reload_pending,
        catalog: snapshot
            .catalog
            .iter()
            .map(|entry| wire::ExtensionCatalogEntry {
                extension_id: entry.extension_id.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                scope: entry.scope.clone(),
                priority: entry.priority,
                status: entry.status.clone(),
                permissions: entry.permissions.clone(),
                reason_code: entry.reason_code.clone(),
            })
            .collect(),
        diagnostics: snapshot
            .diagnostics
            .iter()
            .map(|diagnostic| wire::ExtensionDiagnostic {
                extension_id: diagnostic.extension_id.clone(),
                code: diagnostic.code.clone(),
                severity: diagnostic.severity.clone(),
                message: diagnostic.message.clone(),
                session_id: diagnostic.session_id.clone(),
                event: diagnostic.event.clone(),
                sequence: diagnostic.sequence,
                details_json: serde_json::to_string(&diagnostic.details)
                    .unwrap_or_else(|_| "{}".into()),
                redacted_fields: diagnostic.redacted_fields.clone(),
            })
            .collect(),
        commands: snapshot
            .commands
            .iter()
            .map(|command| wire::ExtensionCommandDescriptor {
                extension_id: command.extension_id.clone(),
                name: command.name.clone(),
                label: command.label.clone(),
                description: command.description.clone(),
                argument_schema_json: command.argument_schema.to_string(),
            })
            .collect(),
        contributions: snapshot
            .contributions
            .iter()
            .map(|contribution| wire::ExtensionContribution {
                contribution_id: contribution.contribution_id.clone(),
                extension_id: contribution.extension_id.clone(),
                scope: contribution.scope.clone(),
                kind: contribution.kind.clone(),
                payload_json: contribution.payload.to_string(),
            })
            .collect(),
    }
}

pub fn extension_snapshot_wire(
    snapshot: Option<&wire::ExtensionSnapshot>,
) -> WireExtensionSnapshot {
    let Some(snapshot) = snapshot else {
        return WireExtensionSnapshot::default();
    };
    WireExtensionSnapshot {
        revision: snapshot.revision,
        reload_pending: snapshot.reload_pending,
        catalog: snapshot
            .catalog
            .iter()
            .map(|entry| WireExtensionCatalogEntry {
                extension_id: entry.extension_id.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                scope: entry.scope.clone(),
                priority: entry.priority,
                status: entry.status.clone(),
                permissions: entry.permissions.clone(),
                reason_code: entry.reason_code.clone(),
            })
            .collect(),
        diagnostics: snapshot
            .diagnostics
            .iter()
            .map(|diagnostic| WireExtensionDiagnostic {
                extension_id: diagnostic.extension_id.clone(),
                code: diagnostic.code.clone(),
                severity: diagnostic.severity.clone(),
                message: diagnostic.message.clone(),
                session_id: diagnostic.session_id.clone(),
                event: diagnostic.event.clone(),
                sequence: diagnostic.sequence,
                details: serde_json::from_str(&diagnostic.details_json).unwrap_or_default(),
                redacted_fields: diagnostic.redacted_fields.clone(),
            })
            .collect(),
        commands: snapshot
            .commands
            .iter()
            .map(|command| WireExtensionCommandDescriptor {
                extension_id: command.extension_id.clone(),
                name: command.name.clone(),
                label: command.label.clone(),
                description: command.description.clone(),
                argument_schema: serde_json::from_str(&command.argument_schema_json)
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect(),
        contributions: snapshot
            .contributions
            .iter()
            .map(|contribution| WireExtensionContribution {
                contribution_id: contribution.contribution_id.clone(),
                extension_id: contribution.extension_id.clone(),
                scope: contribution.scope.clone(),
                kind: contribution.kind.clone(),
                payload: serde_json::from_str(&contribution.payload_json)
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect(),
    }
}

/// Convert a `SessionState` (proto, from a gRPC client) back into the internal
/// serde snapshot model. The client half of the protocol (TUI, future local
/// clients) renders from `WireStatus` exactly like the JSON surface does, so
/// every snapshot frame round-trips through this conversion.
pub fn wire_status(state: &wire::SessionState) -> WireStatus {
    WireStatus {
        session_id: state.session_id.clone(),
        model: state.model.clone(),
        thinking_level: state.thinking_level.clone(),
        model_catalog: state
            .model_catalog
            .iter()
            .map(|group| crate::wire::ProviderGroup {
                provider: group.provider.clone(),
                has_credential: group.has_credential,
                models: group
                    .models
                    .iter()
                    .map(|entry| crate::wire::ModelEntry {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                    })
                    .collect(),
            })
            .collect(),
        cwd: state.cwd.clone(),
        busy: state.busy,
        queued_count: state.queued_count as usize,
        latest_trigger_poll: state.latest_trigger_poll.as_ref().map(|status| {
            crate::feed::TriggerPollStatus {
                checked_at: status.checked_at.clone(),
                trace_id: status.trace_id.clone(),
                source_label: status.source_label.clone(),
                event_label: status.event_label.clone(),
                summary: status.summary.clone(),
            }
        }),
        goal: state
            .goal
            .as_ref()
            .map(|goal| crate::wire::WireGoalSnapshot {
                condition: goal.condition.clone(),
                status: goal.status.clone(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.clone(),
            }),
        control_plane_prompt: state.control_plane_prompt.as_ref().map(|prompt| {
            crate::wire::WireControlPlanePromptSnapshot {
                tool_name: prompt.tool_name.clone(),
                label: prompt.label.clone(),
                reason: prompt.reason.clone(),
                args_hash: prompt.args_hash.clone(),
                payload: prompt.payload.clone(),
            }
        }),
        sidebar: sidebar_wire(state.sidebar.as_ref()),
        feed_blocks: state.feed_blocks.iter().map(wire_feed_block).collect(),
        feed_blocks_base: state.feed_blocks_base,
        feed_block_patches: state
            .feed_block_patches
            .iter()
            .filter_map(|patch| {
                patch
                    .block
                    .as_ref()
                    .map(|block| crate::wire::WireFeedBlockPatch {
                        index: patch.index,
                        block: wire_feed_block(block),
                    })
            })
            .collect(),
        feed_lines: state.feed_lines.clone(),
        feed_lines_base: state.feed_lines_base,
        dags: state.dags.iter().map(wire_dag_run).collect(),
        subagents: state.subagents.iter().map(wire_subagent_job).collect(),
        usage: state
            .context_usage
            .as_ref()
            .map(|usage| crate::wire::WireContextUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                total_tokens: usage.total_tokens,
                context_window: u64::from(usage.context_window),
            })
            .unwrap_or_default(),
        session_usage: state
            .session_context_usage
            .as_ref()
            .map(|usage| crate::wire::WireContextUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_tokens,
                cache_write_tokens: usage.cache_write_tokens,
                total_tokens: usage.total_tokens,
                context_window: u64::from(usage.context_window),
            })
            .unwrap_or_default(),
        tui_max_feed_lines: state.tui_max_feed_lines.map(u64::from),
        extensions: extension_snapshot_wire(state.extensions.as_ref()),
    }
}

fn sidebar_wire(sidebar: Option<&wire::SidebarSnapshot>) -> crate::wire::WireSidebarSnapshot {
    let sidebar = sidebar.cloned().unwrap_or_default();
    crate::wire::WireSidebarSnapshot {
        inbox_new: sidebar.inbox_new as usize,
        skills: crate::wire::WireSkillsSnapshot {
            total: sidebar
                .skills
                .as_ref()
                .map(|s| s.total as usize)
                .unwrap_or(0),
            enabled: sidebar
                .skills
                .as_ref()
                .map(|s| s.enabled as usize)
                .unwrap_or(0),
            disabled: sidebar
                .skills
                .as_ref()
                .map(|s| s.disabled as usize)
                .unwrap_or(0),
            builtin: sidebar
                .skills
                .as_ref()
                .map(|s| s.builtin as usize)
                .unwrap_or(0),
            user: sidebar
                .skills
                .as_ref()
                .map(|s| s.user as usize)
                .unwrap_or(0),
            project: sidebar
                .skills
                .as_ref()
                .map(|s| s.project as usize)
                .unwrap_or(0),
            items: sidebar
                .skills
                .as_ref()
                .map(|s| {
                    s.items
                        .iter()
                        .map(|skill| crate::wire::WireSkillSnapshot {
                            name: skill.name.clone(),
                            source: skill.source.clone(),
                            file_path: skill.file_path.clone(),
                            enabled: skill.enabled,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        triggers: crate::wire::WireTriggersSnapshot {
            total: sidebar
                .triggers
                .as_ref()
                .map(|t| t.total as usize)
                .unwrap_or(0),
            enabled: sidebar
                .triggers
                .as_ref()
                .map(|t| t.enabled as usize)
                .unwrap_or(0),
            disabled: sidebar
                .triggers
                .as_ref()
                .map(|t| t.disabled as usize)
                .unwrap_or(0),
            rules: sidebar
                .triggers
                .as_ref()
                .map(|t| {
                    t.rules
                        .iter()
                        .map(|rule| crate::wire::WireTriggerRuleSnapshot {
                            id: rule.id.clone(),
                            full_id: rule.full_id.clone(),
                            enabled: rule.enabled,
                            mode: rule.mode.clone(),
                            condition: rule.condition.clone(),
                            action: rule.action.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        cron: crate::wire::WireCronSnapshot {
            total: sidebar.cron.as_ref().map(|c| c.total as usize).unwrap_or(0),
            enabled: sidebar
                .cron
                .as_ref()
                .map(|c| c.enabled as usize)
                .unwrap_or(0),
            disabled: sidebar
                .cron
                .as_ref()
                .map(|c| c.disabled as usize)
                .unwrap_or(0),
            jobs: sidebar
                .cron
                .as_ref()
                .map(|c| {
                    c.jobs
                        .iter()
                        .map(|job| crate::wire::WireCronJobSnapshot {
                            id: job.id.clone(),
                            enabled: job.enabled,
                            schedule: job.schedule.clone(),
                            action: job.action.clone(),
                            skipped_overlap_count: job.skipped_overlap_count,
                            last_error: job.last_error.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        mcp: crate::wire::WireMcpSnapshot {
            servers: sidebar
                .mcp
                .as_ref()
                .map(|m| m.servers as usize)
                .unwrap_or(0),
            tools: sidebar.mcp.as_ref().map(|m| m.tools as usize).unwrap_or(0),
            notification_hooks: sidebar
                .mcp
                .as_ref()
                .map(|m| m.notification_hooks as usize)
                .unwrap_or(0),
            server_names: sidebar
                .mcp
                .as_ref()
                .map(|m| m.server_names.clone())
                .unwrap_or_default(),
            tool_names: sidebar
                .mcp
                .as_ref()
                .map(|m| m.tool_names.clone())
                .unwrap_or_default(),
        },
        tools: crate::wire::WireToolsSnapshot {
            total: sidebar
                .tools
                .as_ref()
                .map(|t| t.total as usize)
                .unwrap_or(0),
            names: sidebar
                .tools
                .as_ref()
                .map(|t| t.names.clone())
                .unwrap_or_default(),
        },
        hooks: sidebar.hooks.clone(),
        runtime: sidebar.runtime.clone(),
        commands: sidebar.commands.clone(),
        runtime_revision: sidebar.runtime_revision,
    }
}

fn wire_feed_block(block: &wire::FeedBlock) -> WireFeedBlock {
    use wire::feed_block::Kind;
    let Some(kind) = block.kind.as_ref() else {
        return WireFeedBlock::Plain {
            text: String::new(),
            level: feed::Level::Output,
            timestamp: None,
        };
    };
    match kind {
        Kind::User(block) => WireFeedBlock::User {
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::Assistant(block) => WireFeedBlock::Assistant {
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::Thinking(block) => WireFeedBlock::Thinking {
            text: block.text.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::Tool(block) => WireFeedBlock::Tool {
            name: block.name.clone(),
            args: block.args.clone(),
            timestamp: block.timestamp.clone(),
        },
        Kind::ToolResult(block) => WireFeedBlock::ToolResult {
            lines: block.lines.clone(),
            is_error: block.is_error,
            timestamp: block.timestamp.clone(),
        },
        Kind::Plain(block) => WireFeedBlock::Plain {
            text: block.text.clone(),
            level: level_from_str(&block.level),
            timestamp: block.timestamp.clone(),
        },
    }
}

/// `PlainBlock.level` serializes as snake_case variant names on the JSON surface.
fn level_from_str(level: &str) -> feed::Level {
    match level {
        "system" => feed::Level::System,
        "error" => feed::Level::Error,
        "note" => feed::Level::Note,
        "header" => feed::Level::Header,
        "qr" => feed::Level::Qr,
        _ => feed::Level::Output,
    }
}

fn wire_dag_run(run: &wire::DagRunSnapshot) -> crate::wire::WireDagRunSnapshot {
    crate::wire::WireDagRunSnapshot {
        id: run.id.clone(),
        name: run.name.clone(),
        kind: run.kind.clone(),
        status: run.status.clone(),
        fail_fast: run.fail_fast,
        max_concurrency: run.max_concurrency as usize,
        direction: run.direction.clone(),
        created_at: run.created_at,
        completed_at: run.completed_at,
        error: run.error.clone(),
        nodes: run
            .nodes
            .iter()
            .map(|node| crate::wire::WireDagNodeSnapshot {
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
                result: node
                    .result
                    .as_ref()
                    .map(|result| crate::wire::WireNodeResultSnapshot {
                        success: result.success,
                        error: result.error.clone(),
                        duration_ms: result.duration_ms,
                        attempt: result.attempt,
                        total_attempts: result.total_attempts,
                    }),
                output_tail: node.output_tail.clone(),
                live_preview: node.live_preview.clone(),
            })
            .collect(),
    }
}

fn wire_subagent_job(job: &wire::SubagentJobSnapshot) -> crate::wire::WireAgentJobSnapshot {
    crate::wire::WireAgentJobSnapshot {
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

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/proto/resources.rs"
));

#[cfg(test)]
// Test files live in `tests/transport/proto/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("proto");
