//! Composer feature labels (issue #39): the graph-engine label plus the
//! per-agent `model · think level` fragments derived from the snapshot's DAG
//! runs and subagent jobs.

/// Composer feature labels (issue #39): the composer's top-right corner
/// shows only the graph-engine feature — any `dag`-kind run activates
/// `graph engine`; otherwise the list is empty and the chrome renders
/// nothing. While active, the label also lists the model + thinking
/// intensity each graph node and standalone subagent runs with, deduped by
/// `(agent, model, thinking)`. Trigger-runtime features stay in the trigger
/// panel's Runtime section.
pub(super) fn feature_labels(
    dags: &[theway_transport::wire::WireDagRunSnapshot],
    subagents: &[theway_transport::wire::WireAgentJobSnapshot],
) -> Vec<String> {
    if !dags.iter().any(|run| run.kind == "dag") {
        return Vec::new();
    }
    let mut labels = vec!["graph engine".to_string()];
    let mut push_runtime = |agent: &str, model: Option<&str>, thinking: Option<&str>| {
        let Some(label) = agent_runtime_label(agent, model, thinking) else {
            return;
        };
        if !labels.iter().any(|existing| existing == &label) {
            labels.push(label);
        }
    };
    // Graph nodes first (their DAG jobs also appear in `subagents` but are
    // already represented here), then standalone subagent-tool jobs.
    for run in dags.iter().filter(|run| run.kind == "dag") {
        for node in &run.nodes {
            push_runtime(&node.agent, node.model.as_deref(), node.thinking.as_deref());
        }
    }
    for job in subagents.iter().filter(|job| job.source != "dag") {
        push_runtime(&job.agent, job.model.as_deref(), job.thinking.as_deref());
    }
    labels
}

/// One `agent model · think level` feature-label fragment. The fragment is
/// omitted when neither the model nor the thinking level is known; `think`
/// renders only for an explicit non-off level, matching the composer info
/// line.
fn agent_runtime_label(agent: &str, model: Option<&str>, thinking: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    let thinking = thinking
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "off");
    if model.is_none() && thinking.is_none() {
        return None;
    }
    let mut label = agent.trim().to_string();
    if let Some(model) = model {
        label.push(' ');
        label.push_str(model);
    }
    if let Some(thinking) = thinking {
        label.push_str(" · think ");
        label.push_str(thinking);
    }
    (!label.is_empty()).then_some(label)
}
