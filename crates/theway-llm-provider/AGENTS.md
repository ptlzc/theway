# AGENTS.md — theway-llm-provider

This file adds crate-specific instructions to [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md) and [provider architecture](docs/architecture.md) before changing normalized types or a wire implementation.

## Boundary rules

- Keep agent loops, retry policy across turns, tool execution, sessions, permissions, and UI behavior out of this crate.
- Keep provider-specific request fields, stream event names, authentication, and errors inside `src/providers/`.
- Extend shared normalized types only when at least one caller or multiple provider protocols need the concept.
- Do not make core runtime crates depend on provider implementation modules; callers use the crate-root API and normalized types.

## Streaming rules

- Emit ordered start/delta/end events and exactly one terminal result for success, provider error, cancellation, malformed input, and transport failure.
- Preserve tool-call correlation across interleaved deltas and normalize ids before history is reused with another provider.
- Keep thinking, cache, images, usage, and stop-reason conversion explicit per protocol.
- Bound response parsing and tolerate protocol-permitted empty or partial frames without hanging the result future.
- Redact API keys, authorization headers, and provider response secrets from diagnostics.

## Features, catalogs, and tests

- Add a provider feature, module declaration, dependency set, and built-in registration in the same change.
- Keep generated model data and its Rust projection synchronized through [`scripts/regen_models.sh`](scripts/regen_models.sh); set `TS_PATH` explicitly when using that importer.
- Tests must use scripted providers or local HTTP/SSE fixtures and must not call real provider APIs.
- Update [docs/architecture.md](docs/architecture.md) when normalized types, dispatch, transformation, catalogs, credentials, or provider extension steps change.
- Run the no-default-features and all-features commands in [README.md](README.md), plus `cargo doc -p theway-llm-provider --no-deps --document-private-items`.
