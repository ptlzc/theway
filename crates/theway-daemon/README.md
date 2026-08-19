# theway-daemon

`theway-daemon` is the headless application kernel and the `thewayd` binary. It composes [`theway-core`](../theway-core/README.md), [`theway-storage`](../theway-storage/README.md), [`theway-transport`](../theway-transport/README.md), [`theway-llm-provider`](../theway-llm-provider/README.md), and [`theway-mcp`](../theway-mcp/README.md) into one long-running service.

The daemon owns session runtime assembly, model-facing tools, local and sandbox executor selection, hooks, triggers and cron jobs, nested-agent orchestration, MCP/LSP integration, telemetry export, and protocol-side behavior. It has no client-form or terminal-presentation concepts; [`theway-tui`](../theway-tui/Cargo.toml) is one protocol client.

## Entry points

- `thewayd` parses process options and calls the public `run(DaemonOptions)` composition entry point.
- `DaemonPaths` resolves the base, home, working directory, and additional skill directories once at startup.
- `DaemonServices` owns process-lifetime registries and command output injection.
- `SessionRuntimeBuilder` is the internal construction path for initial, resumed, and switched session runtimes.
- Public modules expose supported extension points for executors, hooks, storage adapters, tools, templates, skills, triggers, and TypeScript extensions.

The default `local` feature selects `LocalExecutor`. A `sandbox`-only build selects `SandboxExecutor`, whose unsupported operations fail with `ExecutorError::UnsupportedKind`. The protocol server can also forward `ToolOps` to a controller-provided gRPC tool endpoint.

## Running and validation

```bash
cargo run -p theway-daemon --bin thewayd -- --help
cargo test -p theway-daemon
cargo doc -p theway-daemon --no-deps --document-private-items
```

See [the daemon architecture](docs/architecture.md) for startup, session, storage, tool, protocol, and observability ownership. The [workspace architecture](../../docs/architecture.md) defines cross-crate layering.
