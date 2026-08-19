# AGENTS.md — theway-storage

This file contains the complete crate-local modification rules for `theway-storage`. Read the [crate overview](README.md) and [persistence architecture](docs/architecture.md) before changing stored formats.

## Boundary rules

- Keep `theway-storage` dependent only on `theway-contract` among runtime workspace crates; do not import core runtime or transport types.
- Implement persistence against `SessionReader`, `SessionStore`, `StoredSessionEntry`, and persisted DAG records rather than duplicating their definitions.
- Keep session execution, graph transition policy, protocol conversion, and UI formatting in their owning crates.

## Durability rules

- Treat session databases as user data: report corruption and leave the file in place.
- Treat DAG databases as rebuildable snapshots: preserve the single rebuild-and-retry behavior when changing recovery paths.
- Keep archive member allowlists, size limits, checksums, entry validation, disabled-by-default automation, staging, WAL checkpoint, and rename commit behavior together.
- Preserve one `<uuidv7>.db` per session and derive sidecars from the final database path.

## Tests and documentation

- Add round-trip and corruption tests for schema or serialization changes, including cleanup assertions for failed archive imports.
- Place multi-file module suites under `tests/<mirrored-src-path>/` and bridge them from the owning source module.
- Update [docs/architecture.md](docs/architecture.md) when a stored artifact, recovery policy, archive rule, or dependency boundary changes.
- Run `cargo test -p theway-storage`, `cargo doc -p theway-storage --no-deps --document-private-items`, and `make layering-check`.
