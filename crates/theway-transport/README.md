# theway-transport

`theway-transport` owns the cross-client wire model and the gRPC and web transports used to control a theway daemon. It provides generated protobuf services, HTTP JSON-RPC, server-sent events, WebSocket events, a typed gRPC client, daemon discovery helpers, and transport-facing operation traits.

The crate is independent of [`theway-core`](../theway-core/README.md), [`theway-daemon`](../theway-daemon/README.md), and [`theway-storage`](../theway-storage/README.md). A server implements `TransportHost` and supplies `TransportEndpoints`; clients use wire/protobuf types and `GrpcClient` without accessing runtime internals.

## Protocol entry points

- `wire` defines commands, complete and incremental status snapshots, graph/job events, configuration, and operation request/result records.
- `transport` defines `TransportEndpoints` plus `SessionOps`, `JobOps`, `GraphOps`, `ToolOps`, and `StorageOps`.
- `grpc`, `http`, and `ws` expose those operations over protobuf RPC, JSON-RPC, SSE, and WebSocket connections.
- `proto`, `tools`, and `state` convert between internal wire records and generated protobuf messages.
- `client` wraps tonic clients and discovers or starts a per-working-directory daemon.
- Shared modules such as `feed`, `commands`, `auth`, `history`, `images`, and `mentions` define client/daemon data that is not tied to a particular carrier.

MCP transport is not implemented here: external MCP clients live in [`theway-mcp`](../theway-mcp/README.md), and the daemon's MCP server lives in [`theway-daemon`](../theway-daemon/README.md).

## Documentation

- [Wire and transport architecture](docs/architecture.md)
- [Workspace architecture](../../docs/architecture.md)

## Validation

```bash
cargo test -p theway-transport
cargo doc -p theway-transport --no-deps --document-private-items
make layering-check
```
