# AGENTS.md — theway-core

This file adds crate-specific instructions to [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md) and [runtime architecture](docs/architecture.md) before changing agent, session, or multiagent behavior.

## Boundary rules

- Keep concrete tool bodies, host filesystem/process code, SQLite types, protocol messages, and telemetry exporters out of core.
- Introduce host-dependent behavior through an explicit trait or injected closure only when the runtime mechanism is reusable outside the daemon.
- Preserve `theway-daemon` as the only direct runtime workspace consumer; run `make layering-check` after dependency changes.
- Keep the bare `Agent` build usable without `harness` and without concrete provider features.

## Runtime rules

- Maintain single-run admission, cancellation cleanup, and terminal lifecycle events together when changing `Agent` or `run_loop`.
- Keep typed session interpretation in `agent/session`; persistence implementations receive only [`theway-contract`](../theway-contract/README.md) records through `PersistentSessionStorage`.
- Keep product events (`LoopEvent`, `SessionEvent`, `SubagentJobEvent`, `DagEvent`) separate from content-safe `RuntimeObserver` records.
- Preserve DAG transition validation in `multiagent/graph/model.rs` and scheduling in `multiagent/graph/scheduler.rs`; tool-facing commands belong to the daemon.
- Bound job output, transcripts, queues, and continuation loops at their owning runtime type.

## Tests and documentation

- Follow the mirrored unit-test rules in [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md); do not place multi-file test suites under `src/`.
- Add lifecycle tests for success, failure, timeout, cancellation, and drop paths when changing asynchronous runtime code.
- Update [docs/architecture.md](docs/architecture.md) when a public interface, ownership boundary, event plane, or graph/session lifecycle changes.
- Run `cargo test -p theway-core`, both `--no-default-features` checks from [README.md](README.md), `cargo doc -p theway-core --no-deps --document-private-items`, and `make layering-check`.
