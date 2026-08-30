/// Convert the internal session summary (session-resource-model) into the
/// structured wire model.
#[allow(deprecated)]
pub fn session_summary_wire(summary: &crate::wire::SessionSummary) -> wire::SessionSummary {
    wire::SessionSummary {
        id: summary.session_id.clone(),
        session_id: summary.session_id.clone(),
        name: summary.name.clone(),
        cwd: summary.cwd.clone(),
        model: summary.model.clone(),
        created_at: summary.created_at.clone(),
        last_activity_at: summary.last_activity_at,
        last_activity_at_rfc3339: summary.last_activity_at_rfc3339.clone(),
        graph_count: summary.graph_count,
        active_graph_count: summary.active_graph_count,
        busy: summary.busy,
        preview: summary.preview.clone(),
        tree_prefix: summary.tree_prefix.clone(),
        metadata: summary.metadata.clone(),
    }
}

#[allow(deprecated)]
pub fn session_summary_from_proto(summary: &wire::SessionSummary) -> crate::wire::SessionSummary {
    let session_id = if !summary.id.is_empty() {
        summary.id.clone()
    } else {
        summary.session_id.clone()
    };
    crate::wire::SessionSummary {
        session_id,
        name: summary.name.clone(),
        cwd: summary.cwd.clone(),
        model: summary.model.clone(),
        created_at: summary.created_at.clone(),
        last_activity_at: summary.last_activity_at,
        last_activity_at_rfc3339: summary.last_activity_at_rfc3339.clone(),
        graph_count: summary.graph_count,
        active_graph_count: summary.active_graph_count,
        busy: summary.busy,
        preview: summary.preview.clone(),
        tree_prefix: summary.tree_prefix.clone(),
        metadata: summary.metadata.clone(),
    }
}

pub fn session_runtime_context_to_proto(
    ctx: &crate::wire::WireSessionRuntimeContext,
) -> wire::SessionRuntimeContext {
    wire::SessionRuntimeContext {
        work_dir: ctx.work_dir.clone(),
        provider: ctx.provider.clone(),
        model: ctx.model.clone(),
        base_url: ctx.base_url.clone(),
        thinking: ctx.thinking,
    }
}

pub fn session_runtime_context_from_proto(
    ctx: &wire::SessionRuntimeContext,
) -> crate::wire::WireSessionRuntimeContext {
    crate::wire::WireSessionRuntimeContext {
        work_dir: ctx.work_dir.clone(),
        provider: ctx.provider.clone(),
        model: ctx.model.clone(),
        base_url: ctx.base_url.clone(),
        thinking: ctx.thinking,
    }
}

pub fn activate_session_request_from_proto(
    request: &wire::ActivateSessionRequest,
) -> Result<crate::wire::WireActivateSessionRequest, crate::wire::WireRpcError> {
    let runtime = request.runtime.as_ref().ok_or(crate::wire::WireRpcError {
        code: "missing_runtime".into(),
        message: "ActivateSessionRequest.runtime is required".into(),
    })?;
    Ok(crate::wire::WireActivateSessionRequest {
        session_id: request.session_id.clone(),
        client_key: request.client_key.clone(),
        name: request.name.clone(),
        runtime: Some(session_runtime_context_from_proto(runtime)),
    })
}

pub fn activate_session_response_to_proto(
    response: &crate::wire::WireActivateSessionResponse,
) -> wire::ActivateSessionResponse {
    wire::ActivateSessionResponse {
        session: response.session.as_ref().map(session_summary_wire),
        created: response.created,
    }
}

/// Set-credential requests carry secrets; the returned wire type is
/// non-Clone/non-Debug/non-serializable.
pub fn set_credential_request_from_proto(
    request: &wire::SetCredentialRequest,
) -> crate::wire::WireSetCredentialRequest {
    crate::wire::WireSetCredentialRequest {
        session_id: request.session_id.clone(),
        provider: request.provider.clone(),
        secret: request.secret.clone(),
    }
}

pub fn clear_credential_request_from_proto(
    request: &wire::ClearCredentialRequest,
) -> crate::wire::WireClearCredentialRequest {
    crate::wire::WireClearCredentialRequest {
        session_id: request.session_id.clone(),
        provider: request.provider.clone(),
    }
}

/// Convert the daemon path context (issue #68) into the structured wire model.
pub fn wire_path_context_to_proto(ctx: &WirePathContext) -> wire::PathContext {
    wire::PathContext {
        home: ctx.home.clone(),
        base: ctx.base.clone(),
        work_dir: ctx.work_dir.clone(),
        skills_dirs: ctx.skills_dirs.clone(),
    }
}

/// Convert a `PathContext` (proto, from a gRPC response) back into the internal
/// path-context model.
pub fn wire_path_context_from_proto(p: &wire::PathContext) -> WirePathContext {
    WirePathContext {
        home: p.home.clone(),
        base: p.base.clone(),
        work_dir: p.work_dir.clone(),
        skills_dirs: p.skills_dirs.clone(),
    }
}

/// Convert the daemon configuration view (issue #72) into the structured wire
/// model.
pub fn daemon_config_to_proto(config: &crate::wire::WireDaemonConfig) -> wire::DaemonConfig {
    wire::DaemonConfig {
        provider: config.provider.clone(),
        model: config.model.clone(),
        base_url: config.base_url.clone(),
        thinking: config.thinking,
        thinking_level: config.thinking_level.clone(),
        builtin_skills: config.builtin_skills.clone(),
        skills_dirs: config.skills_dirs.clone(),
        trigger_poll_secs: config
            .trigger_poll_secs
            .map(|secs| secs.min(u32::MAX as u64) as u32),
        tui_max_feed_lines: config
            .tui_max_feed_lines
            .map(|lines| lines.min(u32::MAX as u64) as u32),
        tool_service_addr: config.tool_service_addr.clone(),
        storage_service_addr: config.storage_service_addr.clone(),
        clear_fields: config.clear_fields.clone(),
    }
}

/// Convert a `DaemonConfig` (proto, from a gRPC request/response) back into
/// the internal settings model.
pub fn daemon_config_from_proto(config: &wire::DaemonConfig) -> crate::wire::WireDaemonConfig {
    crate::wire::WireDaemonConfig {
        provider: config.provider.clone(),
        model: config.model.clone(),
        base_url: config.base_url.clone(),
        thinking: config.thinking,
        thinking_level: config.thinking_level.clone(),
        builtin_skills: config.builtin_skills.clone(),
        skills_dirs: config.skills_dirs.clone(),
        trigger_poll_secs: config.trigger_poll_secs.map(u64::from),
        tui_max_feed_lines: config.tui_max_feed_lines.map(u64::from),
        tool_service_addr: config.tool_service_addr.clone(),
        storage_service_addr: config.storage_service_addr.clone(),
        clear_fields: config.clear_fields.clone(),
    }
}

/// Resolve a session id argument (full id or unique prefix, same semantics as the
/// repo-backed `SessionOps` impls) against a session list. Returns the full id, or
/// `None` when nothing or more than one session matches.
pub(crate) fn resolve_session_id(
    sessions: &[crate::wire::SessionSummary],
    id: &str,
) -> Option<String> {
    match theway_contract::session_id::resolve_unique_prefix(
        sessions.iter().map(|session| session.session_id.as_str()),
        id,
    ) {
        theway_contract::session_id::PrefixMatch::Unique(id) => Some(id.to_string()),
        theway_contract::session_id::PrefixMatch::None
        | theway_contract::session_id::PrefixMatch::Ambiguous => None,
    }
}

/// Convert one DAG run snapshot into the wire form.
pub fn dag_run_wire(run: &crate::wire::WireDagRunSnapshot) -> wire::DagRunSnapshot {
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

fn dag_node_wire(node: &crate::wire::WireDagNodeSnapshot) -> wire::DagNodeSnapshot {
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
pub fn stream_event_wire(event: &WireAgentEvent) -> wire::StreamEvent {
    use wire::stream_event::Kind;
    let kind = match event {
        WireAgentEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
            ..
        } => Kind::SubagentStarted(wire::SubagentStarted {
            id: id.clone(),
            agent: agent.clone(),
            source: source.clone(),
            run_id: run_id.clone(),
            node_id: node_id.clone(),
        }),
        WireAgentEvent::Output { id, chunk, .. } => Kind::SubagentOutput(wire::SubagentOutput {
            id: id.clone(),
            chunk: chunk.clone(),
        }),
        WireAgentEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
            ..
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
        WireAgentEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            ..
        } => Kind::SubagentCompleted(wire::SubagentCompleted {
            id: id.clone(),
            status: status.clone(),
            error: error.clone(),
            duration_ms: None,
            chars: *chars,
            tokens_in: *tokens_in,
            tokens_out: *tokens_out,
            tools_called: *tools_called,
        }),
    };
    let session_id = match event {
        WireAgentEvent::Started { session_id, .. }
        | WireAgentEvent::Output { session_id, .. }
        | WireAgentEvent::Metrics { session_id, .. }
        | WireAgentEvent::Completed { session_id, .. } => session_id.clone(),
    };
    wire::StreamEvent {
        session_id,
        kind: Some(kind),
    }
}

/// Convert a DAG engine event-plane message (node_status / run_status) into
/// the wire `StreamEvent`.
pub fn dag_event_wire(event: &WireDagEvent) -> wire::StreamEvent {
    use wire::stream_event::Kind;
    let kind = match event {
        WireDagEvent::NodeStatus {
            run_id,
            node_id,
            status,
            error,
            ..
        } => Kind::NodeStatus(wire::NodeStatus {
            run_id: run_id.clone(),
            node_id: node_id.clone(),
            status: status.clone(),
            error: error.clone(),
        }),
        WireDagEvent::RunStatus {
            run_id,
            status,
            error,
            ..
        } => Kind::RunStatus(wire::RunStatus {
            run_id: run_id.clone(),
            status: status.clone(),
            error: error.clone(),
        }),
    };
    let session_id = match event {
        WireDagEvent::NodeStatus { session_id, .. }
        | WireDagEvent::RunStatus { session_id, .. } => session_id.clone(),
    };
    wire::StreamEvent {
        session_id,
        kind: Some(kind),
    }
}

fn subagent_wire(job: &crate::wire::WireAgentJobSnapshot) -> wire::SubagentJobSnapshot {
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
fn feed_block(block: &WireFeedBlock) -> wire::FeedBlock {
    use wire::feed_block::Kind;
    let kind = match block {
        WireFeedBlock::User { text, timestamp } => Kind::User(wire::UserBlock {
            text: text.clone(),
            timestamp: timestamp.clone(),
        }),
        WireFeedBlock::Assistant { text, timestamp } => Kind::Assistant(wire::AssistantBlock {
            text: text.clone(),
            timestamp: timestamp.clone(),
        }),
        WireFeedBlock::Thinking { text, timestamp } => Kind::Thinking(wire::ThinkingBlock {
            text: text.clone(),
            timestamp: timestamp.clone(),
        }),
        WireFeedBlock::ToolCall {
            name,
            args,
            metadata,
            timestamp,
        } => Kind::ToolCall(wire::ToolCallBlock {
            name: name.clone(),
            args: args.clone(),
            metadata: metadata.clone(),
            timestamp: timestamp.clone(),
        }),
        WireFeedBlock::Error {
            message,
            code,
            recoverable,
            timestamp,
        } => Kind::Error(wire::ErrorBlock {
            message: message.clone(),
            code: code.clone(),
            recoverable: *recoverable,
            timestamp: timestamp.clone(),
        }),
        WireFeedBlock::ToolResult {
            lines,
            is_error,
            timestamp,
        } => Kind::ToolResult(wire::ToolResultBlock {
            lines: lines.clone(),
            is_error: *is_error,
            timestamp: timestamp.clone(),
        }),
        WireFeedBlock::Plain {
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

/// Public wrapper for protocol adapters: serde feed block → proto oneof.
pub fn wire_feed_block_to_proto(block: &WireFeedBlock) -> wire::FeedBlock {
    feed_block(block)
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
