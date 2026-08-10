//! `subagent_specs` — built-in subagent specifications for DAG node execution.
//!
//! Rust-side counterpart of the TS `.pi/subagents/config.yaml` agent registry the
//! dag-orchestrator extension resolves DAG node `agent` fields against. Each spec names a
//! built-in agent type, a short system prompt, and an iteration budget.
//! [`node_launcher`](super::node_launcher) resolves a node's `agent` field through
//! [`resolve_spec`]; unknown names fail the node with `unknown agent "..."` (mirrors the
//! TS `defaultLauncher`).
//!
//! Specs carry **metadata only** — no tool-set factory. Tool sets are an app-layer
//! decision (the local tools live in the `theway` server crate and may become remote
//! sandbox execution later), so the engine takes a tool-set *resolver* at launcher /
//! task-tool construction time and supplies the resolved tools per run. See
//! `theway_core::tools` module docs for the split rationale.
//!
//! v1 scope: static built-in table only. User-defined specs via `~/.theway/subagents/*.toml`
//! are a follow-up (same line as the `subagent` tool's issue #11 out-of-scope note).

use once_cell::sync::Lazy;

/// Static description of a built-in subagent.
pub struct SubagentSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// Short (1–3 sentences) system prompt, same style as the `subagent` tool's.
    pub system_prompt: &'static str,
    /// Iteration budget. The harness has no hard per-run cap today (the agent loop runs
    /// until the model stops); the field mirrors the `subagent` tool's documented budget and
    /// is reserved for future enforcement.
    pub max_iterations: u32,
}

/// Default iteration budget, matching the `subagent` tool's "max 16 iterations" doc.
const DEFAULT_MAX_ITERATIONS: u32 = 16;

static SPECS: Lazy<[SubagentSpec; 5]> = Lazy::new(|| {
    [
        SubagentSpec {
            name: "explorer",
            description: "Read-only investigation: search and read local context.",
            system_prompt: "You are an exploration subagent dispatched by a coding agent. \
                            Gather facts from the codebase and context you are given. \
                            Stay focused on the prompt; return a concise findings summary.",
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "planner",
            description: "Read-only planning from local context.",
            system_prompt: "You are a planning subagent dispatched by a coding agent. \
                            Turn the given task into a concrete, step-by-step plan. \
                            Stay focused on the prompt; return the plan as your final answer.",
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "executor-coder",
            description: "Full coding agent: read/write/edit/bash plus git.",
            system_prompt: "You are a coding subagent dispatched by a coding agent. \
                            Implement the requested change using your tools. \
                            Stay focused on the prompt; return a concise summary of what you changed.",
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "checker",
            description: "Verification: read-only plus bash and git to check results.",
            system_prompt: "You are a verification subagent dispatched by a coding agent. \
                            Check the given work against the task and report pass/fail with evidence. \
                            Stay focused on the prompt; return a concise verdict.",
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "general",
            description: "Read-only research subagent (same as the subagent tool).",
            system_prompt: "You are a research subagent dispatched by a coding agent. \
                            Stay focused on the prompt; return a concise final answer.",
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
    ]
});

/// All built-in specs, in declaration order (explorer, planner, executor-coder, checker,
/// general).
pub fn builtin_specs() -> &'static [SubagentSpec] {
    &*SPECS
}

/// Names of all built-in specs, in declaration order.
pub fn builtin_spec_names() -> Vec<&'static str> {
    builtin_specs().iter().map(|s| s.name).collect()
}

/// Look up a spec by name. `None` for unknown names — the launcher fails the node.
pub fn resolve_spec(name: &str) -> Option<&'static SubagentSpec> {
    builtin_specs().iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn names_are_unique_and_ordered() {
        let names = builtin_spec_names();
        assert_eq!(
            names,
            vec![
                "explorer",
                "planner",
                "executor-coder",
                "checker",
                "general"
            ]
        );
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "duplicate spec names");
    }

    #[test]
    fn resolve_roundtrips_all_specs_and_rejects_unknown() {
        for spec in builtin_specs() {
            let resolved = resolve_spec(spec.name).expect("builtin name must resolve");
            assert_eq!(resolved.name, spec.name);
            assert_eq!(resolved.max_iterations, DEFAULT_MAX_ITERATIONS);
            assert!(!resolved.system_prompt.is_empty());
            assert!(!resolved.description.is_empty());
        }
        assert!(resolve_spec("no-such-agent").is_none());
        assert!(resolve_spec("").is_none());
    }
}
