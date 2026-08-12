//! Thin entry point for the dynamic trigger e2e suite; the body lives in
//! `dynamic_trigger_e2e/`, split by test domain.

#[path = "dynamic_trigger_e2e/mod.rs"]
mod dynamic_trigger_e2e;

// The `#[path]`-included src modules reference `crate::auth`, `crate::config`, etc.;
// re-export them at the test crate root so those absolute paths resolve.
pub use dynamic_trigger_e2e::{auth, bug_report, config, export, trigger_engine, triggers};
pub use theway_transport::inbox;
