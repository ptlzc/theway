//! Benchmarks for the event dispatch architecture.
//!
//! Verification targets:
//! - Sync callbacks: <1µs hard constraint (emit_sync_only)
//! - Broadcast: non-blocking send (broadcast_multi_receiver)
//! - Three-segment dispatch vs legacy for-await: the three-segment design
//!   separates sync observers (<1µs, memory-only) from async I/O listeners
//!   and broadcast subscribers, whereas the legacy pattern sequentially
//!   awaits all listeners, coupling fast observers to slow I/O.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use theway_core::multiagent::registry::{AgentJobRegistry, JobInit, JobStatus};
use theway_core::{LoopEvent, LoopListener, LoopSyncCallback};

// ── a. emit_three_segment ──────────────────────────────────────────────────
//
// Replicates the three-segment dispatch from `crate::agent::run_loop::utils::emit`:
//   1. 3 sync callbacks (atomic add, simulating cost tracker / metrics)
//   2. 1 async await listener (no-op future, simulating persistence)
//   3. broadcast::Sender<LoopEvent> capacity 256 → send (1 receiver)

fn bench_emit_three_segment(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("emit_three_segment", |b| {
        // 3 sync callbacks: atomic add (cost tracker emulation).
        let c1 = Arc::new(AtomicU64::new(0));
        let c2 = Arc::new(AtomicU64::new(0));
        let c3 = Arc::new(AtomicU64::new(0));
        let sync_callbacks: Arc<Mutex<Vec<LoopSyncCallback>>> = {
            let c1 = c1.clone();
            let c2 = c2.clone();
            let c3 = c3.clone();
            Arc::new(Mutex::new(vec![
                Arc::new(move |_: &LoopEvent| {
                    c1.fetch_add(1, Ordering::Relaxed);
                }),
                Arc::new(move |_: &LoopEvent| {
                    c2.fetch_add(1, Ordering::Relaxed);
                }),
                Arc::new(move |_: &LoopEvent| {
                    c3.fetch_add(1, Ordering::Relaxed);
                }),
            ]))
        };

        // 1 async await listener: no-op future (persistence emulation).
        let noop_listener: LoopListener = Arc::new(
            |_event: LoopEvent,
             _cancel: CancellationToken|
             -> Pin<Box<dyn Future<Output = ()> + Send>> { Box::pin(async {}) },
        );
        let await_listeners: Arc<Mutex<Vec<LoopListener>>> =
            Arc::new(Mutex::new(vec![noop_listener]));

        // Broadcast capacity 256, 1 receiver.
        let (tx, _rx) = tokio::sync::broadcast::channel::<LoopEvent>(256);

        let event = LoopEvent::TurnStart;

        b.iter(|| {
            rt.block_on(async {
                // Segment 1: sync callbacks (catch_unwind per callback).
                let cbs = sync_callbacks.lock().clone();
                for cb in &cbs {
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| cb(&event)));
                }

                // Segment 2: async await listeners.
                let cancel = CancellationToken::new();
                for listener in await_listeners.lock().iter() {
                    listener(event.clone(), cancel.clone()).await;
                }

                // Segment 3: broadcast (non-blocking send).
                let _ = tx.send(event.clone());
            });
            black_box(());
        });
    });
}

// ── b. emit_legacy_for_await ───────────────────────────────────────────────
//
// Legacy dispatch pattern: Vec<async listener>, sequential clone + await.
// 2 listeners: one no-op, one atomic add.
// Contrast with emit_three_segment — the legacy pattern couples fast sync
// observers to async I/O via sequential await.

fn bench_emit_legacy_for_await(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("emit_legacy_for_await", |b| {
        let counter = Arc::new(AtomicU64::new(0));

        let listeners: Vec<LoopListener> = {
            let counter = counter.clone();
            vec![
                // no-op
                Arc::new(
                    |_event: LoopEvent,
                     _cancel: CancellationToken|
                     -> Pin<Box<dyn Future<Output = ()> + Send>> {
                        Box::pin(async {})
                    },
                ),
                // atomic add
                Arc::new(
                    move |_event: LoopEvent,
                          _cancel: CancellationToken|
                          -> Pin<Box<dyn Future<Output = ()> + Send>> {
                        let c = counter.clone();
                        Box::pin(async move {
                            c.fetch_add(1, Ordering::Relaxed);
                        })
                    },
                ),
            ]
        };

        let event = LoopEvent::TurnStart;

        b.iter(|| {
            rt.block_on(async {
                let cancel = CancellationToken::new();
                for listener in &listeners {
                    listener(event.clone(), cancel.clone()).await;
                }
            });
            black_box(());
        });
    });
}

// ── c. emit_sync_only ──────────────────────────────────────────────────────
//
// Bare sync path: 3 sync callbacks, no await / broadcast.
// This measures the minimal dispatch overhead and validates
// the <1µs hard constraint on sync observers.

fn bench_emit_sync_only(c: &mut Criterion) {
    c.bench_function("emit_sync_only", |b| {
        let c1 = Arc::new(AtomicU64::new(0));
        let c2 = Arc::new(AtomicU64::new(0));
        let c3 = Arc::new(AtomicU64::new(0));
        let sync_callbacks: Arc<Mutex<Vec<LoopSyncCallback>>> = {
            let c1 = c1.clone();
            let c2 = c2.clone();
            let c3 = c3.clone();
            Arc::new(Mutex::new(vec![
                Arc::new(move |_: &LoopEvent| {
                    c1.fetch_add(1, Ordering::Relaxed);
                }),
                Arc::new(move |_: &LoopEvent| {
                    c2.fetch_add(1, Ordering::Relaxed);
                }),
                Arc::new(move |_: &LoopEvent| {
                    c3.fetch_add(1, Ordering::Relaxed);
                }),
            ]))
        };

        let event = LoopEvent::TurnStart;

        b.iter(|| {
            let cbs = sync_callbacks.lock().clone();
            for cb in &cbs {
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| cb(&event)));
            }
            black_box(());
        });
    });
}

// ── d. broadcast_multi_receiver ────────────────────────────────────────────
//
// Broadcast send overhead: 1 receiver vs 10 receivers, capacity 256.
// Validates non-blocking send — cost difference between 1 and 10
// receivers should be small (single clone + per-receiver position bump).

fn bench_broadcast_multi_receiver(c: &mut Criterion) {
    let event = LoopEvent::TurnStart;

    c.bench_function("broadcast_1_receiver", |b| {
        let (tx, _rx) = tokio::sync::broadcast::channel::<LoopEvent>(256);
        b.iter(|| {
            let _ = tx.send(black_box(event.clone()));
        });
    });

    c.bench_function("broadcast_10_receivers", |b| {
        let (tx, _) = tokio::sync::broadcast::channel::<LoopEvent>(256);
        let _receivers: Vec<_> = (0..10).map(|_| tx.subscribe()).collect();
        b.iter(|| {
            let _ = tx.send(black_box(event.clone()));
        });
    });
}

// ── e. registry_emit ──────────────────────────────────────────────────────
//
// Real path: AgentJobRegistry::register + finish triggers internal emit
// (AgentJobEvent::Started + Completed via broadcast send). The registry's
// `emit` is pub(crate), so we exercise the public register/finish API
// which internally dispatches to the broadcast channel.

fn bench_registry_emit(c: &mut Criterion) {
    c.bench_function("registry_emit", |b| {
        let registry = AgentJobRegistry::new();
        // Subscribe so the broadcast channel has live receivers.
        let mut _rx = registry.subscribe();

        b.iter(|| {
            let id = registry.register(JobInit {
                agent: "general".into(),
                source: "bench".into(),
                run_id: None,
                node_id: None,
                session_id: None,
            });
            registry.finish(&id, JobStatus::Succeeded, None);
            black_box(());
        });
    });
}

criterion_group!(
    benches,
    bench_emit_three_segment,
    bench_emit_legacy_for_await,
    bench_emit_sync_only,
    bench_broadcast_multi_receiver,
    bench_registry_emit,
);
criterion_main!(benches);
