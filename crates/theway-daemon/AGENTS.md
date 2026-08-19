# AGENTS.md — theway-daemon

This file contains the complete crate-local modification rules for `theway-daemon`. Read the [crate overview](README.md) and [daemon architecture](docs/architecture.md) before changing application assembly or protocol behavior.

## Ownership rules

- Keep reusable agent-loop, session, observability, and graph-engine mechanics in `theway-core`; keep concrete host policy and adapters here.
- Keep wire records and transport servers in `theway-transport`, persistence implementations in `theway-storage`, and all client appearance and interaction in `theway-tui`.
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

- Keep multi-file unit suites under `tests/<mirrored-src-path>/` and bridge them from the owning source module; use process or network tests only for assembled behavior.
- Cover startup, resume, session switch, cancellation, service replacement, local/remote storage, and transport adaptation when their paths change.
- Update [docs/architecture.md](docs/architecture.md) when composition ownership, public extension points, storage ports, session assembly, or protocol adaptation changes.
- Run `cargo test -p theway-daemon`, `cargo doc -p theway-daemon --no-deps --document-private-items`, and `make layering-check`; run the relevant transport or storage crate tests when their adapter behavior changes.
