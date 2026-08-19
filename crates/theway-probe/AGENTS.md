# AGENTS.md — theway-probe

This file adds crate-specific instructions to [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md) and [probe architecture](docs/architecture.md) before changing protocol coverage.

## Boundary rules

- Keep the probe a standalone external gRPC client; do not depend on `theway-daemon`, `theway-core`, or the Rust `theway-transport` crate.
- Compile protobuf clients from the definitions owned beside [`../theway-transport/proto/health.proto`](../theway-transport/proto/health.proto) and do not copy protocol files here.
- Keep checks deterministic, keyless, and safe against an operator-supplied daemon endpoint.
- Do not add daemon process startup, shutdown, filesystem mutation, or LLM calls to a serviceability check without an explicit scope change.

## Output rules

- Return one `TestResult` per selected check and fail unknown names explicitly.
- Keep stdout useful to a human and keep optional JSON result files stable for automation.
- Preserve non-zero exit status whenever any selected check fails.

## Tests and documentation

- Update [docs/architecture.md](docs/architecture.md) and [README.md](README.md) when adding or removing a check or changing CLI/output behavior.
- Rebuild the probe after transport proto changes that affect its imported services.
- Run `cargo check -p theway-probe` and `cargo doc -p theway-probe --no-deps --document-private-items`; exercise the affected check against a local test daemon for behavior changes.
