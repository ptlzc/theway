# theway-daemon architecture

English | [中文](architecture.zh.md)

## Application role

`theway-daemon` is the only workspace crate that directly consumes `theway-core`. It supplies concrete host behavior and adapts core state to the persistence and protocol crates. Reusable runtime mechanics stay in core; wire representations stay in `theway-transport`; terminal interaction stays in `theway-tui`.

[`src/lib.rs`](../src/lib.rs) exposes a narrow application API around `DaemonOptions`, `DaemonServices`, `DaemonTransport`, `SessionSelection`, `DaemonPaths`, and `run`. Most orchestration and state modules remain crate-private.

## Startup composition

[`orchestration/startup/mod.rs`](../src/orchestration/startup/mod.rs) is the application composition path:

1. Set the resolved working directory and select local or remote `RuntimeStorage`.
2. Create or resume a raw session store and initialize logging and telemetry.
3. Resolve model configuration and build the provider stream function.
4. Create process-lifetime services, load trigger/cron state, and construct the DAG engine and subagent-job registry with one shared observer.
5. Select a `ToolExecutor`, load optional MCP/LSP/hooks/templates/skills/extensions sources, and assemble model-facing tools.
6. Build the initial `SessionRuntime`, create the `TurnHost`, and hand it to the selected gRPC, HTTP, or MCP server lifecycle. When remote controller storage is configured, supervise that lifecycle with bounded storage probes.

[`paths.rs`](../src/paths.rs) resolves base, home, work directory, and additional skill directories at the CLI boundary. Runtime modules receive `DaemonPaths` or explicit paths rather than resolving `HOME`, `THEWAY_DIR`, or the process current directory independently.

[`orchestration/services.rs`](../src/orchestration/services.rs) owns process-lifetime mutable services such as trigger and cron registries, notification hooks, command output, and the per-project subagent settings registry ([`subagent_settings.rs`](../src/subagent_settings.rs), the last-set model/thinking memory shared by `dag_plan` and the `subagent` tool). Tests and embedders replace behavior by constructing `DaemonServices`, not by modifying process globals.

## Session runtime lifecycle

[`orchestration/session.rs`](../src/orchestration/session.rs) owns `SessionRuntimeBuilder`. Initial startup, resume, and session switching all pass through the same builder, which:

- opens an injected `SessionStore` through `SessionRepository`;
- adapts it with `theway-core::PersistentSessionStorage`;
- validates the persisted working-directory binding;
- constructs `AgentHarness`, trigger execution, graph persistence, job transcripts, hooks, and notification registrations for that session;
- optionally rehydrates typed runtime state from the active persisted branch;
- supplies the persisted session id and daemon working directory to the core runtime-extension context and starts the session lifecycle after reconstruction.

`SessionExecutionContext` owns the canonical cwd, thinking level, project resources, MCP resources, hook resources, and extension resources for each session. `SessionExecutionContext::build_for_work_dir` canonicalizes an arbitrary requested work directory, derives cwd-scoped paths, and constructs the executor/resources/hooks/extension host without changing the process cwd. `SessionRuntimeBuilder::build_opened` uses the context's thinking level for the harness.

[`turn/kernel.rs`](../src/turn/kernel.rs) provides `ReplKernel`, which admits one active prompt/continuation, owns queued turns, and replaces the complete runtime when switching sessions. [`turn/daemon.rs`](../src/turn/daemon.rs) owns the protocol-neutral daemon state machine, command routing, snapshots, feed updates, and lifecycle event handling.

Session activation is the serialized atomic create-or-resume boundary for new clients. `SessionActivator` validates the client key and exact canonical work directory, builds the complete session runtime before mutating any process state, then commits the binding and replaces the active harness on the event loop before replying. Credentials installed through `SetCredential` / `ClearCredential` live in a zeroizing `SessionExecutionRegistry` keyed by session/provider; they are never persisted, are cleared when a session is deleted, and are zeroized during daemon shutdown.

Session switching invokes the current harness's extension gate before constructing a target runtime. An active turn is cancelled and driven through settlement before the old runtime sends `session_shutdown`; only then does `ReplKernel::replace_runtime` activate the reconstructed target and publish `session_switched`. The `/fork` command invokes the fork gate before `SessionRepository::fork` and publishes `session_forked` only after the new session metadata is readable. A rejected gate therefore leaves the current runtime and session repository unchanged.

## User input admission and display projection

[`attachments/mod.rs`](../src/attachments/mod.rs) owns admission for one round of input. [`attachments/files.rs`](../src/attachments/files.rs) turns mention text into `File` parts with the shared `theway_transport::mentions` parser and truncation window, and [`attachments/images.rs`](../src/attachments/images.rs) decodes and validates submitted images with the shared `theway_transport::images` checks. `PromptAdmission::admit` writes every byte into `DaemonServices.attachments` before it returns, so a persisted `UserInput` never names content that was not stored, and `PromptAdmission::with_injected` appends the skill, trigger, or extension part after the attachments.

A `/skill` turn reaches admission as the `attach_skill_prompt(text, Some(name))` envelope. `PromptAdmission::admit` splits it with `theway_transport::commands::split_skill_prompt`, so the record's `text` is the user's own text and the envelope preamble from `skill_prompt_preamble` becomes the `Injected { source: "skill", name }` part, while the model-facing prompt the caller already holds is never rewritten.

[`orchestration/startup/process_state.rs`](../src/orchestration/startup/process_state.rs)'s `start_process_services` receives the resolved `DaemonPaths::base` and passes it to `DaemonServices::with_attachments_base` in [`orchestration/services.rs`](../src/orchestration/services.rs), which roots `DaemonServices.attachments` at `<base>/attachments/v1` for the process lifetime, so a `--theway-dir` override moves the whole library with the rest of the base-directory layout.

The active-session and `submit_web_text_for_session` intake paths carry the record together with the prompt, and the steering path queues it through `AgentHarness::enqueue_steering_input`. Core appends the record before the user message, and the display projection depends on that order.

Command-synthesised prompts carry a record too. [`turn/daemon/input.rs`](../src/turn/daemon/input.rs)'s `command_prompt_record` records a `CommandOutcome::RunAgentPrompt` prompt built from a skill envelope as `InputSource::User` with the same injected part, and every other synthesised prompt — a goal command, a trigger-authoring command, a file command, or host-injected text — as `InputSource::Host`; [`turn/daemon/commands/triggers.rs`](../src/turn/daemon/commands/triggers.rs)'s `trigger_web_rule_now` records the `WireCommand::TriggerRuleNow { id }` turn as `InputSource::Trigger` with the rule id in `source_ref`.

Trigger and cron injection builds its record through `trigger_record_message` in [`trigger_engine/execution/promotion.rs`](../src/trigger_engine/execution/promotion.rs): the model-facing message keeps the `[Trigger <trace_id>] ` prefix `ensure_trigger_prefix` enforces, while the record's `text` has that prefix stripped, `source` is `InputSource::Trigger`, and `source_ref` is the trace id. `apply_promotion` and the inject-and-run action in [`trigger_engine/execution/action.rs`](../src/trigger_engine/execution/action.rs) write that record immediately before the user message on both branches — the streaming branch hands record then message to `Agent::enqueue_follow_up`, and the idle branch appends record then message to the session transcript and the agent's in-memory state — so the two entries stay adjacent in the log.

The live turn path emits its blocks through the shared `theway_transport::feed::user_input_blocks` mapping, [`feed_replay.rs`](../src/feed_replay.rs)'s `replay_transcript` rebuilds them from session entries, and the paging path in [`session_observability.rs`](../src/session_observability.rs) projects the same record: the user block carries the submitted text, the attachment chips, and the origin, and each injected part becomes a context row. Because a record describes the `Message::User` entry written immediately after it, replay and paging skip that message; paging resolves the paired set over the whole branch before cutting a page, so a page boundary cannot render a paired message as a record-less user bubble, while `total` and the cursor keep counting message entries.

## Storage ownership

[`runtime_storage.rs`](../src/runtime_storage.rs) defines daemon application ports:

- `RuntimeStorage` supplies session repositories, DAG snapshots, job transcripts, trigger rules, cron jobs, and a persistence sink.
- `SessionRepository` supplies create, resume, open, list, delete, fork, and import operations using `Arc<dyn SessionStore>` rather than a concrete database type.

The local adapter uses `theway-storage`. `RemoteRuntimeStorage` uses the storage RPC operations from `theway-transport`. Orchestration code depends on these daemon traits and does not expose SQLite types.

A daemon configured with controller storage is valid only while that storage service remains reachable. [`orchestration/startup/mod.rs`](../src/orchestration/startup/mod.rs) completes a service-scoped gRPC health check once per second, resets the failure count after recovery, and logs the recovery. Three consecutive failed probes end the protocol lifecycle and shut the daemon down normally; shutdown flushes DAG persistence, aborts active graph runs, drains telemetry, and removes the discovery entry only when it still belongs to that process.

## Tools and host integrations

[`tools/mod.rs`](../src/tools/mod.rs) contains model-facing tool implementations and assembly. Filesystem, command, git, search, memory, skill, MCP, web, subagent, and DAG tools are daemon-owned because they combine core tool interfaces with host policy and external services.

[`executor/mod.rs`](../src/executor/mod.rs) implements `theway-core::ToolExecutor`. The execution environment is selected at runtime from `[executor] kind` in `config.toml` (issue #123): `local` provides `LocalExecutor`, `sandbox` provides a fail-fast placeholder and the runtime tool assembly omits direct-OS tools. [`forwarding_tool_ops.rs`](../src/forwarding_tool_ops.rs) is a separate protocol adapter that sends `ToolOps` requests to the controller address in `WireDaemonConfig` and refreshes its cached client when that address changes.

[`hooks/mod.rs`](../src/hooks/mod.rs), [`hook_executors.rs`](../src/hook_executors.rs), [`trigger_engine/mod.rs`](../src/trigger_engine/mod.rs), and [`triggers/mod.rs`](../src/triggers/mod.rs) own process/webhook effects, dynamic trigger polling and promotion, cron execution, and notification delivery. Persisted sidecar records come from `theway-contract`; scheduling and delivery policy remains here.

[`mcp_loader.rs`](../src/mcp_loader.rs) uses `theway-mcp` to discover external MCP tools and notifications from `paths.base/mcp.toml` and `paths.work_dir/.theway/mcp.toml`; stdio servers start in `paths.work_dir`, and HTTP auth reads `paths.base/auth.json`. MCP tools, hooks, inject sets, and capability metadata are owned by the `SessionExecutionContext`; TS extension catalog, legacy compaction host, compact registry, and engine pool are likewise session-scoped on the context. [`mcp_server.rs`](../src/mcp_server.rs) exposes the daemon as an MCP server. [`lsp_supervisor.rs`](../src/lsp_supervisor.rs) owns language-server process lifecycle.

Templates, LSP config, and hooks are also discovered from `paths.base` and `paths.work_dir/.theway` via [`templates.rs`](../src/templates.rs), [`lsp_supervisor.rs`](../src/lsp_supervisor.rs), and [`hooks/mod.rs`](../src/hooks/mod.rs); hook runner cwd modes use the same explicit `work_dir`, `base`, and `home` values.

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
- Attachment bytes are written before the record that names them is persisted, and every display surface projects that record rather than the materialized message.
- Every recorded round is written immediately before the user message it describes, and a trigger round's record holds the body without the model-facing `[Trigger <id>] ` prefix.
- Tool, trigger, hook, MCP, LSP, and telemetry failures report through their owning operation without corrupting the session runtime lifecycle.
