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
        context_usage: Some(context_usage_to_proto(&snapshot.usage)),
        session_context_usage: Some(context_usage_to_proto(&snapshot.session_usage)),
        tui_max_feed_lines: snapshot.tui_max_feed_lines.map(|n| n as u32),
        extensions: Some(extension_snapshot_proto(&snapshot.extensions)),
        system_context: snapshot.system_context.clone(),
    }
}
