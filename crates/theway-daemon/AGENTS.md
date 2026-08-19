# AGENTS.md — theway-daemon

This file adds crate-specific instructions to [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md), [daemon architecture](docs/architecture.md), and [workspace daemon positioning](../../AGENTS.md#daemon-positioning) before changing application assembly or protocol behavior.

## Ownership rules

- Keep reusable agent-loop, session, observability, and graph-engine mechanics in [`theway-core`](../theway-core/README.md); keep concrete host policy and adapters here.
- Keep wire records and transport servers in [`theway-transport`](../theway-transport/README.md), persistence implementations in [`theway-storage`](../theway-storage/README.md), and all client appearance and interaction in [`theway-tui`](../theway-tui/Cargo.toml).
- Add model-facing tool bodies under `src/tools/`; do not move concrete tool behavior into core.
- Keep the public exports in `src/lib.rs` intentional; new internal modules remain private unless an embedder needs a supported extension point.

## Composition rules

- Route initial, resumed, and switched sessions through `SessionRuntimeBuilder`; do not create a second harness assembly path.
- Own process-lifetime registries in `DaemonServices` and inject them into session/runtime builders rather than introducing process globals.
- Program orchestration against `RuntimeStorage` and `SessionRepository`; concrete local and remote adapters contain SQLite and RPC details.
- Resolve host paths through `DaemonPaths` at startup and pass paths explicitly to consumers.
- Define cross-client operations in transport types first, then implement daemon semantics through adapters and endpoint traits.
- Preserve fail-fast behavior for unsupported sandbox execution and explicit configuration errors.

## Tests and documentation

- Follow [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) for mirrored unit suites and use process/network tests only for assembled behavior.
- Cover startup, resume, session switch, cancellation, service replacement, local/remote storage, and transport adaptation when their paths change.
- Update [docs/architecture.md](docs/architecture.md) when composition ownership, public extension points, storage ports, session assembly, or protocol adaptation changes.
- Run `cargo test -p theway-daemon`, `cargo doc -p theway-daemon --no-deps --document-private-items`, and `make layering-check`; run the relevant transport or storage crate tests when their adapter behavior changes.
