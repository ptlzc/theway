# AGENTS.md — theway-tui

This file contains the complete crate-local modification rules for `theway-tui`. Read the [crate overview](README.md) and [client/controller architecture](docs/architecture.md) before changing interaction or startup behavior.

## Boundary rules

- Never add dependencies on `theway-core` or `theway-daemon`; use `theway-transport` messages, clients, and operation traits.
- Keep terminal colors, layout, keys, mouse behavior, loaders, pickers, clipboard handling, and feed presentation in this crate.
- Define shared cross-client state in transport first; do not infer daemon internals or add TUI-only fields to runtime snapshots.
- Keep direct SQLite access inside controller storage and offline session commands; interactive runtime mutations use protocol operations.

## Controller and state rules

- Start controller `ToolService` and `StorageService` before attaching or spawning the daemon, bind them to loopback, and provision their addresses explicitly.
- Keep `LocalToolOps` rooted at the selected working directory and preserve request validation, output bounds, timeout, and path behavior.
- Treat complete snapshots as authoritative; validate delta bases and reset session-scoped caches when session identity changes.
- Keep streaming feed caches derived and disposable, with full-render fallback for non-append edits.
- Implement rendering primitives in the dedicated markdown, pager, or textarea crate when they are reusable outside the application shell.

## Tests and documentation

- Use local tonic fixtures and fake operation traits; tests must not start provider calls or depend on a user's session directory.
- Cover keyboard/mouse actions, snapshot lag and session changes, daemon reuse/spawn, controller services, offline commands, and width-sensitive rendering in their owning modules.
- Place multi-file suites under `tests/<mirrored-src-path>/` and bridge them from the owning source module.
- Update [docs/architecture.md](docs/architecture.md) when startup sequencing, controller ownership, state application, rendering, or offline/interactive boundaries change.
- Run `cargo test -p theway-tui`, `cargo doc -p theway-tui --no-deps --document-private-items`, and `make layering-check`.
