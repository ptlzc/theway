//! `subagent_specs` — built-in subagent specifications for DAG node execution.
//!
//! Rust-side counterpart of the TS `.pi/subagents/config.yaml` agent registry the
//! dag-orchestrator extension resolves DAG node `agent` fields against. Each spec names a
//! built-in agent type, a short system prompt, a tool-set factory and an iteration budget.
//! [`node_launcher`](super::node_launcher) resolves a node's `agent` field through
//! [`resolve_spec`]; unknown names fail the node with `unknown agent "..."` (mirrors the
//! TS `defaultLauncher`).
//!
//! v1 scope: static built-in table only. User-defined specs via `~/.theway/subagents/*.toml`
//! are a follow-up (same line as the `task` tool's issue #11 out-of-scope note).

#![allow(dead_code)] // consumed by node_launcher/dag_tools (p3c-wire) once wired into the binary.

use std::sync::Arc;

use once_cell::sync::Lazy;
use theway_core::AgentTool;

/// Static description of a built-in subagent.
pub struct SubagentSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// Short (1–3 sentences) system prompt, same style as the `task` tool's.
    pub system_prompt: &'static str,
    /// Tool-set factory, reused at every launch so each node starts with the same tools.
    pub tools: fn() -> Vec<Arc<dyn AgentTool>>,
    /// Iteration budget. The harness has no hard per-run cap today (the agent loop runs
    /// until the model stops); the field mirrors the `task` tool's documented budget and
    /// is reserved for future enforcement.
    pub max_iterations: u32,
}

/// Default iteration budget, matching the `task` tool's "max 16 iterations" doc.
const DEFAULT_MAX_ITERATIONS: u32 = 16;

static SPECS: Lazy<[SubagentSpec; 5]> = Lazy::new(|| {
    [
        SubagentSpec {
            name: "explorer",
            description: "Read-only investigation: search, read, and web lookups.",
            system_prompt: "You are an exploration subagent dispatched by a coding agent. \
                            Gather facts from the codebase and context you are given. \
                            Stay focused on the prompt; return a concise findings summary.",
            tools: explorer_tools,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "planner",
            description: "Read-only planning from local context (no web).",
            system_prompt: "You are a planning subagent dispatched by a coding agent. \
                            Turn the given task into a concrete, step-by-step plan. \
                            Stay focused on the prompt; return the plan as your final answer.",
            tools: planner_tools,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "executor-coder",
            description: "Full coding agent: read/write/edit/bash plus web and git.",
            system_prompt: "You are a coding subagent dispatched by a coding agent. \
                            Implement the requested change using your tools. \
                            Stay focused on the prompt; return a concise summary of what you changed.",
            tools: executor_coder_tools,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "checker",
            description: "Verification: read-only plus bash and git to check results.",
            system_prompt: "You are a verification subagent dispatched by a coding agent. \
                            Check the given work against the task and report pass/fail with evidence. \
                            Stay focused on the prompt; return a concise verdict.",
            tools: checker_tools,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        },
        SubagentSpec {
            name: "general",
            description: "Read-only research subagent (same as the task tool).",
            system_prompt: "You are a research subagent dispatched by a coding agent. \
                            Stay focused on the prompt; return a concise final answer.",
            tools: general_tools,
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

// ── tool-set factories ──────────────────────────────────────────────────────

fn explorer_tools() -> Vec<Arc<dyn AgentTool>> {
    let mut tools = theway_core::tools::subagent_read_only_tools();
    tools.push(Arc::new(
        theway_core::tools::web_search::WebSearchTool::new(),
    ));
    tools
}

fn planner_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(theway_core::tools::read::ReadTool),
        Arc::new(theway_core::tools::ls::LsTool),
        Arc::new(theway_core::tools::grep::GrepTool),
        Arc::new(theway_core::tools::find::FindTool),
    ]
}

fn executor_coder_tools() -> Vec<Arc<dyn AgentTool>> {
    // Same store as the parent agent: DAG subagents share the parent's memory dir.
    theway_core::tools::default_tools(default_memory_dir())
}

/// The theway memory dir: `${THEWAY_DIR:-$HOME/.theway}/memory`. Inlined (not via the CLI's
/// `config` module, which lives one layer up) so this module stays engine-self-contained and
/// can be pulled into integration tests through `#[path]` includes. Mirrors the CLI's
/// `config::memory_dir()`.
fn default_memory_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("THEWAY_DIR") {
        return std::path::PathBuf::from(p).join("memory");
    }
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".theway").join("memory"))
        .unwrap_or_else(|| std::path::PathBuf::from(".theway").join("memory"))
}

fn checker_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(theway_core::tools::read::ReadTool),
        Arc::new(theway_core::tools::ls::LsTool),
        Arc::new(theway_core::tools::grep::GrepTool),
        Arc::new(theway_core::tools::find::FindTool),
        Arc::new(theway_core::tools::bash::BashTool),
        Arc::new(theway_core::tools::git::GitTool),
    ]
}

fn general_tools() -> Vec<Arc<dyn AgentTool>> {
    theway_core::tools::subagent_read_only_tools()
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

    #[test]
    fn tool_sets_are_non_empty() {
        for spec in builtin_specs() {
            let tools = (spec.tools)();
            assert!(
                !tools.is_empty(),
                "{} must have a non-empty tool set",
                spec.name
            );
            for tool in &tools {
                assert!(!tool.label().is_empty());
            }
        }
    }

    #[test]
    fn explorer_and_general_are_read_only_plus_web() {
        let labels = |spec: &SubagentSpec| {
            let mut l: Vec<String> = (spec.tools)()
                .iter()
                .map(|t| t.label().to_string())
                .collect();
            l.sort();
            l
        };
        let general = labels(resolve_spec("general").unwrap());
        assert_eq!(
            general,
            vec!["find", "git", "grep", "ls", "read", "web_fetch"]
        );
        let explorer = labels(resolve_spec("explorer").unwrap());
        assert_eq!(
            explorer,
            vec![
                "find",
                "git",
                "grep",
                "ls",
                "read",
                "web_fetch",
                "web_search"
            ]
        );
    }
}
