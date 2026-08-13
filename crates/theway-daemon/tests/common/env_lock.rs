//! Process-wide env-mutation lock shared by every bridged unit-test module that
//! mutates process env (`THEWAY_DIR`, provider API keys, base URLs, …).
//!
//! History (issue #16): `tests/commands/mod.rs` serialized its env mutations
//! with a local `static ENV_LOCK` while `tests/local_models/mod.rs` used its own
//! TokioMutex. Both modules are bridged into the same lib test binary
//! (`theway-daemon --lib`, via `tests_bridge!`), so the two locks never saw each
//! other: `AuthStore` resolves `THEWAY_DIR` at call time, and a concurrent
//! `THEWAY_DIR` swap from the other module made `model_credential_hint_*` fail
//! ~1/6 of runs. One process-wide lock fixes the race.
//!
//! Wiring: the daemon lib test target bridges this file from `src/lib.rs`
//! (`#[path] mod test_env`). Integration binaries that also pull env-mutating
//! modules bridge it at their own crate root (see `tests/commands_e2e_main.rs`).
//! Every binary is its own process (env is per-process), so no cross-binary
//! coordination is needed.

/// Serializes all env mutations in this test process across bridged modules.
///
/// Holding the guard across `.await` is safe in tests: `#[tokio::test]` runs on
/// a current-thread runtime, so the future never migrates worker threads. (The
/// workspace already allows `clippy::await_holding_lock` for this pattern.)
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores the previous env value (or its absence) on drop.
pub(crate) struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    pub(crate) fn remove(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}
