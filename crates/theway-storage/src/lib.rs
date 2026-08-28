//! theway-storage — concrete persistence backends for leaf contracts.
//!
//! Session persistence implements the raw reader/store interfaces from
//! `theway-contract`; runtime entry interpretation stays in `theway-core`.
//! SQLite via Turso stores one `<uuidv7>.db` file per session. The composition
//! root chooses the backend and adapts it to a core runtime session when needed.
//!
//! This crate depends only on leaf contracts, never on core or the transport
//! stack.

//! Self-alias so bridged unit tests (tests_bridge) and lib code share one path
//! shape (`theway_storage::…`), same pattern as theway-core / theway-daemon.
extern crate self as theway_storage;

pub mod session;
pub mod session_archive;
pub mod session_graph;
pub mod sqlite_dag;
pub mod sqlite_repo;
pub mod sqlite_storage;
