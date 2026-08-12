//! `providers/images/` index. Mirrors `packages/ai/src/providers/images/`.

pub mod register_builtins;

#[cfg(feature = "openrouter-images")]
pub mod openrouter;
