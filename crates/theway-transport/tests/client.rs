//! Integration-test entry point for the client suite.
//!
//! The canonical client tests live in `tests/client/mod.rs` and are bridged
//! into the library's unit-test target by `client.rs` (see
//! docs/rust-test-files.md). This file also exposes them as a normal
//! `cargo test --test client` target so the suite can be run explicitly from
//! the integration-test harness.
//!
//! The module uses `crate::` paths because it is normally compiled inside the
//! library crate; this shim re-exports the same public paths from the library
//! crate at the integration-test crate root.

pub use futures::{Stream, StreamExt};
pub use theway_transport::client::*;
pub use theway_transport::*;

#[path = "client/mod.rs"]
mod client_suite;