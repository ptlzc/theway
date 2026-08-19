# theway-daemon

`theway-daemon` is the runtime composition root and service crate. Its `thewayd` binary assembles [`theway-core`](../theway-core), [`theway-storage`](../theway-storage), [`theway-transport`](../theway-transport), provider integrations, and MCP/LSP support.

The daemon is the only workspace crate that depends directly on `theway-core`. Core owns runtime mechanics; the daemon supplies concrete tool bodies and executors, adapts persisted session/DAG records, manages triggers and cron jobs, and serves gRPC, HTTP/SSE, WebSocket, and MCP endpoints.

Client form is outside the daemon boundary. [`theway-tui`](../theway-tui) connects through transport contracts and contains terminal rendering and interaction. The daemon exposes client-coordination state through snapshots and events without importing TUI code.

## Runtime composition

- `src/bin/thewayd.rs` resolves CLI paths and starts the service.
- `src/turn/` assembles session harnesses and executes turns.
- `src/tools/` contains model-facing tool implementations.
- `src/executor/` implements the core `ToolExecutor` seam.
- `src/runtime_storage.rs` and `src/dag_persist.rs` adapt core runtime state to storage and leaf contract records.
- `src/trigger_engine/` and `src/triggers/` own automation execution, including cron jobs.
- `src/transport_adapter.rs` maps runtime state to transport-owned wire types.

The `local` and `sandbox` features select controller-side execution compatibility modes; transport behavior remains client-agnostic.

```bash
cargo run -p theway-daemon --bin thewayd -- --help
cargo test -p theway-daemon
```

See [`docs/architecture.md`](../../docs/architecture.md) for the workspace dependency rules and runtime boundaries.
