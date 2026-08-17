//! RFC 1 (issue #20) dedup window + cycle suppression engine.
//!
//! Pure logic, no IO. `AgentHarness::handle_trigger` (follow-up sub-PR) wraps this engine
//! into the agent loop entrypoint, but the dedup / cycle decisions live here so they are
//! independently testable.
//!
//! Behaviour matches RFC 1 §5:
//! - **Dedup window**: same `idempotency_key` seen twice within
//!   [`TriggerRuntimeConfig::dedup_window`] (default 5 minutes) → outcome depends on the
//!   *first* trigger's [`ReplacementPolicy`] (per RFC 1 §11 fixed decision #4 — sources
//!   declare per-event; the runtime trusts the first arrival's declaration to set the
//!   window's collapse semantics).
//! - **Cycle suppression**: when the same `trace_id` exceeds
//!   [`TriggerRuntimeConfig::cycle_hop_limit`] (default 5) → forced
//!   [`EvaluationOutcome::CycleSuppressed`]. Each accepted trigger bumps the per-trace hop
//!   counter; the runtime calls [`TriggerRuntime::record_follow_up_hop`] before spawning
//!   sub-triggers that share the parent's trace.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use super::types::{ReplacementPolicy, Trigger};

/// Tunable knobs for [`TriggerRuntime`]. The runtime never mutates these; callers can swap
/// them via [`TriggerRuntime::new`] or [`TriggerRuntime::with_config`].
#[derive(Clone, Copy, Debug)]
pub struct TriggerRuntimeConfig {
    /// How long after a successful admission the same `idempotency_key` is considered a
    /// duplicate. RFC 1 §5 default: 5 minutes. Capped at 24h to bound memory.
    pub dedup_window: Duration,
    /// Maximum `trace_id` chain depth before the runtime forces
    /// [`EvaluationOutcome::CycleSuppressed`]. RFC 1 §5 default: 5.
    pub cycle_hop_limit: u32,
}

impl TriggerRuntimeConfig {
    pub const DEFAULT_DEDUP_WINDOW: Duration = Duration::from_secs(5 * 60);
    pub const DEFAULT_CYCLE_HOP_LIMIT: u32 = 5;
    /// Upper bound enforced by [`TriggerRuntime::with_config`]; anything larger is clamped
    /// down because the dedup registry is in-memory and would otherwise grow unbounded.
    pub const MAX_DEDUP_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
}

impl Default for TriggerRuntimeConfig {
    fn default() -> Self {
        Self {
            dedup_window: Self::DEFAULT_DEDUP_WINDOW,
            cycle_hop_limit: Self::DEFAULT_CYCLE_HOP_LIMIT,
        }
    }
}

/// Result of running a [`Trigger`] through [`TriggerRuntime::evaluate`]. Subsequent runtime
/// state (state machine transitions, session audit, permission evaluator) consumes this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvaluationOutcome {
    /// First admission of this `idempotency_key` in the current dedup window AND the
    /// `trace_id` is still within `cycle_hop_limit`. The runtime should advance the state
    /// machine to `Accepted` (subject to subsequent permission evaluation).
    Accept,
    /// The same `idempotency_key` has been seen before in the dedup window. The first
    /// trigger's `ReplacementPolicy` decides what happens; the runtime's audit record
    /// captures the previous `trace_id` so the user can correlate which event "won".
    Deduped {
        replacement_policy: ReplacementPolicy,
        previous_trace_id: String,
    },
    /// Cycle suppression fired: the `trace_id` has already passed through this runtime
    /// `hop_count` times, exceeding `cycle_hop_limit`.
    CycleSuppressed { hop_count: u32 },
}

/// In-memory dedup + cycle registry shared across all `NotificationHook` sources for a
/// single agent / daemon. Cloning is cheap; the actual state lives behind an `Arc<Mutex>`.
#[derive(Clone, Debug)]
pub struct TriggerRuntime {
    inner: std::sync::Arc<Mutex<Inner>>,
    config: TriggerRuntimeConfig,
}

impl Default for TriggerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Inner {
    /// `idempotency_key` → first-arrival entry. Pruned lazily on every [`evaluate`].
    dedup: HashMap<String, DedupEntry>,
    /// `trace_id` → hop count. Lazy pruning is not safe here (we cannot tell when a trace
    /// is "done"), so we cap each entry's lifetime to one cycle window (= `dedup_window`,
    /// reused for simplicity) and prune the same way as the dedup map.
    cycle: HashMap<String, CycleEntry>,
    /// Monotonic counters surfaced through [`TriggerRuntime::snapshot`] for TUI / `/triggers`
    /// observability. These never decrement and survive entry pruning.
    deduped_total: u64,
    cycle_suppressed_total: u64,
    accepted_total: u64,
}

/// Point-in-time view of the runtime's dedup + cycle bookkeeping. Cheap to copy; surfaced
/// via `TriggerExecutor::notification_status_snapshot` for status banners and `/triggers`
/// rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriggerRuntimeSnapshot {
    /// Number of distinct `idempotency_key` entries currently inside the dedup window.
    pub dedup_entries: usize,
    /// Number of distinct `trace_id` chains currently inside the cycle window.
    pub active_traces: usize,
    /// Lifetime count of triggers that admitted (advanced the dedup map + cycle counter).
    pub accepted_total: u64,
    /// Lifetime count of triggers that were dropped because their `idempotency_key`
    /// matched an entry still inside the dedup window.
    pub deduped_total: u64,
    /// Lifetime count of triggers that were dropped because their `trace_id` exceeded
    /// `cycle_hop_limit`.
    pub cycle_suppressed_total: u64,
}

#[derive(Clone, Debug)]
struct DedupEntry {
    received_at: DateTime<Utc>,
    replacement_policy: ReplacementPolicy,
    trace_id: String,
}

#[derive(Clone, Debug)]
struct CycleEntry {
    last_seen_at: DateTime<Utc>,
    hop_count: u32,
}

impl TriggerRuntime {
    /// Construct a runtime with [`TriggerRuntimeConfig::default`].
    pub fn new() -> Self {
        Self::with_config(TriggerRuntimeConfig::default())
    }

    /// Construct a runtime with a custom config. `dedup_window` is clamped to
    /// [`TriggerRuntimeConfig::MAX_DEDUP_WINDOW`].
    pub fn with_config(mut config: TriggerRuntimeConfig) -> Self {
        if config.dedup_window > TriggerRuntimeConfig::MAX_DEDUP_WINDOW {
            config.dedup_window = TriggerRuntimeConfig::MAX_DEDUP_WINDOW;
        }
        Self {
            inner: std::sync::Arc::new(Mutex::new(Inner {
                dedup: HashMap::new(),
                cycle: HashMap::new(),
                deduped_total: 0,
                cycle_suppressed_total: 0,
                accepted_total: 0,
            })),
            config,
        }
    }

    /// Point-in-time view of the dedup / cycle bookkeeping plus lifetime counters. Intended
    /// for status banners; cheap (one mutex lock + struct copy). Lifetime counters never
    /// decrement so consumers can build delta UIs without missing intermediate events.
    pub fn snapshot(&self) -> TriggerRuntimeSnapshot {
        let inner = self.inner.lock();
        TriggerRuntimeSnapshot {
            dedup_entries: inner.dedup.len(),
            active_traces: inner.cycle.len(),
            accepted_total: inner.accepted_total,
            deduped_total: inner.deduped_total,
            cycle_suppressed_total: inner.cycle_suppressed_total,
        }
    }

    /// Convenience getter for the active configuration. Useful in tests and status output.
    pub fn config(&self) -> TriggerRuntimeConfig {
        self.config
    }

    /// Decide whether a fresh trigger should be admitted, deduped, or cycle-suppressed.
    /// Pure (modulo wall-clock pruning); does NOT advance the trigger state machine —
    /// that's the harness's job after it sees the outcome.
    ///
    /// Side effects (when the outcome is [`EvaluationOutcome::Accept`]):
    /// - inserts the `idempotency_key` → first-arrival entry into the dedup map
    /// - bumps the `trace_id` hop counter
    ///
    /// On [`EvaluationOutcome::Deduped`] or [`EvaluationOutcome::CycleSuppressed`] the
    /// internal maps are *not* mutated for that trigger (the prior entry stands; the cycle
    /// counter does not advance on a suppressed trigger).
    pub fn evaluate(&self, trigger: &Trigger) -> EvaluationOutcome {
        let mut inner = self.inner.lock();
        let now = trigger.received_at;

        prune_expired(&mut inner.dedup, now, self.config.dedup_window);
        prune_expired_cycle(&mut inner.cycle, now, self.config.dedup_window);

        // Dedup check runs first because a duplicate event is never "real" for cycle
        // counting — we do not want a deduped event to consume hop budget.
        if let Some(prev) = inner.dedup.get(&trigger.idempotency_key) {
            let outcome = EvaluationOutcome::Deduped {
                replacement_policy: prev.replacement_policy,
                previous_trace_id: prev.trace_id.clone(),
            };
            inner.deduped_total = inner.deduped_total.saturating_add(1);
            return outcome;
        }

        // Cycle check runs against the trace counter as it stands BEFORE this trigger; if
        // we are already at the limit, suppress without advancing.
        if let Some(existing) = inner.cycle.get(&trigger.trace_id) {
            if existing.hop_count >= self.config.cycle_hop_limit {
                let outcome = EvaluationOutcome::CycleSuppressed {
                    hop_count: existing.hop_count,
                };
                inner.cycle_suppressed_total = inner.cycle_suppressed_total.saturating_add(1);
                return outcome;
            }
        }

        // Admit. Record both the dedup entry and the hop bump in one atomic critical section.
        inner.dedup.insert(
            trigger.idempotency_key.clone(),
            DedupEntry {
                received_at: now,
                replacement_policy: trigger.replacement_policy,
                trace_id: trigger.trace_id.clone(),
            },
        );
        inner
            .cycle
            .entry(trigger.trace_id.clone())
            .and_modify(|e| {
                e.hop_count = e.hop_count.saturating_add(1);
                e.last_seen_at = now;
            })
            .or_insert(CycleEntry {
                hop_count: 1,
                last_seen_at: now,
            });
        inner.accepted_total = inner.accepted_total.saturating_add(1);

        EvaluationOutcome::Accept
    }

    /// Record an additional hop on `trace_id` without going through dedup. Called by the
    /// harness immediately before spawning a follow-up trigger that inherits the parent's
    /// trace (e.g. an `AgentDelegate` trigger emitted by a tool call).
    ///
    /// `now` is wall-clock time at the moment the follow-up is queued; used both to bump
    /// the entry's `last_seen_at` and to drive lazy pruning of stale trace entries.
    pub fn record_follow_up_hop(&self, trace_id: &str, now: DateTime<Utc>) {
        let mut inner = self.inner.lock();
        prune_expired_cycle(&mut inner.cycle, now, self.config.dedup_window);
        inner
            .cycle
            .entry(trace_id.to_string())
            .and_modify(|e| {
                e.hop_count = e.hop_count.saturating_add(1);
                e.last_seen_at = now;
            })
            .or_insert(CycleEntry {
                hop_count: 1,
                last_seen_at: now,
            });
    }

    /// Test helper: snapshot the current dedup map size. Public for white-box tests; not
    /// part of the public surface users build against.
    #[cfg(test)]
    pub(crate) fn dedup_entry_count(&self) -> usize {
        self.inner.lock().dedup.len()
    }

    /// Test helper: snapshot the current trace map size.
    #[cfg(test)]
    pub(crate) fn cycle_entry_count(&self) -> usize {
        self.inner.lock().cycle.len()
    }
}

fn prune_expired(map: &mut HashMap<String, DedupEntry>, now: DateTime<Utc>, window: Duration) {
    let cutoff =
        now - chrono::Duration::from_std(window).expect("dedup_window fits in chrono::Duration");
    map.retain(|_, entry| entry.received_at >= cutoff);
}

fn prune_expired_cycle(
    map: &mut HashMap<String, CycleEntry>,
    now: DateTime<Utc>,
    window: Duration,
) {
    let cutoff =
        now - chrono::Duration::from_std(window).expect("dedup_window fits in chrono::Duration");
    map.retain(|_, entry| entry.last_seen_at >= cutoff);
}

#[cfg(test)]
// Test files live in `tests/trigger_engine/runtime/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("trigger_engine/runtime");
