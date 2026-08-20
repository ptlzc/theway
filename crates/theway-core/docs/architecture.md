# theway-core architecture

English | [中文](architecture.zh.md)

## Responsibility and dependencies

`theway-core` depends on `theway-contract` for raw persisted records and on `theway-llm-provider` for normalized model messages and streams. The optional Mermaid parser is used by the `harness` feature to parse DAG plans.

The crate exposes runtime mechanisms and host interfaces. `theway-daemon` supplies tools, storage implementations, process and filesystem behavior, telemetry export, configuration sources, and protocol adaptation.

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

[`agent/assembly/mod.rs`](../src/agent/assembly/mod.rs) builds `AgentHarness` from a model, typed `Session`, skills, prompt templates, tools, hooks, an observer, a `RuntimeExtensionPort`, and optional provider stream override. The harness persists prompt-cycle state, emits `SessionEvent` records, tracks cost, reloads skills through an injected closure, and enforces the configured turn-continuation cap. `AgentHarnessOptions::new` installs `NoopRuntimeExtensionPort`, so an embedder that configures no extensions takes no extension-engine path.

[`agent/session/session.rs`](../src/agent/session/session.rs) defines typed append-only `SessionTreeEntry` values and derives the active branch. `MemorySessionStorage` supports isolated embedders and tests. `PersistentSessionStorage` encodes typed entries into `theway-contract::StoredSessionEntry`, delegates all I/O to an injected `SessionStore`, and filters opaque extension records from the typed transcript and model context.

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

## Runtime extension ports

[`agent/runtime_extensions`](../src/agent/runtime_extensions/mod.rs) defines one engine-independent `RuntimeExtensionPort` composed from session, run, request, message, tool, and compaction domain traits. Core invocations contain lifecycle correlation and JSON-compatible payloads but no discovered extension identifiers; the daemon-owned implementation translates one invocation to its session instances.

Every domain dispatcher verifies that the lifecycle event belongs to that core seam and validates the returned ABI action batch through `ExtensionHookContract`. Only class-specific `ValidatedRuntimeExtensionResult` variants reach a call site, so an embedding implementation cannot apply a message or tool mutation through an input seam. `RuntimeExtensionScopeAllocator` shares monotonic lifecycle sequences and stable session-qualified identifiers across its clones.

`PersistentSessionExtensionStatePort` converts validated durable actions to one parent-linked `StoredSessionEntry` batch and commits it through `SessionStore::append_entries`; replay always reads the selected persisted branch. `ExtensionModelContextProjection` filters private state and custom events, preserves model-context branch order, and replaces duplicate `(extension_id, context_id)` values in place so each stable item is model-visible once.

`AgentHarness` maps input, run, turn, context, model-selection, branch/session, fork, and session-boundary operations to these ports. An input command outcome stops before provider dispatch and is emitted as structured `SessionEvent::ExtensionCommandOutcome`; accepted input/context replacements preserve message roles and remain local to their declared seam. A `before_run` patch atomically persists its parent-linked messages before the agent emits `run_started`, while its system-prompt replacement is restored at the end of that run. Run terminal events are emitted after awaited transcript persistence in `run_ended`, optional `run_error`, then `run_settled` order.

Extension follow-ups use a separate 32-item, stable-id de-duplicated queue rather than the bare Agent's within-run queue. The harness consumes that queue only after `run_settled` and stops one prompt cycle after 16 extension-driven follow-up runs. A task-local dispatch guard rejects recursive lifecycle dispatch and runtime operations started synchronously from a hook. `shutdown_runtime_extensions` cancels the active run and waits for awaited loop listeners before `session_shutdown`.

Finalized user, assistant, and tool-result messages pass through the message transform before entering agent state or awaited session persistence; a transformed assistant is also the source for tool-call extraction. Message observations use one stable message id from start through streaming updates and finalization. Tool preflight remains in assistant source order, stops after the extension gate denies a call, and starts only admitted executions; parallel siblings still execute concurrently, while execution-end observations, tool-result transforms, and persisted tool-result messages finalize in source order.

[`agent/model_request.rs`](../src/agent/model_request.rs) defines `NormalizedModelRequestDraft`, which is assembled after context conversion from request-local system instructions, normalized messages, visible tool definitions, immutable executable-tool names, and supported generation options. `before_model_request` receives that complete draft before provider serialization; core accepts a replacement only when provider/model identity, tool catalog references, and generation bounds validate together. The accepted executable implementations travel with the model result, so tool dispatch cannot observe later registry changes and rejects any name excluded from that request without execution lifecycle events. `RuntimeRequestExtensionPort::has_request_hook` lets the no-subscriber path skip extension dispatch.

Compaction invokes the extension gate before selecting or running either the builtin or a registered algorithm, publishes failure for provider or persistence errors, and publishes success only after the compaction entry and in-memory state commit. `ExtensionModelContextProjection::compaction_messages` contributes each de-duplicated model-visible context item once to summarization without changing cut-point entry identity; private state and custom events are not projected.

## Extension rules

- Add provider protocols and model catalogs in `theway-llm-provider`, not in the agent loop.
- Add model-facing tool implementations and host integrations in `theway-daemon`; add only their reusable traits and data types here.
- Add a storage backend in `theway-storage` or another crate implementing the leaf traits; keep typed-entry conversion in `PersistentSessionStorage`.
- Add telemetry exporters in the embedding runtime by implementing `RuntimeObserver`.
- Add graph execution backends through `NodeLauncher` and persistence through `DagPersistSink`.

## Invariants

- Core remains independent of concrete storage, transport, telemetry, and host-execution libraries.
- Persisted state crosses the crate boundary through `theway-contract` records, never backend types.
- Core lifecycle ports never discover packages or evaluate extension code, and raw daemon action batches never bypass core validation.
- Private extension state remains outside typed session messages and model-context projection.
- Extension follow-ups cannot enter the active run before settlement or recurse without a bound.
- Finalized message replacements precede persistence and downstream tool extraction, and denied tools produce a model-visible error without execution lifecycle events.
- A normalized request replacement is atomic and request-local; visible definitions and executable references describe the same immutable tool catalog.
- Compaction input contains only typed session messages plus de-duplicated model-visible extension context, never private extension state.
- Cancellation produces a terminal runtime outcome and releases run admission and control handles.
- Event payloads and operation correlation remain deterministic enough for the daemon to project snapshots without accessing private core state.
