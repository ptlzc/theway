//! Trigger engine — the host-side (CLI) implementation of external-event-driven agent
//! invocation, moved out of theway-core.
//!
//! The core runtime maintains state and exposes the agent loop (`AgentHarness`); the
//! trigger engine is a host concern: it owns the trigger envelope types, the dedup/cycle
//! runtime, the permission hook chain, the audit persistence (via core `Session` public
//! APIs), the sub-agent execution and result promotion (via core `Agent` public APIs),
//! and its own event stream for CLI listeners.
//!
//! Modules:
//! - [`types`] — trigger envelope, state machine, audit record, action/promotion types
//! - [`runtime`] — dedup window + cycle suppression engine (pure logic)
//! - [`notification_hook`] — transport-agnostic source adapter trait + status surface
//! - [`event`] — trigger lifecycle events for CLI listeners
//! - [`execution`] — [`TriggerExecutor`](execution::TriggerExecutor): the pipeline that
//!   replaces the old `AgentHarness::handle_trigger` (evaluate → permission → audit →
//!   sub-agent → promote)
//!
//! Transport adapters live in `crates/server/src/triggers` (MCP push, cron, dynamic) and
//! register with the executor via [`execution::TriggerExecutor::register_notification_hook`].

pub mod event;
pub mod execution;
pub mod notification_hook;
pub mod runtime;
pub mod types;
