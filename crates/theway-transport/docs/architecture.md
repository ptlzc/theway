# theway-transport architecture

## Dependency position

`theway-transport` depends on [`theway-contract`](../../theway-contract/docs/architecture.md) for shared persisted records and on [`theway-llm-provider`](../../theway-llm-provider/README.md) for model-facing data reused in configuration and snapshots. It does not depend on the runtime engine, daemon application, or storage implementation.

This direction lets any server or client use the protocol without linking `AgentHarness`, SQLite, terminal rendering, or daemon-owned tools.

## Wire model and endpoint API

[`wire.rs`](../src/wire.rs) is the serde representation shared by the server event loop and JSON transports. `WireCommand` carries mutations into the serialized runtime loop. `WireStatus` is the authoritative client snapshot, while `WireStatusUpdate` can carry a complete snapshot or a feed delta that applies only when its base indexes match the receiving snapshot.

[`transport.rs`](../src/transport.rs) defines the server-facing API:

- `TransportEndpoints` contains the command channel, status broadcaster and latest snapshot, agent and DAG event broadcasters, session identity and path/config views, plus operation trait objects.
- `SessionOps`, `JobOps`, `GraphOps`, `ToolOps`, and `StorageOps` expose request/response operations that do not need direct runtime state access.
- `Unavailable*` implementations provide explicit errors or empty behavior when a host does not support an optional operation group.

[`host.rs`](../src/host.rs) defines `TransportHost`. The server gives a transport its endpoints, then runs the serialized application loop alongside the server task. The concrete implementation is daemon-owned.

## gRPC carrier

The protobuf files, including [`commands.proto`](../proto/commands.proto), [`events.proto`](../proto/events.proto), and [`state.proto`](../proto/state.proto), are the source of truth for services and messages. [`build.rs`](../build.rs) compiles every proto file with `protox` and `tonic-prost-build`, so no system `protoc` is required.

[`grpc/mod.rs`](../src/grpc/mod.rs) maps command, session, settings, graph, event, tool, storage, and health services to `TransportEndpoints`. Mutating operations that must serialize with turns enqueue a `WireCommand`; read/control traits execute through their endpoint object. Event subscriptions receive current state and then incremental frames, with lag recovery from the authoritative latest snapshot.

[`proto.rs`](../src/proto.rs), [`tools.rs`](../src/tools.rs), and [`state.rs`](../src/state.rs) own protobuf conversion for session state, tool operations, and runtime storage records. A proto change must update these conversions and the generated TypeScript SDK through `make sdk-sync`.

## Web carriers

[`http.rs`](../src/http.rs) serves health, JSON-RPC, SSE events, and WebSocket upgrade routes from the same endpoint set used by gRPC. [`ws.rs`](../src/ws.rs) accepts JSON commands and publishes status, agent, and DAG events as JSON frames.

HTTP and WebSocket handlers translate carrier input into transport-owned requests or `WireCommand` values. They do not implement model, session, storage, tool, or graph policy.

## Client and daemon discovery

[`client.rs`](../src/client.rs) wraps generated tonic clients in `GrpcClient` and exposes typed commands, state streams, session/graph control, controller tool service, and storage service calls.

Daemon discovery reads a per-working-directory port/pid file under `${THEWAY_DIR:-$HOME/.theway}`, probes candidate loopback addresses, removes stale entries only when ownership matches, and can spawn `thewayd` before waiting for readiness. Discovery is loopback-oriented and does not add an authentication protocol.

## Shared client records

Modules including [`feed/mod.rs`](../src/feed/mod.rs), [`commands.rs`](../src/commands.rs), [`auth.rs`](../src/auth.rs), [`history.rs`](../src/history.rs), [`images.rs`](../src/images.rs), and [`mentions.rs`](../src/mentions.rs) define reusable client/daemon records and pure helpers. Leaf path, trigger, cron, and raw persistence definitions remain in `theway-contract` and are re-exported only where a stable transport path requires it.

## Invariants

- Wire and protobuf records never contain core or daemon-private types.
- All carriers drive the same `TransportEndpoints` semantics; carrier-specific handlers do not acquire business policy.
- Runtime mutations that need ordering enter the serialized command queue.
- Snapshot deltas apply only to the matching base and recover through a complete authoritative snapshot after lag or mismatch.
- Proto source, Rust conversions, service handlers, client calls, and generated SDK output change together.
- The crate contains no client appearance, terminal input handling, storage backend, or MCP implementation.
