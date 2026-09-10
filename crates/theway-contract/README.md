# theway-contract

English | [中文](README.zh.md)

`theway-contract` is the workspace leaf for data that must cross runtime, persistence, and protocol implementations without importing any of them. It defines serializable records, storage traits, automation sidecar models, session identifiers, and the `~/.theway` path layout; it contains no agent engine, database backend, or network transport.

## Public modules

| Module | Responsibility |
|---|---|
| [`config`](src/config.rs) | Resolve the base directory and derive stable per-working-directory paths. |
| [`session`](src/session.rs) | Define raw stored session records plus the asynchronous `SessionReader` and `SessionStore` traits. |
| [`session_id`](src/session_id.rs) | Validate and normalize persisted session identifiers. |
| [`dag`](src/dag.rs) | Define persisted DAG run and node snapshots and their state-file path. |
| [`triggers`](src/triggers.rs) | Define session-scoped dynamic-trigger and cron sidecar records. |
| [`extension`](src/extension/mod.rs) | Define the single unversioned runtime-extension ABI: manifests, lifecycle/action envelopes, durable entries, trust records, diagnostics, and client-neutral contributions. |
| [`user_input`](src/user_input.rs) | Define the canonical record of one round of user input: submitted text, ordered parts, origin, and the attachment digest form. |
| [`attachments`](src/attachments.rs) | Define the content-addressed attachment byte-store contract and its failure cases. |

`theway-core` converts typed runtime session entries to these raw records. `theway-storage` implements the persistence traits, while `theway-transport` reuses or re-exports the client-visible data that belongs at this leaf.

The checked-in TypeScript declarations and JSON Schemas shipped by the workspace plugin development SDK are generated from the Rust extension contracts. Regenerate them with `cargo run -p theway-contract --example generate_extension_artifacts -- sdks/plugin/abi`; the extension contract tests regenerate into a temporary directory and reject drift.

The unversioned ABI keeps lifecycle envelopes, hook classes and actions, branch-local durable entries, diagnostics, trust records, commands, and client contributions engine-neutral. Sensitive values, executable runtime objects, and ABI version selectors have no field in these records.

## Structured user input

[`user_input`](src/user_input.rs) is the canonical record of one round of input: `UserInput { text, parts, source, source_ref }`. `text` is the text the user submitted, with `@path` tokens kept in the shape they were typed; `parts` is the ordered `Vec<InputPart>` of attachments and injected content; `source` is the `InputSource` (`user` / `trigger` / `subagent` / `host`); and `source_ref` names the non-`user` origin. `UserInput::CUSTOM_ROLE` (`user_input`) is the one role string the session log and the display projection share.

`InputPart` is `File { path, name, digest, bytes, media_type, truncated }`, `Image { name, digest, bytes, media_type }`, or `Injected { source, name, text }`; `has_attachments` reports whether any part is a `File` or an `Image`. Bytes never travel in the record: `digest_bytes` produces the workspace-wide attachment identifier `sha256:<64 lowercase hex>` (`DIGEST_PREFIX`, validated by `digest_is_valid`) and the record carries that digest.

[`attachments`](src/attachments.rs) defines the `AttachmentStore` trait (`put` / `get` / `contains`) and `AttachmentError` (`InvalidDigest`, `NotFound`, `DigestMismatch`, `Io`). `put` returns the digest of the bytes it stored, and `get` re-verifies the bytes against that digest before returning them. [`config::attachments_dir`](src/config.rs) derives the library root from the shared base dir as `<base>/attachments/v1`.

The record is an append-only addition to the session log: it is appended immediately before the user message it describes, and an older session without one is read through its message alone. The `camelCase` field names and snake_case enum tags of these records are persisted-format behaviour.

## Documentation

- [Architecture and invariants](docs/architecture.md)

## Validation

```bash
cargo test -p theway-contract
cargo doc -p theway-contract --no-deps
```
