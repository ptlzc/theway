//! theway-storage — concrete persistence backends for leaf contracts.
//!
//! Session persistence implements the raw reader/store interfaces from
//! `theway-contract`; runtime entry interpretation stays in `theway-core`.
//! SQLite via Turso stores one `<uuidv7>.db` file per session. The composition
//! root chooses the backend and adapts it to a core runtime session when needed.
//!
//! [`attachments`] implements the content-addressed attachment store contract as a
//! local `<root>/<shard>/<digest>` file tree: writes are deduplicated by digest and
//! committed by rename, reads re-verify the digest before returning bytes.
//!
//! This crate depends only on leaf contracts, never on core or the transport
//! stack.

// Kernel code propagates errors instead of unwrapping; `clippy.toml` exempts `#[cfg(test)]`
// modules and `#[test]` fns (`allow-unwrap-in-tests` / `allow-panic-in-tests`).
#![deny(clippy::unwrap_used)]
#![deny(clippy::panic)]
// The test build is exempt as a whole: rustc consumes a `#[cfg(test)]` written on a
// `tests_bridge!` invocation before expanding it, so bridged mirror modules never carry the
// attribute and clippy cannot recognise their fixture unwraps through `clippy.toml`. The plain
// library target still carries both denies above, so every non-test line stays gated.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::panic))]

//! Self-alias so bridged unit tests (tests_bridge) and lib code share one path
//! shape (`theway_storage::…`), same pattern as theway-core / theway-daemon.
extern crate self as theway_storage;

pub mod attachments;
pub mod session;
pub mod session_archive;
pub mod session_graph;
pub mod sqlite_dag;
pub mod sqlite_repo;
pub mod sqlite_storage;
