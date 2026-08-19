# AGENTS.md — theway-transport

This file adds crate-specific instructions to [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md) and [transport architecture](docs/architecture.md) before changing wire records, protobuf services, or carrier behavior.

## Boundary rules

- Keep this crate independent of `theway-core`, `theway-daemon`, `theway-storage`, and all UI crates.
- Define cross-client request, result, snapshot, and event records here before implementing daemon or client behavior.
- Keep model/session/tool/graph policy in the server implementation; gRPC, HTTP, SSE, and WebSocket handlers translate and route only.
- Keep MCP client and server behavior in [`theway-mcp`](../theway-mcp/README.md) and [`theway-daemon`](../theway-daemon/README.md), respectively.

## Protocol rules

- Route ordered runtime mutations through `WireCommand`; use operation traits for independent reads and controls.
- Preserve complete-snapshot recovery whenever changing incremental feed/status frames.
- Change proto files, Rust conversion modules, service implementations, client wrappers, and the TypeScript SDK together; run `make sdk-sync` after proto changes.
- Keep gRPC and web carriers semantically aligned for shared operations and errors.
- Reuse leaf records from [`theway-contract`](../theway-contract/README.md) instead of introducing transport-owned duplicates.

## Tests and documentation

- Add conversion round trips for every new protobuf field and carrier tests for routing, validation, streaming, lag, and disconnect behavior.
- Follow [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) for mirrored suites.
- Update [docs/architecture.md](docs/architecture.md) when endpoint ownership, snapshot semantics, a carrier, discovery, or protocol generation changes.
- Run `cargo test -p theway-transport`, `cargo doc -p theway-transport --no-deps --document-private-items`, and `make layering-check`; run `make sdk-sync` and its generated diff checks for proto changes.
