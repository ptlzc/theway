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
