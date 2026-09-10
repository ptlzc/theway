# theway-storage architecture

English | [中文](architecture.zh.md)

## Dependency position

`theway-storage` implements records and asynchronous traits from `theway-contract`. It does not depend on the typed runtime in `theway-core` or on protocol types from `theway-transport`.

The daemon and TUI choose when local persistence is appropriate. This crate owns local files and their recovery behavior, not session execution or client coordination.

## Session repository and database

[`sqlite_repo.rs`](../src/sqlite_repo.rs) owns a repository directory. `SqliteSessionRepo` creates a `<uuidv7>.db`, opens a selected path, lists database files, and deletes an exact session file.

[`sqlite_storage.rs`](../src/sqlite_storage.rs) owns one session database. `SqliteSessionStorage` stores metadata in the `meta` table and append-only `StoredSessionEntry` JSON payloads in sequence order. `append_entries` serializes a complete ordered batch before opening one SQLite transaction and commits every row together; serialization, constraint, or commit failure exposes none of the batch. The latest ordinary entry becomes the active leaf; a `leaf` entry moves that pointer to its recorded target.

`get_extension_entries` resolves an explicit leaf or the persisted active leaf, follows parent indexes to the root, and returns only the requested extension's opaque entries in replay order. Branch switches, process restarts, forks, and archive imports therefore reconstruct extension data from the session log without a daemon-local cache.

Opening an existing session runs SQLite integrity checks and decodes its metadata. A damaged session returns `SessionErrorCode::Corrupted` and remains untouched because the transcript is user data. `checkpoint` flushes WAL pages before archive-import staging renames a database.

[`session.rs`](../src/session.rs) builds higher-level repository operations over the raw store: create, resume, fork, list, preview, rename, lookup, delete, and automation-sidecar discovery. These helpers still operate on raw stored entries and do not decode `theway-core::SessionTreeEntry`.

## Session archives

[`session_archive.rs`](../src/session_archive.rs) exports a canonical `session.jsonl`, a manifest, and optional trigger and cron sidecars into a `.theway-session` tar archive. The manifest records the transcript hash, entry count, active leaf, source identity, and included sidecars; provider credentials and separate authentication stores are not archive members.

Import accepts only the fixed archive member names, enforces member-size limits, verifies the schema, transcript SHA-256, entry count, active leaf, entry graph, UTF-8, and sidecar syntax, then assigns a new UUIDv7 session id. It populates a non-`.db` staging database, checkpoints it, writes sidecars, and renames the database as the commit point. A failed import removes the staging database and sidecars.

Automation is disabled on import unless `ActivateTriggers::On` is selected. Interactive `Ask` handling belongs to the calling client and is not performed by `import_session`.

## Attachment library

[`attachments.rs`](../src/attachments.rs) implements the leaf contract's `AttachmentStore` as a local object tree that is independent of the session databases. `LocalAttachmentStore::new` takes an explicit root, `from_base_dir` roots the store at `theway_contract::config::attachments_dir` (`<base>/attachments/v1`), and `root` reports the resolved directory.

An object lives at `<root>/<shard>/<hex>`, where `hex` is the 64 lowercase hex characters of the `sha256:<hex>` digest and `shard` is its first two characters. `put` hashes the bytes, returns the digest unchanged when the object file already exists, and otherwise creates the shard directory, writes a `tmp-<pid>-<nanos>` staging file inside that same directory, and renames it into place; a failed rename removes the staging file, so neither a partial object nor a staging file is left behind. Because the object name is the content digest, an existing object is never rewritten.

`get` rejects a malformed digest with `InvalidDigest`, reports a missing object as `NotFound`, recomputes the digest of the bytes it read, and returns `DigestMismatch` rather than bytes that do not hash to the digest they are filed under. `contains` answers from object existence only and never reads bytes.

## DAG snapshots

[`sqlite_dag.rs`](../src/sqlite_dag.rs) stores `PersistedRun` and `PersistedNode` records. `save` transactionally replaces the complete snapshot set; `load` skips individual rows whose JSON cannot be decoded.

DAG snapshots are rebuildable runtime state. A database that cannot be opened or written is discarded and rebuilt once, unlike a corrupt session transcript, which is preserved and reported.

## Invariants

- Session databases are high-value append-only records and are never auto-rebuilt after corruption.
- Ordered session entry batches are all-or-nothing SQLite transactions.
- Extension payloads remain opaque to storage and branch selection is derived from persisted parent and leaf records.
- DAG snapshot databases are replaceable projections and may be rebuilt after corruption.
- Archive import validates all content before exposing the final `<uuidv7>.db` path.
- Attachment objects are immutable and content-addressed: an existing object is never rewritten, and a read verifies the digest before returning bytes.
- Attachment objects commit by rename, so a reader never observes a partially written object.
- Raw persistence remains independent of typed runtime and wire representations.
- Sidecar paths derive from the final session database path through shared helpers.
