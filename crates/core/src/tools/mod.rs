//! Engine tools — capabilities that support the harness runtime itself.
//!
//! One flat module (no runtime-tools / core-tools split): everything here is a core
//! (engine-crate) capability:
//!
//! - **graph / DAG orchestration**: [`dag_tools`] (model-facing dag_* tools), backed by
//!   `crate::runtime::graph_engineering`.
//! - **subagents**: [`subagent_specs`] (built-in agent specs), [`subagent_runner`]
//!   (shared sub-harness lifecycle), [`node_launcher`] (DAG node execution), [`subagent`]
//!   (model-facing Task delegation tool).
//! - **skills**: [`skill`], [`install_skill`], [`remove_skill`], [`set_skill_state`],
//!   [`skill_builder`].
//! - **memory**: [`memory`].
//! - **MCP**: [`mcp_adapter`] (wraps MCP-server tools as `AgentTool`s).
//!
//! Local-execution tools (bash / shell / fs / git / grep / find / ls / outline / truncate)
//! and web tools live in the application layer (the `theway` server crate,
//! `crate::tools` there): they are environment-specific agent capabilities — the local
//! ones may become remote sandbox execution later — not engine concerns. The engine
//! decides *what* a subagent can do only through injected tool-set resolvers
//! (`SubagentSpec` carries no tool factory; the app layer supplies one).

pub mod assembly;
pub mod dag_tools;
pub mod install_skill;
pub mod mcp_adapter;
pub mod memory;
pub mod node_launcher;
pub mod remove_skill;
pub mod set_skill_state;
pub mod skill;
pub mod skill_builder;
pub mod subagent_runner;
pub mod subagent_specs;
pub mod subagent;
