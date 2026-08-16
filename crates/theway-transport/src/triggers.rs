//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Session-scoped automation data models — trigger rules cross the wire, so
//! they are contract.
//!
//! Issue #64: the models themselves now live in the pure leaf crate
//! `theway-contract` (`theway_contract::triggers`) so `theway-storage` can
//! share them without depending on the transport stack; this module keeps the
//! public path `theway_transport::triggers` via re-export.

pub use theway_contract::triggers::{
    CronJob, DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS, DynamicTriggerRule,
};
