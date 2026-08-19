# theway-core

`theway-core` is the reusable agent runtime composed by [`theway-daemon`](../theway-daemon/README.md). It owns the single-agent loop, `AgentHarness`, typed runtime sessions, skills and prompt assembly, compaction, lifecycle and permission hooks, the `ToolExecutor` and `RuntimeObserver` interfaces, and multiagent DAG/goal orchestration.

Core does not own concrete tools, filesystem or process implementations, persistence backends, telemetry exporters, or protocol servers. The workspace layering check permits [`theway-daemon`](../theway-daemon/README.md) as its only direct runtime consumer.

## Public entry points

- `Agent` and `AgentOptions` run the provider-neutral message and tool loop.
- `AgentHarness` composes an agent with a typed `Session`, skills, compaction, cost tracking, and cross-turn hooks.
- `PersistentSessionStorage` adapts typed session entries to the raw `SessionReader` and `SessionStore` records from [`theway-contract`](../theway-contract/README.md).
- `ToolExecutor` defines filesystem and process effects supplied by an embedding runtime.
- `RuntimeObserver` receives transport-neutral operation start and finish records.
- `multiagent` provides nested agent runs, live subagent-job state, DAG scheduling, and goal evaluation when the `harness` feature is enabled.

## Features

The default build enables `harness` and `default-providers`. `harness` includes sessions, skills, compaction, permissions, hooks, and multiagent orchestration; `default-providers` enables the Anthropic and faux provider implementations in [`theway-llm-provider`](../theway-llm-provider/README.md).

```bash
# Bare Agent loop
cargo check -p theway-core --no-default-features

# Harness without concrete providers
cargo check -p theway-core --no-default-features --features harness
```

## Documentation

- [Runtime architecture and extension interfaces](docs/architecture.md)
- [Workspace architecture](../../docs/architecture.md)
- [Test-file layout](../../docs/rust-test-files.md)

## Validation

```bash
cargo test -p theway-core
cargo doc -p theway-core --no-deps --document-private-items
make layering-check
```
