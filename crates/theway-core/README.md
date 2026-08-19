# theway-core

`theway-core` is the runtime engine composed by [`theway-daemon`](../theway-daemon). Within this workspace, the daemon is its only direct consumer; [`scripts/check-workspace-layering.py`](../../scripts/check-workspace-layering.py) enforces that boundary.

Core owns the single-agent loop, `AgentHarness`, typed runtime sessions, skills and prompt assembly, compaction, lifecycle and permission hooks, the `ToolExecutor` interface, and the multiagent DAG/goal engine. It does not own concrete tools, filesystem/process executor implementations, persistence backends, or protocol servers.

## Persistence boundary

Runtime session entries remain typed inside core. [`PersistentSessionStorage`](src/agent/session/persistent_storage.rs) converts them to and from the backend-neutral `SessionReader` / `SessionStore` records in [`theway-contract`](../theway-contract); [`theway-storage`](../theway-storage) implements those leaf interfaces without depending on core.

The DAG engine exposes persisted snapshots through [`multiagent::graph::persist`](src/multiagent/graph/persist.rs). The daemon projects engine state into `theway-contract` records and passes those records to storage.

## Features

The default build enables `harness` and `default-providers`. `harness` contains sessions, skills, compaction, permissions, and multiagent orchestration; `default-providers` enables the Anthropic and faux provider implementations.

```bash
# Bare Agent loop
cargo check -p theway-core --no-default-features

# Harness without concrete providers
cargo check -p theway-core --no-default-features --features harness
```

## Layout

```text
src/
  lib.rs                 public runtime surface
  types.rs               agent messages, events, hooks, and tool contracts
  executor.rs            ToolExecutor interface and execution result types
  agent.rs               bare Agent state machine
  agent/
    assembly/            AgentHarness composition
    run_loop/            LLM and tool-call loop
    compaction/          context estimation and summarization
    session/             typed sessions, in-memory stores, persistence adapter
    cost.rs              token and cost accounting
    messages.rs          custom message helpers
    permission.rs        tool permission classification
    skills.rs            SKILL.md parsing and loading
    system_prompt.rs     skill catalog rendering
    types.rs             harness-specific types and ExecutionEnv seam
  multiagent/            nested runs, registry, and DAG/goal orchestration
```

Substantial unit suites live under [`tests/`](tests) and are bridged from their source modules so they retain private-module access. See [`docs/rust-test-files.md`](../../docs/rust-test-files.md).

```bash
cargo test -p theway-core
```
