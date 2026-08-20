# theway-contract architecture

English | [中文](architecture.zh.md)

## Dependency position

`theway-contract` has no workspace dependencies. Runtime crates depend inward on it so persisted records and shared path rules do not acquire an agent-engine, SQLite, or transport dependency.

The crate owns representation and compatibility rules only. Selection policy, execution, serialization to a concrete medium, and protocol handling stay in their implementing crates.

## Path and identity rules

[`config.rs`](../src/config.rs) resolves `${THEWAY_DIR}` when present and otherwise uses `$HOME/.theway`. `sessions_dir_for_cwd` combines that base with the deterministic hash produced by `cwd_hash`; changing this algorithm changes the location of existing session data and therefore requires an explicit compatibility decision.

[`session_id.rs`](../src/session_id.rs) centralizes session identifier validation so file-backed and protocol-backed implementations accept the same identifier set.

## Session persistence records

[`session.rs`](../src/session.rs) separates storage representation from runtime interpretation:

- `StoredSessionEntry` carries the raw JSON payload, indexed identity, parent, timestamp, and entry type used by persistence implementations.
- `validate_session_entries` verifies entry structure and derives the active leaf from the append-only record sequence.
- `SessionReader` exposes metadata and tree queries.
- `SessionStore` extends the reader operations with entry creation, append, and leaf movement.

`theway-core::PersistentSessionStorage` is the adapter that encodes and decodes typed `SessionTreeEntry` values. This crate does not interpret prompts, model changes, compaction records, or custom runtime events.

## Runtime extension ABI records

[`extension`](../src/extension/mod.rs) defines ABI major 2 data shared across runtime layers. Package manifests, permissions, trust decisions, lifecycle events, hook/action contracts, extension-owned durable entries, catalog state, redacted diagnostics, command outcomes, and declarative client contributions are serializable values with no engine handles or protocol objects.

The `ExtensionDurableEntry` envelope is stored inside an opaque session entry. It identifies the owning extension, state schema, originating lifecycle sequence, and one private state mutation, immutable custom event, model-context item, or migration record. Storage implementations preserve the envelope without interpreting its payload; runtime projection and policy remain outside this crate.

JSON Schema derives and the generator in [`generate_extension_artifacts.rs`](../examples/generate_extension_artifacts.rs) produce the checked-in artifacts under [`sdk/extension-abi-v2`](../sdk/extension-abi-v2). The generated TypeScript file contains the schema bundle digest, and tests compare every generated artifact with a temporary regeneration.

## DAG and automation records

[`dag.rs`](../src/dag.rs) contains the serializable run, node, result, status, and direction records needed to persist graph-engine snapshots. The graph scheduler and transition rules live in `theway-core`.

[`triggers.rs`](../src/triggers.rs) contains the sidecar representation for dynamic trigger rules and cron jobs. Polling, scheduling, promotion, and delivery live in `theway-daemon`.

## Invariants

- Public records remain independent of concrete storage and transport libraries.
- Serde field names, defaults, and enum encodings are persisted data rules; changes require round-trip and compatibility tests.
- Path derivation and session-id validation remain shared functions rather than copied implementations in consuming crates.
- The crate does not acquire behavior that needs an LLM provider, daemon service, filesystem backend, or client UI.
- Runtime extension records contain only versioned, JSON-serializable data; script-engine values and client-specific rendering objects never enter this crate.
