//! Built-in tools. Modeled on `packages/coding-agent/src/core/tools/` (the TS implementation):
//! same names, same parameter shapes, simpler bodies. Each tool implements
//! [`theway_core::AgentTool`].
//!
//! Split (openspec tools-into-core): the tool BODIES live here in the engine crate; the
//! injection-style ASSEMBLY (session-stamped / harness-cell-wired constructors like
//! `session_tool_set`, `task_tool`, the skill family, cron/trigger builders) stays in the
//! application layer (`theway` crate, `src/tools.rs`). The two set constructors below stay
//! here because engine code (`subagent_specs`' built-in tool-set factories) consumes them
//! directly and they need no runtime injection.

pub mod dag_tools;
pub mod install_skill;
pub mod memory;
pub mod node_launcher;
pub mod remove_skill;
pub mod set_skill_state;
pub mod skill;
pub mod skill_builder;
pub mod subagent_runner;
pub mod subagent_specs;
pub mod task;
