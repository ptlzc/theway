use parking_lot::Mutex;

/// Lifecycle phase for one session-owned package instance. Registration
/// effects will share this owner boundary and are disposed before the phase
/// becomes `Disposed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InstanceLifecyclePhase {
    Loaded,
    Started,
    Disposed,
}

#[derive(Default)]
struct HealthState {
    consecutive_failures: usize,
    circuit_open: bool,
}

/// Per-session health state shared by synchronous dispatch and queued
/// observations for one extension instance.
#[derive(Default)]
pub(super) struct InstanceHealth {
    state: Mutex<HealthState>,
}

impl InstanceHealth {
    pub(super) fn is_open(&self) -> bool {
        self.state.lock().circuit_open
    }

    pub(super) fn record_success(&self) {
        self.state.lock().consecutive_failures = 0;
    }

    /// Returns true only for the transition that opens the circuit.
    pub(super) fn record_failure(&self, threshold: usize) -> bool {
        let mut state = self.state.lock();
        if state.circuit_open {
            return false;
        }
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures < threshold {
            return false;
        }
        state.circuit_open = true;
        true
    }
}
