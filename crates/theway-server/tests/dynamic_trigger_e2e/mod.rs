//! End-to-end coverage for dynamic trigger creation from ordinary conversation.
//!
//! The model is simulated with a deterministic stream: the first user prompt creates a
//! dynamic rule via the model-facing `NewTrigger` tool, and a later runtime `Trigger`
//! causes the trigger sub-agent to call `bash` for the matching rule action.
//!
//! Layout: shared fixtures live in `helpers.rs`; the tests are split by domain into
//! `natural_language.rs` (creation from conversation), `promoted.rs` (chat promotion +
//! skill catalog inheritance), and `periodic.rs` (periodic hook, audit-only matches,
//! home fixture flow).

#[allow(dead_code)]
#[path = "../../src/auth.rs"]
pub mod auth;
#[allow(dead_code)]
#[path = "../../src/bug_report.rs"]
pub mod bug_report;
#[allow(dead_code)]
#[path = "../../src/config.rs"]
pub mod config;
#[allow(dead_code)]
#[path = "../../src/export.rs"]
pub mod export;
#[path = "../../src/triggers/mod.rs"]
#[allow(dead_code)]
pub mod triggers;
// The src `triggers` module is pulled in by path and references
// `crate::trigger_engine::...`; forward to the lib crate so all code sees ONE
// type identity (no duplicated path-include types).
pub mod trigger_engine {
    pub use theway::trigger_engine::*;
}

pub mod helpers;
mod natural_language;
mod periodic;
mod promoted;
