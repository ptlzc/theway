//! App-layer subagent spec table — the BEHAVIOR content of the built-in subagent
//! specs (explorer / planner / executor-coder / checker / general).
//!
//! The engine (`theway_core::tools::subagent_specs`) defines the mechanism (the
//! [`SubagentSpec`] structure and the [`SpecResolver`] injection point) but not the
//! content; which specs exist and what they are told to do is an app-layer decision,
//! same line as the tool-set resolver. User-defined specs via
//! `~/.theway/subagents/*.toml` can replace this table later without engine changes.

use std::sync::Arc;

use theway_core::tools::subagent_specs::{DEFAULT_MAX_ITERATIONS, SpecResolver, SubagentSpec};

/// The built-in spec table, in declaration order.
pub static SUBAGENT_SPECS: [SubagentSpec; 5] = [
    SubagentSpec {
        name: "explorer",
        description: "Investigation: search and read local context.",
        system_prompt: "You are an exploration subagent dispatched by a coding agent. \
                        Gather facts from the codebase and context you are given. \
                        Stay focused on the prompt; return a concise findings summary.",
        max_iterations: DEFAULT_MAX_ITERATIONS,
    },
    SubagentSpec {
        name: "planner",
        description: "Planning from local context.",
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
        description: "Verification: check results with evidence.",
        system_prompt: "You are a verification subagent dispatched by a coding agent. \
                        Check the given work against the task and report pass/fail with evidence. \
                        Stay focused on the prompt; return a concise verdict.",
        max_iterations: DEFAULT_MAX_ITERATIONS,
    },
    SubagentSpec {
        name: "general",
        description: "General-purpose research subagent (same as the subagent tool).",
        system_prompt: "You are a research subagent dispatched by a coding agent. \
                        Stay focused on the prompt; return a concise final answer.",
        max_iterations: DEFAULT_MAX_ITERATIONS,
    },
];

/// App-layer spec resolver: spec name -> spec (the one injected into the subagent tool
/// and the DAG node launcher).
pub fn spec_resolver() -> SpecResolver {
    Arc::new(|name: &str| SUBAGENT_SPECS.iter().find(|s| s.name == name).copied())
}

/// Known spec names, in declaration order (populates the `subagent_type` enum and the
/// DAG `agent` validation).
pub fn spec_names() -> Vec<String> {
    SUBAGENT_SPECS.iter().map(|s| s.name.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn names_are_unique_and_ordered() {
        let names = spec_names();
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
        let unique: HashSet<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(unique.len(), names.len(), "duplicate spec names");
    }

    #[test]
    fn resolver_roundtrips_all_specs_and_rejects_unknown() {
        for spec in &SUBAGENT_SPECS {
            let resolved = spec_resolver()(spec.name).expect("known name must resolve");
            assert_eq!(resolved.name, spec.name);
            assert_eq!(resolved.max_iterations, DEFAULT_MAX_ITERATIONS);
            assert!(!resolved.system_prompt.is_empty());
            assert!(!resolved.description.is_empty());
        }
        assert!(spec_resolver()("no-such-agent").is_none());
        assert!(spec_resolver()("").is_none());
    }
}
