//! Native entry point. Mirrors `packages/agent/src/node.ts`. Re-exports the native env adapter
//! so the common consumer path is `use theway_core::node::*`.

#[cfg(feature = "native-env")]
pub use crate::harness::env::native::NativeEnv;
