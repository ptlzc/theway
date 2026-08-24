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
6. Build the initial `SessionRuntime`, create the `TurnHost`, and hand it to the selected gRPC, HTTP, or MCP server lifecycle. When remote controller storage is configured, supervise that lifecycle with bounded storage probes.

[`paths.rs`](../src/paths.rs) resolves base, home, work directory, and additional skill directories at the CLI boundary. Runtime modules receive `DaemonPaths` or explicit paths rather than resolving `HOME`, `THEWAY_DIR`, or the process current directory independently.

[`orchestration/services.rs`](../src/orchestration/services.rs) owns process-lifetime mutable services such as trigger and cron registries, notification hooks, and command output. Tests and embedders replace behavior by constructing `DaemonServices`, not by modifying process globals.

## Session runtime lifecycle

[`orchestration/session.rs`](../src/orchestration/session.rs) owns `SessionRuntimeBuilder`. Initial startup, resume, and session switching all pass through the same builder, which:

- opens an injected `SessionStore` through `SessionRepository`;
- adapts it with `theway-core::PersistentSessionStorage`;
- validates the persisted working-directory binding;
- constructs `AgentHarness`, trigger execution, graph persistence, job transcripts, hooks, and notification registrations for that session;
- optionally rehydrates typed runtime state from the active persisted branch;
- supplies the persisted session id and daemon working directory to the core runtime-extension context and starts the session lifecycle after reconstruction.

[`turn/kernel.rs`](../src/turn/kernel.rs) provides `ReplKernel`, which admits one active prompt/continuation, owns queued turns, and replaces the complete runtime when switching sessions. [`turn/daemon.rs`](../src/turn/daemon.rs) owns the protocol-neutral daemon state machine, command routing, snapshots, feed updates, and lifecycle event handling.

Session switching invokes the current harness's extension gate before constructing a target runtime. An active turn is cancelled and driven through settlement before the old runtime sends `session_shutdown`; only then does `ReplKernel::replace_runtime` activate the reconstructed target and publish `session_switched`. The `/fork` command invokes the fork gate before `SessionRepository::fork` and publishes `session_forked` only after the new session metadata is readable. A rejected gate therefore leaves the current runtime and session repository unchanged.

## Storage ownership

[`runtime_storage.rs`](../src/runtime_storage.rs) defines daemon application ports:

- `RuntimeStorage` supplies session repositories, DAG snapshots, job transcripts, trigger rules, cron jobs, and a persistence sink.
- `SessionRepository` supplies create, resume, open, list, delete, fork, and import operations using `Arc<dyn SessionStore>` rather than a concrete database type.

The local adapter uses `theway-storage`. `RemoteRuntimeStorage` uses the storage RPC operations from `theway-transport`. Orchestration code depends on these daemon traits and does not expose SQLite types.

A daemon configured with controller storage is valid only while that storage service remains reachable. [`orchestration/startup.rs`](../src/orchestration/startup.rs) completes a service-scoped gRPC health check once per second, resets the failure count after recovery, and logs the recovery. Three consecutive failed probes end the protocol lifecycle and shut the daemon down normally; shutdown flushes DAG persistence, aborts active graph runs, drains telemetry, and removes the discovery entry only when it still belongs to that process.

## Tools and host integrations

[`tools/mod.rs`](../src/tools/mod.rs) contains model-facing tool implementations and assembly. Filesystem, command, git, search, memory, skill, MCP, web, subagent, and DAG tools are daemon-owned because they combine core tool interfaces with host policy and external services.

[`executor/mod.rs`](../src/executor/mod.rs) implements `theway-core::ToolExecutor`. The default `local` feature provides `LocalExecutor`; `sandbox` without `local` provides a fail-fast placeholder. [`forwarding_tool_ops.rs`](../src/forwarding_tool_ops.rs) is a separate protocol adapter that sends `ToolOps` requests to the controller address in `WireDaemonConfig` and refreshes its cached client when that address changes.

[`hooks/mod.rs`](../src/hooks/mod.rs), [`hook_executors.rs`](../src/hook_executors.rs), [`trigger_engine/mod.rs`](../src/trigger_engine/mod.rs), and [`triggers/mod.rs`](../src/triggers/mod.rs) own process/webhook effects, dynamic trigger polling and promotion, cron execution, and notification delivery. Persisted sidecar records come from `theway-contract`; scheduling and delivery policy remains here.

[`mcp_loader.rs`](../src/mcp_loader.rs) uses `theway-mcp` to discover external MCP tools and notifications from `paths.base/mcp.toml` and `paths.work_dir/.theway/mcp.toml`; stdio servers start in `paths.work_dir`, and HTTP auth reads `paths.base/auth.json`. MCP tools, hooks, inject sets, and capability metadata are owned by the `SessionExecutionContext`. [`mcp_server.rs`](../src/mcp_server.rs) exposes the daemon as an MCP server. [`lsp_supervisor.rs`](../src/lsp_supervisor.rs) owns language-server process lifecycle.

## Protocol adaptation

[`transport_adapter.rs`](../src/transport_adapter.rs) converts core DAG runs, nodes, job state, and events into transport-owned wire snapshots and implements `GraphOps` and `JobOps`. The transport crate receives `TransportEndpoints` and `TransportHost`; it does not access `AgentHarness` or daemon-private state.

Cross-client behavior starts with a type or operation in `theway-transport`. The daemon implements protocol-side semantics and emits snapshots or events. Appearance, key handling, layout, and local interaction remain client-owned.

## Observability

[`observability.rs`](../src/observability.rs) implements core's `RuntimeObserver` using a bounded non-blocking queue. The worker emits structured logs, OpenTelemetry traces and metrics, and Prometheus measurements without putting prompts, messages, tool arguments, tool results, generated text, or raw error strings into observation records.

One observer instance is injected into the primary and resumed harnesses, `SubagentJobRegistry`, and `DagEngine`. Exporter or queue failures do not change runtime results, and shutdown drains the worker within bounded timeouts.

## Invariants

- Session construction has one `SessionRuntimeBuilder` path for startup and switching.
- Session switch and fork gates run before target construction or persistence, and successful events run only after commit.
- Process services and storage implementations are injected through owned handles and traits rather than hidden globals or concrete SQLite types.
- A controller-backed daemon does not outlive the controller storage required to build and persist session runtimes.
- The daemon owns runtime semantics but no client presentation state.
- Protocol conversion occurs in daemon adapters against transport-owned messages.
- Host paths are resolved once and passed explicitly.
- Tool, trigger, hook, MCP, LSP, and telemetry failures report through their owning operation without corrupting the session runtime lifecycle.
