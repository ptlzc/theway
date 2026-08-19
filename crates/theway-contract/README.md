# theway-contract

`theway-contract` is the workspace leaf for data that must cross runtime, persistence, and protocol implementations without importing any of them. It defines serializable records, storage traits, automation sidecar models, session identifiers, and the `~/.theway` path layout; it contains no agent engine, database backend, or network transport.

## Public modules

| Module | Responsibility |
|---|---|
| [`config`](src/config.rs) | Resolve the base directory and derive stable per-working-directory paths. |
| [`session`](src/session.rs) | Define raw stored session records plus the asynchronous `SessionReader` and `SessionStore` traits. |
| [`session_id`](src/session_id.rs) | Validate and normalize persisted session identifiers. |
| [`dag`](src/dag.rs) | Define persisted DAG run and node snapshots and their state-file path. |
| [`triggers`](src/triggers.rs) | Define session-scoped dynamic-trigger and cron sidecar records. |

[`theway-core`](../theway-core) converts typed runtime session entries to these raw records. [`theway-storage`](../theway-storage) implements the persistence traits, while [`theway-transport`](../theway-transport) reuses or re-exports the client-visible data that belongs at this leaf.

## Documentation

- [Architecture and invariants](docs/architecture.md)
- [Workspace architecture](../../docs/architecture.md)

## Validation

```bash
cargo test -p theway-contract
cargo doc -p theway-contract --no-deps
```
