//! App-layer subagent spec table — the spec CONCEPT lives here, not in the engine.
//!
//! The engine (`theway_core::tools::subagent_launch`) provides only the launch data
//! channel ([`SubagentLaunch`] + [`LaunchResolver`]); it does not define what a spec is.
//! This module owns the spec structure (name / description / system prompt / iteration
//! budget), the built-in table (explorer / planner / executor-coder / checker /
//! general), and the mapping into launch parameters injected into the engine. User-defined
//! specs via `~/.theway/subagents/*.toml` can extend this table later without engine
//! changes.

use std::sync::Arc;

use theway_core::runtime::subagents::types::{LaunchResolver, SubagentLaunch};

/// Iteration budget default, mirroring the `subagent` tool's "max 16 iterations" doc.
pub const DEFAULT_MAX_ITERATIONS: u32 = 16;

/// App-layer spec definition. Structure and content are server decisions; the engine
/// only ever sees the mapped [`SubagentLaunch`].
#[derive(Clone, Copy)]
pub struct SubagentSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub system_prompt: &'static str,
    pub max_iterations: u32,
}

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

/// App-layer launch resolver: spec name -> launch parameters (the one injected into the
/// subagent tool and the DAG node launcher). How specs map to launches is server policy.
pub fn launch_resolver() -> LaunchResolver {
    Arc::new(|name: &str| {
        SUBAGENT_SPECS
            .iter()
            .find(|s| s.name == name)
            .map(|s| SubagentLaunch {
                name: s.name,
                description: s.description,
                system_prompt: s.system_prompt,
                max_iterations: s.max_iterations,
            })
    })
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
            let launch = launch_resolver()(spec.name).expect("known name must resolve");
            assert_eq!(launch.name, spec.name);
            assert_eq!(launch.description, spec.description);
            assert_eq!(launch.system_prompt, spec.system_prompt);
            assert_eq!(launch.max_iterations, spec.max_iterations);
        }
        assert!(launch_resolver()("no-such-agent").is_none());
        assert!(launch_resolver()("").is_none());
    }
}
