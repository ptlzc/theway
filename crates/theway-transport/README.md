# theway-transport

English | [中文](README.zh.md)

`theway-transport` owns the cross-client wire model and the gRPC and web transports used to control a theway daemon. It provides generated protobuf services, HTTP JSON-RPC, server-sent events, WebSocket events, a typed gRPC client, daemon discovery helpers, and transport-facing operation traits.

The crate is independent of `theway-core`, `theway-daemon`, and `theway-storage`. A server implements `TransportHost` and supplies `TransportEndpoints`; clients use wire/protobuf types and `GrpcClient` without accessing runtime internals.

## Protocol entry points

- `wire` defines commands, complete and incremental status snapshots, graph/job events, configuration, operation request/result records, and open runtime-extension catalog, diagnostic, command, contribution, trust, and reload records.
- `transport` defines `TransportEndpoints` plus `SessionOps`, `JobOps`, `GraphOps`, `ToolOps`, and `StorageOps`.
- `grpc`, `http`, and `ws` expose those operations over protobuf RPC, JSON-RPC, SSE, and WebSocket connections.
- `proto`, `tools`, and `state` convert between internal wire records and generated protobuf messages.
- `client` wraps tonic clients and discovers or starts a per-working-directory daemon.
- Shared modules such as `feed`, `commands`, `auth`, `history`, `images`, and `mentions` define client/daemon data that is not tied to a particular carrier.

MCP transport is not implemented here: external MCP clients live in `theway-mcp`, and the daemon's MCP server lives in `theway-daemon`.

## Documentation

- [Wire and transport architecture](docs/architecture.md)

## Validation

```bash
cargo test -p theway-transport
cargo doc -p theway-transport --no-deps --document-private-items
make layering-check
```
