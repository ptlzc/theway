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
| [`extension`](src/extension/mod.rs) | Define runtime-extension ABI v2 manifests, lifecycle/action envelopes, durable entries, trust records, diagnostics, and client-neutral contributions. |

`theway-core` converts typed runtime session entries to these raw records. `theway-storage` implements the persistence traits, while `theway-transport` reuses or re-exports the client-visible data that belongs at this leaf.

The checked-in TypeScript declarations and JSON Schemas shipped by the workspace plugin development SDK are generated from the Rust extension contracts. Regenerate them with `cargo run -p theway-contract --example generate_extension_artifacts -- sdks/plugin/abi-v2`; the extension contract tests regenerate into a temporary directory and reject drift.

ABI v2 contracts keep lifecycle envelopes, hook classes and actions, branch-local durable entries, diagnostics, trust records, commands, and client contributions engine-neutral. Sensitive values and executable runtime objects have no field in these records.

## Documentation

- [Architecture and invariants](docs/architecture.md)

## Validation

```bash
cargo test -p theway-contract
cargo doc -p theway-contract --no-deps
```
