//! Environment adapters. The harness IO surface ([`crate::harness::types::ExecutionEnv`]) is a
//! trait; each adapter implements it for a concrete platform.
//!
//! Currently only `native` (std::fs + tokio::process) ships; a browser/sandbox adapter would
//! plug in alongside.

#[cfg(feature = "native-env")]
pub mod native;
