//! Engine tools — capabilities that support the harness runtime itself.
//!
//! One flat module (no runtime-tools / core-tools split): everything here is a core
//! (engine-crate) capability:
//!
//! - **graph / DAG orchestration**: [`dag_tools`] (model-facing dag_* tools), backed by
//!   `crate::multiagent::graph`.
//! - **subagents**: [`subagent`] (model-facing delegation tool). The runtime side
//!   (launch data channel, shared sub-harness lifecycle, DAG node execution) lives in
//!   `crate::multiagent`.
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
//! (no spec/tool content lives here; the app layer supplies both resolvers).

pub mod assembly;
pub mod dag_tools;
pub mod install_skill;
pub mod mcp_adapter;
pub mod memory;

pub mod remove_skill;
pub mod set_skill_state;
pub mod skill;
pub mod skill_builder;
pub mod subagent;
