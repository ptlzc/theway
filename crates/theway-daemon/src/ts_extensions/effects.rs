/// Lifecycle phase for one session-owned package instance. Registration
/// effects will share this owner boundary and are disposed before the phase
/// becomes `Disposed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InstanceLifecyclePhase {
    Loaded,
    Started,
    Disposed,
}
