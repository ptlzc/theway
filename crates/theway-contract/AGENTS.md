# AGENTS.md — theway-contract

This file adds crate-specific instructions to the repository rules in [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md) and [architecture reference](docs/architecture.md) before changing persisted records or path rules.

## Ownership rules

- Add a type here only when multiple runtime layers need the same engine-independent representation or persistence interface.
- Keep runtime policy, database code, protocol conversion, and UI behavior out of this crate.
- Do not add workspace dependencies; `theway-contract` remains the dependency leaf for runtime data.

## Compatibility rules

- Treat serde names, enum encodings, optional-field defaults, and `StoredSessionEntry` validation as persisted-format behavior.
- Keep `config::cwd_hash`, session-directory layout, and session-id validation compatible with existing on-disk names.
- Put conversion between raw records and core runtime types in [`theway-core`](../theway-core/README.md), and conversion to protocol messages in [`theway-transport`](../theway-transport/README.md).

## Tests and documentation

- Add round-trip tests for record changes and invalid-input tests for validation changes.
- Update [docs/architecture.md](docs/architecture.md) when module ownership, compatibility behavior, or a public trait changes.
- Run `cargo test -p theway-contract` and `cargo doc -p theway-contract --no-deps`.
