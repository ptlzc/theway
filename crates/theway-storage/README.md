# theway-storage

English | [中文](README.zh.md)

`theway-storage` provides the local durable implementations of the raw persistence interfaces in `theway-contract`. It stores one Turso/SQLite database per session, commits ordered entry batches atomically, replays extension entries from selected branches, manages session discovery and sidecar paths, imports and exports `.theway-session` archives, and stores persisted DAG snapshots.

The crate does not interpret typed agent messages or DAG transition rules. It depends only on `theway-contract` among the runtime workspace crates and never imports `theway-core` or `theway-transport`.

## Public modules

| Module | Responsibility |
|---|---|
| [`attachments`](src/attachments.rs) | Store content-addressed attachment bytes in a local object tree. |
| [`sqlite_storage`](src/sqlite_storage.rs) | Implement `SessionReader` and `SessionStore` for one session database. |
| [`sqlite_repo`](src/sqlite_repo.rs) | Create, open, list, and delete session database files under one repository root. |
| [`session`](src/session.rs) | Provide create/resume/fork/list helpers, session previews, and trigger/cron sidecar paths. |
| [`session_archive`](src/session_archive.rs) | Export and import validated `.theway-session` tar archives. |
| [`sqlite_dag`](src/sqlite_dag.rs) | Replace and restore persisted DAG run snapshots. |

## Attachment library

[`attachments`](src/attachments.rs) implements the contract's `AttachmentStore` as `LocalAttachmentStore`, a content-addressed tree of object files rooted at one directory. `new(root)` takes an explicit root, `from_base_dir()` roots it at `<base>/attachments/v1` — the `theway_contract::config::attachments_dir` layout — and `root()` reports the resolved directory.

An object lives at `<root>/<ab>/<sha256>`, where `<ab>` is the digest's first two hex characters and the object file is named with all 64 hex characters. `put` deduplicates on the content digest, stages new bytes in a temporary file inside the target directory, and renames it into place, so a concurrent reader sees either the whole object or nothing. `get` recomputes the digest of the bytes it read and reports `DigestMismatch` instead of returning bytes that do not hash to the digest they are filed under; `contains` answers from the file's existence without reading it.

## Documentation

- [Persistence architecture and failure behavior](docs/architecture.md)

## Validation

```bash
cargo test -p theway-storage
cargo doc -p theway-storage --no-deps --document-private-items
make layering-check
```
