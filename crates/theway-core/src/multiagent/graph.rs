//! The graph engine: DAG validation and dependency reconciliation ([`model`]),
//! Mermaid parsing/rendering ([`mermaid`]), the scheduler/state machine
//! ([`engine`]), and the default node executor ([`node_launcher`]). One engine
//! serves both `RunKind::Dag` and `RunKind::Goal`.
//!
//! Dependency direction: this module orchestrates agent runs, so it may use the
//! rest of `multiagent`; the reverse is not allowed.

pub mod engine;
pub mod mermaid;
pub mod model;
pub mod node_launcher;
pub mod persist;
pub mod types;

mod engine_state;
mod observability;
mod scheduler;
