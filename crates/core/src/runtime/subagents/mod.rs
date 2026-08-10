//! Subagent runtime — the base capability: the data contract ([`types`]), the shared
//! sub-harness runner ([`runner`]), and the job registry + metrics ([`registry`]).
//!
//! Zero dependencies upward: orchestrators (the DAG engine in
//! `crate::runtime::graph_engineering`) build on top of this module, never the other
//! way. The spec concept and the tool-set policy live app-side (`theway` crate);
//! model-facing tools live in `crate::tools` (`dag_tools`, `subagent`).

pub mod registry;
pub mod runner;
pub mod types;
