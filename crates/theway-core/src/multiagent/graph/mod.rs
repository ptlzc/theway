//! The graph engine: the graph model (mermaid parse/render, validation,
//! dependency reconciliation), the scheduler/state machine ([`engine`]), and the
//! engine's default node executor ([`node_launcher`] — agent-based, running on
//! top of [`super::runner`]). One engine serves both `RunKind::Dag` (dag_* tools)
//! and `RunKind::Goal` (goal mode's special graph).
//!
//! Dependency direction: this module orchestrates agent runs, so it may use the
//! rest of `multiagent`; the reverse is not allowed.

pub mod engine;
pub mod model;
pub mod node_launcher;
pub mod persist;
pub mod types;
