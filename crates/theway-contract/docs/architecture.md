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
- `SessionReader` exposes metadata and tree queries, including extension entries filtered to one selected branch in root-to-leaf replay order.
- `SessionStore` extends the reader operations with entry creation, leaf movement, and atomic ordered entry batches; the single-entry adapter uses the same batch contract.
- `SessionStore::set_binding` persists or clears the non-secret client binding. The default fails closed with `StorageFailure` and the stable message `session store does not support binding updates`; storage backends override it when they support binding updates.

`theway-core::PersistentSessionStorage` is the adapter that encodes and decodes typed `SessionTreeEntry` values. This crate does not interpret prompts, model changes, compaction records, or custom runtime events.

## Runtime extension ABI records

[`extension`](../src/extension/mod.rs) defines the single unversioned ABI shared across runtime layers. Package manifests, permissions, trust decisions, lifecycle events, hook/action contracts, extension-owned durable entries, catalog state, redacted diagnostics, command outcomes, and declarative client contributions are serializable values with no engine handles, protocol objects, or ABI selectors.

The `ExtensionDurableEntry` envelope is stored inside an opaque session entry. It identifies the owning extension, state schema, originating lifecycle sequence, and one private state mutation, immutable custom event, model-context item, or migration record. Storage implementations preserve the envelope without interpreting its payload; runtime projection and policy remain outside this crate.

JSON Schema derives and the generator in [`generate_extension_artifacts.rs`](../examples/generate_extension_artifacts.rs) produce the checked-in artifacts shipped by the workspace plugin development SDK. The generated TypeScript file contains the schema bundle digest, and tests compare every generated artifact with a temporary regeneration.

## DAG and automation records

[`dag.rs`](../src/dag.rs) contains the serializable run, node, result, status, and direction records needed to persist graph-engine snapshots. The graph scheduler and transition rules live in `theway-core`.

[`subagent_settings.rs`](../src/subagent_settings.rs) contains the project-level last-set subagent model/thinking overrides (keyed by DAG node id and subagent spec name) and the `subagent_settings_path_for_project` path rule placing them at `<project>/.pi/subagent-settings.json`, independent of session-scoped state files. Merge policy and file I/O live in `theway-daemon`.

[`triggers.rs`](../src/triggers.rs) contains the sidecar representation for dynamic trigger rules and cron jobs. Polling, scheduling, promotion, and delivery live in `theway-daemon`.

## User input and attachment records

[`user_input.rs`](../src/user_input.rs) owns the canonical shape of one round of input and the `sha256:<64 lowercase hex>` identifier that every layer uses to name attachment bytes. The record carries digests only, and [`attachments.rs`](../src/attachments.rs) declares the byte-store contract those digests resolve through, so neither the storage layout nor the admission policy enters this crate.

The record reaches a session as an `AgentMessage::Custom` entry whose role is `UserInput::CUSTOM_ROLE`, immediately before the user message it describes. Display projection and the model request are derived downstream, and a session without that entry is read through its message alone; the record is append-only and never rewrites stored history.

[`config.rs`](../src/config.rs) derives `attachments_dir()` as `<base>/attachments/v1` under the same base-dir rule as the other layouts. The trait implementation and the object layout belong to `theway-storage`; admission, which resolves mentions and writes bytes before the record exists, belongs to `theway-daemon`.

## Invariants

- Attachment bytes are referenced by `sha256:` digest only; the record never embeds file or image content.
- Public records remain independent of concrete storage and transport libraries.
- Serde field names, defaults, and enum encodings are persisted data rules; changes require round-trip and compatibility tests.
- Path derivation and session-id validation remain shared functions rather than copied implementations in consuming crates.
- The crate does not acquire behavior that needs an LLM provider, daemon service, filesystem backend, or client UI.
- Runtime extension records contain only unversioned, JSON-serializable ABI data; script-engine values and client-specific rendering objects never enter this crate.
