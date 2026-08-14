//! Slash-command e2e tests, split by domain (file-size governance: >800 lines →
//! directory split, see docs/RUST_TEST_FILES.md).
//!
//! The `#[path]` include block that pulls in the `src/` modules under test lives in the
//! test-crate root (`commands_e2e_main.rs`), **not** here: the included modules reference
//! each other through `crate::...` paths, and the `commands` macros expand to
//! `$crate::commands::...`, so those modules must sit at the crate root. Domain modules
//! reach them via `use crate::...` and share fixtures through `helpers`.

pub mod control_plane;
pub mod file_commands;
pub mod goal;
pub mod helpers;
pub mod model_session;
pub mod skills;
pub mod triggers;
