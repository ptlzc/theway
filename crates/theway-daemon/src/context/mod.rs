//! Daemon-side context assembly.
//!
//! Owns the context domain for the daemon: reading persisted collapse context,
//! rendering lineage / handoff blocks, and composing the system prompt for the
//! agent harness.

pub mod lineage;
pub mod session;
pub mod system_prompt;

#[cfg(test)]
tests_bridge_macro::tests_bridge!("context");
