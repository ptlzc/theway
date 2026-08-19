# theway-core architecture

English | [中文](architecture.zh.md)

## Responsibility and dependencies

`theway-core` depends on [`theway-contract`](../../theway-contract/README.md) for raw persisted records and on [`theway-llm-provider`](../../theway-llm-provider/README.md) for normalized model messages and streams. The optional Mermaid parser is used by the `harness` feature to parse DAG plans.

The crate exposes runtime mechanisms and host interfaces. [`theway-daemon`](../../theway-daemon/docs/architecture.md) supplies tools, storage implementations, process and filesystem behavior, telemetry export, configuration sources, and protocol adaptation.

## Single-agent execution

[`agent.rs`](../src/agent.rs) owns `Agent`, mutable `AgentState`, run admission, steering and follow-up queues, cancellation, and lifecycle subscriptions. Only one prompt or continuation may run for an `Agent`; concurrent admission returns `AgentRunError::AlreadyStreaming`.

[`agent/run_loop/mod.rs`](../src/agent/run_loop/mod.rs) drives each turn:

1. Convert runtime messages to provider messages and start a normalized LLM stream.
2. Apply stream updates to agent state and emit `LoopEvent` records.
3. Classify, authorize, and launch tool calls, allowing independent calls to run concurrently.
4. Append tool results, drain queued steering or follow-up messages according to `QueueMode`, and decide whether another turn is required.
5. Finalize partial output on cancellation or interruption so state and emitted events agree.

Tool bodies implement `AgentTool`. Host-level filesystem and process operations use the object-safe [`ToolExecutor`](../src/executor.rs) trait; the core crate provides no executor implementation.

## Harness and sessions

[`agent/assembly/mod.rs`](../src/agent/assembly/mod.rs) builds `AgentHarness` from a model, typed `Session`, skills, prompt templates, tools, hooks, an observer, and optional provider stream override. The harness persists prompt-cycle state, emits `SessionEvent` records, tracks cost, reloads skills through an injected closure, and enforces the configured turn-continuation cap.

[`agent/session/session.rs`](../src/agent/session/session.rs) defines typed append-only `SessionTreeEntry` values and derives the active branch. `MemorySessionStorage` supports isolated embedders and tests. `PersistentSessionStorage` encodes typed entries into [`theway-contract::StoredSessionEntry`](../../theway-contract/src/session.rs) and delegates all I/O to an injected `SessionStore`.

[`agent/compaction/mod.rs`](../src/agent/compaction/mod.rs) estimates context use, chooses a cut point, produces or invokes a summarizer, and records compaction metadata without knowing which persistence backend stores the session.

## Multiagent runtime

[`multiagent/runner.rs`](../src/multiagent/runner.rs) launches a fresh harness for one nested agent run, filters its tool set, enforces idle-timeout cancellation, and returns normalized output and usage.

[`multiagent/jobs.rs`](../src/multiagent/jobs.rs) owns `SubagentJobRegistry`, the bounded live view of nested jobs. It tracks lifecycle, metrics, messages, control handles for interrupt/steer, and optional transcript persistence through `JobTranscriptStore`.

[`multiagent/graph.rs`](../src/multiagent/graph.rs) owns DAG and goal-run scheduling:

- `model.rs` validates definitions, builds runs, derives downstream closure, and reconciles node readiness.
- `mermaid.rs` adapts Mermaid flowchart text to DAG definitions and renders run state.
- `engine.rs` owns run state, retry/skip/cancel transitions, events, persistence notification, and launcher injection.
- `scheduler.rs` selects ready nodes subject to concurrency and dependency state.
- `node_launcher.rs` adapts graph nodes to nested agent runs.
- `persist.rs` converts live runs to and from persistence records through an injected `DagPersistSink`.

[`multiagent/goal.rs`](../src/multiagent/goal.rs) stores goal state in the session and implements the turn-end evaluator that either completes the goal, pauses it, or requests another turn. DAG and goal runs share `DagEngine`; the run kind distinguishes their lifecycle rules.

## Observation and product events

[`observability.rs`](../src/observability.rs) defines `RuntimeObserver`, correlated operation identities, stable outcome and error categories, and `OperationScope`. Dropping an unfinished scope emits an abandoned finish record. Observer calls are isolated from runtime results, and the default observer is a no-op.

Observation records are not product event streams. `LoopEvent`, `SessionEvent`, `SubagentJobEvent`, and `DagEvent` carry runtime state to persistence, tools, and clients; `RuntimeObservation` carries content-safe operational measurements to an embedder-owned exporter.

## Extension rules

- Add provider protocols and model catalogs in [`theway-llm-provider`](../../theway-llm-provider/README.md), not in the agent loop.
- Add model-facing tool implementations and host integrations in [`theway-daemon`](../../theway-daemon/docs/architecture.md); add only their reusable traits and data types here.
- Add a storage backend in [`theway-storage`](../../theway-storage/docs/architecture.md) or another crate implementing the leaf traits; keep typed-entry conversion in `PersistentSessionStorage`.
- Add telemetry exporters in the embedding runtime by implementing `RuntimeObserver`.
- Add graph execution backends through `NodeLauncher` and persistence through `DagPersistSink`.

## Invariants

- Core remains independent of concrete storage, transport, telemetry, and host-execution libraries.
- Persisted state crosses the crate boundary through `theway-contract` records, never backend types.
- Cancellation produces a terminal runtime outcome and releases run admission and control handles.
- Event payloads and operation correlation remain deterministic enough for the daemon to project snapshots without accessing private core state.
