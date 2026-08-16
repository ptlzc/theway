//! theway-storage — concrete persistence backends for theway-core contracts.
//!
//! The engine (`theway-core`) defines the storage *contracts* (`SessionStorage`,
//! `SessionRepo`, `DagPersistSink`) and ships an in-memory default so embedders
//! work out of the box. The durable backends — SQLite via Turso, one `<uuidv7>.db`
//! file per session — live here, keeping the engine crate dependency-light and
//! leaving the *choice* of backend to the composition root (the `thewayd` daemon
//! and the `theway` TUI binary).
//!
//! Dependencies: theway-core (traits + types) and theway-contract (session
//! sidecar models + base-dir/path layout). This crate never depends on
//! theway-transport; the core never references this crate, so there is no cycle.

//! Self-alias so bridged unit tests (tests_bridge) and lib code share one path
//! shape (`theway_storage::…`), same pattern as theway-core / theway-daemon.
extern crate self as theway_storage;

pub mod session;
pub mod session_archive;
pub mod sqlite_dag;
pub mod sqlite_repo;
pub mod sqlite_storage;
