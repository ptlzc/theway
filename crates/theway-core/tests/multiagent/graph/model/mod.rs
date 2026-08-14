use super::*;

mod build;
mod helpers;
mod mermaid;
mod reconcile;
mod render;
mod validate;

fn node_def(id: &str, agent: &str, task: &str, deps: &[&str]) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: agent.to_string(),
        task: task.to_string(),
        depends_on: if deps.is_empty() {
            None
        } else {
            Some(deps.iter().map(|s| s.to_string()).collect())
        },
        timeout: None,
        cwd: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
    }
}

fn run_def(name: &str, nodes: Vec<DagNodeDef>) -> DagRunDef {
    DagRunDef {
        name: name.to_string(),
        nodes,
        max_concurrency: None,
        fail_fast: None,
        direction: None,
    }
}
