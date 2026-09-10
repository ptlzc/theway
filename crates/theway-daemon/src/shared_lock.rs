//! Poison-tolerant access to the daemon's shared `std::sync` locks.
//!
//! The daemon's shared views — the `WireDaemonConfig` / `WirePathContext` slots handed to
//! `TransportEndpoints`, the provisioned skill/template catalogs, the per-session MCP provision
//! slot, and the background-shell registry — are written under a single lock by the serialized
//! event loop, and their readers have no error channel to report a poisoned lock through.
//! Recovering the guard keeps the settings RPCs, the snapshot builders, and the shell tools serving
//! the last written value instead of panicking on a lock another thread left poisoned.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Read side of a shared `RwLock`, recovered from a poisoned lock.
pub(crate) fn read_lock<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Write side of a shared `RwLock`, with the same poison recovery as `read_lock`.
pub(crate) fn write_lock<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Lock a shared `Mutex`, with the same poison recovery as `read_lock`.
pub(crate) fn lock_mutex<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}
