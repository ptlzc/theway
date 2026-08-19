# theway-daemon architecture

English | [中文](architecture.zh.md)

## Application role

`theway-daemon` is the only workspace crate that directly consumes `theway-core`. It supplies concrete host behavior and adapts core state to the persistence and protocol crates. Reusable runtime mechanics stay in core; wire representations stay in `theway-transport`; terminal interaction stays in `theway-tui`.

[`src/lib.rs`](../src/lib.rs) exposes a narrow application API around `DaemonOptions`, `DaemonServices`, `DaemonTransport`, `SessionSelection`, `DaemonPaths`, and `run`. Most orchestration and state modules remain crate-private.

## Startup composition

[`orchestration/startup.rs`](../src/orchestration/startup.rs) is the application composition path:

1. Set the resolved working directory and select local or remote `RuntimeStorage`.
2. Create or resume a raw session store and initialize logging and telemetry.
3. Resolve model configuration and build the provider stream function.
4. Create process-lifetime services, load trigger/cron state, and construct the DAG engine and subagent-job registry with one shared observer.
5. Select a `ToolExecutor`, load optional MCP/LSP/hooks/templates/skills/extensions sources, and assemble model-facing tools.
6. Build the initial `SessionRuntime`, create the `TurnHost`, and hand it to the selected gRPC, HTTP, or MCP server lifecycle.

[`paths.rs`](../src/paths.rs) resolves base, home, work directory, and additional skill directories at the CLI boundary. Runtime modules receive `DaemonPaths` or explicit paths rather than resolving `HOME`, `THEWAY_DIR`, or the process current directory independently.

[`orchestration/services.rs`](../src/orchestration/services.rs) owns process-lifetime mutable services such as trigger and cron registries, notification hooks, and command output. Tests and embedders replace behavior by constructing `DaemonServices`, not by modifying process globals.

## Session runtime lifecycle

[`orchestration/session.rs`](../src/orchestration/session.rs) owns `SessionRuntimeBuilder`. Initial startup, resume, and session switching all pass through the same builder, which:

- opens an injected `SessionStore` through `SessionRepository`;
- adapts it with `theway-core::PersistentSessionStorage`;
- validates the persisted working-directory binding;
- constructs `AgentHarness`, trigger execution, graph persistence, job transcripts, hooks, and notification registrations for that session;
- optionally rehydrates typed runtime state from the active persisted branch.

[`turn/kernel.rs`](../src/turn/kernel.rs) provides `ReplKernel`, which admits one active prompt/continuation, owns queued turns, and replaces the complete runtime when switching sessions. [`turn/daemon.rs`](../src/turn/daemon.rs) owns the protocol-neutral daemon state machine, command routing, snapshots, feed updates, and lifecycle event handling.

## Storage ownership

[`runtime_storage.rs`](../src/runtime_storage.rs) defines daemon application ports:

- `RuntimeStorage` supplies session repositories, DAG snapshots, job transcripts, trigger rules, cron jobs, and a persistence sink.
- `SessionRepository` supplies create, resume, open, list, delete, fork, and import operations using `Arc<dyn SessionStore>` rather than a concrete database type.

The local adapter uses `theway-storage`. `RemoteRuntimeStorage` uses the storage RPC operations from `theway-transport`. Orchestration code depends on these daemon traits and does not expose SQLite types.

## Tools and host integrations

[`tools/mod.rs`](../src/tools/mod.rs) contains model-facing tool implementations and assembly. Filesystem, command, git, search, memory, skill, MCP, web, subagent, and DAG tools are daemon-owned because they combine core tool interfaces with host policy and external services.

[`executor/mod.rs`](../src/executor/mod.rs) implements `theway-core::ToolExecutor`. The default `local` feature provides `LocalExecutor`; `sandbox` without `local` provides a fail-fast placeholder. [`forwarding_tool_ops.rs`](../src/forwarding_tool_ops.rs) is a separate protocol adapter that sends `ToolOps` requests to the controller address in `WireDaemonConfig` and refreshes its cached client when that address changes.

[`hooks/mod.rs`](../src/hooks/mod.rs), [`hook_executors.rs`](../src/hook_executors.rs), [`trigger_engine/mod.rs`](../src/trigger_engine/mod.rs), and [`triggers/mod.rs`](../src/triggers/mod.rs) own process/webhook effects, dynamic trigger polling and promotion, cron execution, and notification delivery. Persisted sidecar records come from `theway-contract`; scheduling and delivery policy remains here.

[`mcp_loader.rs`](../src/mcp_loader.rs) uses `theway-mcp` to discover external MCP tools and notifications. [`mcp_server.rs`](../src/mcp_server.rs) exposes the daemon as an MCP server. [`lsp_supervisor.rs`](../src/lsp_supervisor.rs) owns language-server process lifecycle.

## Protocol adaptation

[`transport_adapter.rs`](../src/transport_adapter.rs) converts core DAG runs, nodes, job state, and events into transport-owned wire snapshots and implements `GraphOps` and `JobOps`. The transport crate receives `TransportEndpoints` and `TransportHost`; it does not access `AgentHarness` or daemon-private state.

Cross-client behavior starts with a type or operation in `theway-transport`. The daemon implements protocol-side semantics and emits snapshots or events. Appearance, key handling, layout, and local interaction remain client-owned.

## Observability

[`observability.rs`](../src/observability.rs) implements core's `RuntimeObserver` using a bounded non-blocking queue. The worker emits structured logs, OpenTelemetry traces and metrics, and Prometheus measurements without putting prompts, messages, tool arguments, tool results, generated text, or raw error strings into observation records.

One observer instance is injected into the primary and resumed harnesses, `SubagentJobRegistry`, and `DagEngine`. Exporter or queue failures do not change runtime results, and shutdown drains the worker within bounded timeouts.

## Invariants

- Session construction has one `SessionRuntimeBuilder` path for startup and switching.
- Process services and storage implementations are injected through owned handles and traits rather than hidden globals or concrete SQLite types.
- The daemon owns runtime semantics but no client presentation state.
- Protocol conversion occurs in daemon adapters against transport-owned messages.
- Host paths are resolved once and passed explicitly.
- Tool, trigger, hook, MCP, LSP, and telemetry failures report through their owning operation without corrupting the session runtime lifecycle.
