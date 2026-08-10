//! Multi-agent orchestration — everything above the single-agent runtime
//! (`crate::runtime::agent::agent_harness` / the bare `crate::agent`): spawning nested agent runs
//! ([`runner`]), the job registry + live control ([`registry`]), the run data
//! contract ([`types`]), the DAG/goal graph engine ([`graph`]), and the goal
//! mode hook ([`goal`]).
//!
//! Dependency direction: this module uses the single-agent runtime's public API
//! and never the reverse. The spec concept and the tool-set policy live app-side
//! (`theway` crate); model-facing tools live in `crate::tools` (`dag_tools`,
//! `subagent`).

pub mod goal;
pub mod graph;
pub mod registry;
pub mod runner;
pub mod types;
