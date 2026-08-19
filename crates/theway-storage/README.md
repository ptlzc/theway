# theway-storage

English | [中文](README.zh.md)

`theway-storage` provides the local durable implementations of the raw persistence interfaces in [`theway-contract`](../theway-contract/README.md). It stores one Turso/SQLite database per session, manages session discovery and sidecar paths, imports and exports `.theway-session` archives, and stores persisted DAG snapshots.

The crate does not interpret typed agent messages or DAG transition rules. It depends only on `theway-contract` among the runtime workspace crates and never imports [`theway-core`](../theway-core/README.md) or [`theway-transport`](../theway-transport/README.md).

## Public modules

| Module | Responsibility |
|---|---|
| [`sqlite_storage`](src/sqlite_storage.rs) | Implement `SessionReader` and `SessionStore` for one session database. |
| [`sqlite_repo`](src/sqlite_repo.rs) | Create, open, list, and delete session database files under one repository root. |
| [`session`](src/session.rs) | Provide create/resume/fork/list helpers, session previews, and trigger/cron sidecar paths. |
| [`session_archive`](src/session_archive.rs) | Export and import validated `.theway-session` tar archives. |
| [`sqlite_dag`](src/sqlite_dag.rs) | Replace and restore persisted DAG run snapshots. |

## Documentation

- [Persistence architecture and failure behavior](docs/architecture.md)
- [Leaf record definitions](../theway-contract/docs/architecture.md)
- [Workspace architecture](../../docs/architecture.md)

## Validation

```bash
cargo test -p theway-storage
cargo doc -p theway-storage --no-deps --document-private-items
make layering-check
```
