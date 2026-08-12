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

pub mod hybrid_repo;
pub mod sqlite_dag;
pub mod sqlite_repo;
pub mod sqlite_storage;
