//! DAG orchestration: the graph model (mermaid parse/render, validation,
//! dependency reconciliation), the engine (scheduler/state machine), and the
//! engine's default node executor ([`node_launcher`] — subagent-based, running on
//! top of `crate::runtime::subagents`).
//!
//! Dependency direction: this module orchestrates subagents, so it may use
//! `runtime::subagents` (the base capability); the reverse is not allowed.

pub mod engine;
pub mod graph;
pub mod node_launcher;
pub mod persist;
pub mod types;
