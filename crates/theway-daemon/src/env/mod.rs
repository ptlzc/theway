//! Environment adapters — the `ExecutionEnv` implementations owned by the
//! daemon kernel (daemon-kernel-layers: the engine crate keeps the trait, the
//! kernel supplies the concrete implementations).
//!
//! [`native::NativeEnv`] (std::fs + tokio::process) is gated behind the `local`
//! feature: a `sandbox`-only build must not pull the OS-touching environment.

#[cfg(feature = "local")]
pub mod native;
