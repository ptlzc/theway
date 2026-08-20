use std::sync::atomic::{AtomicU64, Ordering};

/// Session-local sequence used for host-owned load/start/shutdown events and
/// direct dispatcher tests. Core lifecycle invocations retain their own
/// allocator when the runtime port adapter is attached.
#[derive(Debug)]
pub(super) struct HostLifecycleSequence(AtomicU64);

impl Default for HostLifecycleSequence {
    fn default() -> Self {
        Self(AtomicU64::new(1))
    }
}

impl HostLifecycleSequence {
    pub(super) fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}
