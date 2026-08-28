//! Multi-agent runtime orchestration — everything above the single-agent runtime
//! (`crate::agent::assembly` / the bare `crate::agent`): spawning nested agent runs
//! ([`runner`]), subagent job tracking + live control ([`jobs`]), the run data
//! contract ([`types`]), the DAG/goal graph engine ([`graph`]), and the goal
//! mode hook ([`goal`]).
//!
//! Dependency direction: this module uses the single-agent runtime's public API
//! and never the reverse. The spec concept and the tool-set policy live app-side
//! (the daemon kernel: `agent_specs` + the tool assembly); model-facing tools live
//! in the daemon's `tools` module (`dag_tools`, `subagent`).
//!
//! ## Event plane
//!
//! This module owns the third event plane, [`jobs::SubagentJobEvent`], broadcast via
//! [`jobs::SubagentJobRegistry::subscribe`]. Its scope is multi-agent job telemetry
//! (graph mode): job start, streaming output chunks, per-turn metrics, and
//! completion status. It is independent of the single-agent planes
//! ([`LoopEvent`] in `crate::agent::run_loop` and [`SessionEvent`] in
//! `crate::agent::assembly`); external consumers wire all three through the
//! transport layer into a unified gRPC `StreamEvent`.

pub mod goal;
pub mod graph;
mod job_events;
mod job_metrics;
mod job_transcript;
pub mod jobs;
pub mod runner;
pub mod session_graph;
pub mod types;
