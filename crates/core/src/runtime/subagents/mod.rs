//! Subagent runtime support: the data contract, the shared sub-harness runner,
//! the DAG node launcher, and the job registry + metrics (graph mode).
//!
//! This is the ENGINE side of subagents (capability); the spec concept and the tool-set
//! policy live app-side (`theway` crate). Model-facing tools live in
//! `crate::tools` (`dag_tools`, `subagent`).

pub mod node_launcher;
pub mod registry;
pub mod runner;
pub mod types;
