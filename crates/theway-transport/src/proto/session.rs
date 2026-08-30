/// Convert a `WireStatus` into the nested `SessionSnapshot` proto message.
pub fn session_snapshot_wire(status: &WireStatus) -> wire::SessionSnapshot {
    wire_session_snapshot(&WireSessionSnapshot::from(status))
}

/// Convert a `SessionSnapshot` proto message back into the internal `WireStatus`.
pub fn wire_status_from_session_snapshot(snapshot: &wire::SessionSnapshot) -> WireStatus {
    WireStatus::from(&wire_session_snapshot_from_proto(snapshot))
}

/// Convert the nested wire snapshot into the proto `SessionSnapshot`.
#[allow(deprecated)]
pub fn wire_session_snapshot(snapshot: &WireSessionSnapshot) -> wire::SessionSnapshot {
    let status = WireStatus::from(snapshot);
    wire::SessionSnapshot {
        session_id: if !snapshot.session_id.is_empty() {
            snapshot.session_id.clone()
        } else {
            status.session_id.clone()
        },
        info: Some(wire::SessionInfo {
            id: if !snapshot.info.id.is_empty() {
                snapshot.info.id.clone()
            } else {
                status.session_id.clone()
            },
            name: snapshot.info.name.clone(),
            cwd: snapshot.info.cwd.clone(),
            created_at: snapshot.info.created_at.clone(),
            last_activity_at: snapshot.info.last_activity_at,
            last_activity_at_rfc3339: snapshot.info.last_activity_at_rfc3339.clone(),
            busy: snapshot.info.busy,
            preview: snapshot.info.preview.clone(),
            metadata: snapshot.info.metadata.clone(),
            graph_count: snapshot.info.graph_count,
            active_graph_count: snapshot.info.active_graph_count,
            queued_count: snapshot.info.queued_count as u32,
            sidebar: sidebar_proto(&status.sidebar),
        }),
        runtime: Some(wire::SessionRuntime {
            model: Some(wire::ModelRef {
                provider: snapshot.runtime.model.provider.clone(),
                model: snapshot.runtime.model.model.clone(),
                base_url: snapshot.runtime.model.base_url.clone(),
            }),
            thinking_level: thinking_level_to_proto(&snapshot.runtime.thinking_level),
            supported_thinking_levels: snapshot
                .runtime
                .supported_thinking_levels
                .iter()
                .map(|level| thinking_level_to_proto(level))
                .collect(),
            context_usage: Some(context_usage_to_proto(&status.usage)),
            session_context_usage: Some(context_usage_to_proto(&status.session_usage)),
            tui_max_feed_lines: status.tui_max_feed_lines.map(|n| n as u32),
            model_catalog: status
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
            latest_trigger_poll: status.latest_trigger_poll.as_ref().map(|poll| {
                wire::TriggerPollStatus {
                    checked_at: poll.checked_at.clone(),
                    trace_id: poll.trace_id.clone(),
                    source_label: poll.source_label.clone(),
                    event_label: poll.event_label.clone(),
                    summary: poll.summary.clone(),
                }
            }),
            goal: status.goal.as_ref().map(|goal| wire::GoalSnapshot {
                condition: goal.condition.clone(),
                status: goal.status.clone(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.clone(),
            }),
            control_plane_prompt: status.control_plane_prompt.as_ref().map(|prompt| {
                wire::ControlPlanePromptSnapshot {
                    tool_name: prompt.tool_name.clone(),
                    label: prompt.label.clone(),
                    reason: prompt.reason.clone(),
                    args_hash: prompt.args_hash.clone(),
                    payload: prompt.payload.clone(),
                }
            }),
            extensions: Some(extension_snapshot_proto(&status.extensions)),
            system_context: snapshot.runtime.system_context.clone(),
        }),
        feed: Some(wire::SessionFeed {
            blocks: snapshot.feed.blocks.iter().map(feed_block).collect(),
            lines: snapshot.feed.lines.clone(),
            blocks_base: snapshot.feed.blocks_base,
            lines_base: snapshot.feed.lines_base,
            block_patches: snapshot
                .feed
                .block_patches
                .iter()
                .map(|patch| wire::FeedBlockPatch {
                    index: patch.index,
                    block: Some(feed_block(&patch.block)),
                })
                .collect(),
        }),
        graph_state: Some(wire::SessionGraphState {
            dags: snapshot.graph_state.dags.iter().map(dag_run_wire).collect(),
            subagents: snapshot
                .graph_state
                .subagents
                .iter()
                .map(subagent_wire)
                .collect(),
            nodes: snapshot
                .graph_state
                .nodes
                .iter()
                .map(session_graph_node_wire)
                .collect(),
            active_node_id: snapshot.graph_state.active_node_id.clone(),
        }),
        lineage: Some(wire::SessionLineage {
            parent_session_id: snapshot.lineage.parent_session_id.clone(),
            root_session_id: snapshot.lineage.root_session_id.clone(),
            ancestor_session_ids: snapshot.lineage.ancestor_session_ids.clone(),
            child_session_ids: snapshot.lineage.child_session_ids.clone(),
            collapsed_from_session_id: snapshot.lineage.collapsed_from_session_id.clone(),
            collapsed_into_session_id: snapshot.lineage.collapsed_into_session_id.clone(),
        }),
    }
}

/// Convert the proto `SessionSnapshot` into the nested wire snapshot.
pub fn wire_session_snapshot_from_proto(snapshot: &wire::SessionSnapshot) -> WireSessionSnapshot {
    let info = session_info_from_proto(snapshot.info.as_ref());
    WireSessionSnapshot {
        session_id: if !snapshot.session_id.is_empty() {
            snapshot.session_id.clone()
        } else {
            info.id.clone()
        },
        info,
        runtime: session_runtime_from_proto(snapshot.runtime.as_ref()),
        feed: session_feed_from_proto(snapshot.feed.as_ref()),
        graph_state: session_graph_state_from_proto(snapshot.graph_state.as_ref()),
        lineage: session_lineage_from_proto(snapshot.lineage.as_ref()),
    }
}

#[allow(deprecated)]
fn session_info_from_proto(info: Option<&wire::SessionInfo>) -> WireSessionInfo {
    let info = info.cloned().unwrap_or_default();
    WireSessionInfo {
        id: info.id,
        name: info.name,
        cwd: info.cwd,
        created_at: info.created_at,
        last_activity_at: info.last_activity_at,
        last_activity_at_rfc3339: info.last_activity_at_rfc3339,
        busy: info.busy,
        preview: info.preview,
        metadata: info.metadata,
        graph_count: info.graph_count,
        active_graph_count: info.active_graph_count,
        queued_count: info.queued_count as usize,
        sidebar: sidebar_wire(info.sidebar.as_ref()),
    }
}

fn session_runtime_from_proto(runtime: Option<&wire::SessionRuntime>) -> WireSessionRuntime {
    let runtime = runtime.cloned().unwrap_or_default();
    let model = runtime.model.unwrap_or_default();
    WireSessionRuntime {
        model: crate::wire::WireModelRef {
            provider: model.provider,
            model: model.model,
            base_url: model.base_url,
        },
        thinking_level: thinking_level_from_proto(runtime.thinking_level),
        supported_thinking_levels: runtime
            .supported_thinking_levels
            .iter()
            .map(|level| thinking_level_from_proto(*level))
            .collect(),
        context_usage: context_usage_from_proto(runtime.context_usage.as_ref()),
        session_context_usage: context_usage_from_proto(runtime.session_context_usage.as_ref()),
        tui_max_feed_lines: runtime.tui_max_feed_lines.map(u64::from),
        model_catalog: runtime
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
        latest_trigger_poll: runtime.latest_trigger_poll.as_ref().map(|status| {
            crate::feed::TriggerPollStatus {
                checked_at: status.checked_at.clone(),
                trace_id: status.trace_id.clone(),
                source_label: status.source_label.clone(),
                event_label: status.event_label.clone(),
                summary: status.summary.clone(),
            }
        }),
        goal: runtime
            .goal
            .as_ref()
            .map(|goal| crate::wire::WireGoalSnapshot {
                condition: goal.condition.clone(),
                status: goal.status.clone(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.clone(),
            }),
        control_plane_prompt: runtime.control_plane_prompt.as_ref().map(|prompt| {
            crate::wire::WireControlPlanePromptSnapshot {
                tool_name: prompt.tool_name.clone(),
                label: prompt.label.clone(),
                reason: prompt.reason.clone(),
                args_hash: prompt.args_hash.clone(),
                payload: prompt.payload.clone(),
            }
        }),
        extensions: extension_snapshot_wire(runtime.extensions.as_ref()),
        system_context: runtime.system_context.clone(),
    }
}

fn context_usage_to_proto(usage: &crate::wire::WireContextUsage) -> wire::ContextUsage {
    wire::ContextUsage {
        cached_tokens: usage.cached_tokens,
        new_tokens: usage.new_tokens,
        total_input_tokens: usage.total_input_tokens,
        output_tokens: usage.output_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        provider_cache_hit_rate: usage.provider_cache_hit_rate,
        prefix_cache_hit_rate: usage.prefix_cache_hit_rate,
        prefix_hit_tokens: Some(usage.prefix_hit_tokens),
        context_window: usage.context_window.min(u32::MAX as u64) as u32,
    }
}

fn context_usage_from_proto(usage: Option<&wire::ContextUsage>) -> crate::wire::WireContextUsage {
    usage
        .map(|usage| crate::wire::WireContextUsage {
            cached_tokens: usage.cached_tokens,
            new_tokens: usage.new_tokens,
            total_input_tokens: usage.total_input_tokens,
            output_tokens: usage.output_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            provider_cache_hit_rate: usage.provider_cache_hit_rate,
            prefix_cache_hit_rate: usage.prefix_cache_hit_rate,
            prefix_hit_tokens: usage.prefix_hit_tokens.unwrap_or(0),
            context_window: u64::from(usage.context_window),
        })
        .unwrap_or_default()
}

fn session_feed_from_proto(feed: Option<&wire::SessionFeed>) -> crate::wire::WireSessionFeed {
    let feed = feed.cloned().unwrap_or_default();
    crate::wire::WireSessionFeed {
        blocks: feed.blocks.iter().map(wire_feed_block).collect(),
        lines: feed.lines,
        blocks_base: feed.blocks_base,
        lines_base: feed.lines_base,
        block_patches: feed
            .block_patches
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
    }
}

fn session_graph_state_from_proto(
    graph_state: Option<&wire::SessionGraphState>,
) -> crate::wire::WireSessionGraphState {
    let graph_state = graph_state.cloned().unwrap_or_default();
    crate::wire::WireSessionGraphState {
        dags: graph_state.dags.iter().map(wire_dag_run).collect(),
        subagents: graph_state
            .subagents
            .iter()
            .map(wire_subagent_job)
            .collect(),
        nodes: graph_state
            .nodes
            .iter()
            .map(session_graph_node_from_proto)
            .collect(),
        active_node_id: graph_state.active_node_id,
    }
}

fn session_lineage_from_proto(
    lineage: Option<&wire::SessionLineage>,
) -> crate::wire::WireSessionLineage {
    let lineage = lineage.cloned().unwrap_or_default();
    crate::wire::WireSessionLineage {
        parent_session_id: lineage.parent_session_id,
        root_session_id: lineage.root_session_id,
        ancestor_session_ids: lineage.ancestor_session_ids,
        child_session_ids: lineage.child_session_ids,
        collapsed_from_session_id: lineage.collapsed_from_session_id,
        collapsed_into_session_id: lineage.collapsed_into_session_id,
    }
}
