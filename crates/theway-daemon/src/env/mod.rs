//! Environment adapters — the `ExecutionEnv` implementations owned by the
//! daemon kernel (daemon-kernel-layers: the engine crate keeps the trait, the
//! kernel supplies the concrete implementations).
//!
//! [`native::NativeEnv`] (std::fs + tokio::process) is always compiled. The
//! execution environment is selected at runtime by `[executor] kind` in
//! config.toml (issue #123); sandbox mode gates registration of the
//! host-touching tool bodies instead of compiling the implementation out.

pub mod native;
