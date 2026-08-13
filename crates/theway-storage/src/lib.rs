//! theway-storage — concrete persistence backends for theway-core contracts.
//!
//! The engine (`theway-core`) defines the storage *contracts* (`SessionStorage`,
//! `DagPersistSink`) and ships lightweight defaults (memory / JSONL) so
//! embedders work out of the box. Heavier backends — SQLite via Turso — live
//! here, keeping the engine crate dependency-light and leaving the *choice* of
//! backend to the composition root (the `theway` server binary).
//!
//! One-way dependency: theway-storage → theway-core (traits + types). The core
//! never references this crate, so there is no cycle.

//! Self-alias so bridged unit tests (tests_bridge) and lib code share one path
//! shape (`theway_storage::…`), same pattern as theway-core / theway-daemon.
extern crate self as theway_storage;

pub mod session;
pub mod session_archive;
pub mod sqlite_dag;
pub mod sqlite_repo;
pub mod sqlite_storage;
