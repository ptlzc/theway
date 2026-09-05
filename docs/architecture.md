# Architecture: daemon runtime core, single daemon kernel, protocol + clients

theway is a three-layer agent runtime:

1. **Daemon runtime core** — `theway-core` on top of `theway-contract` and `theway-llm-provider`: the `Agent` loop, `AgentHarness`, typed runtime sessions, multiagent DAG engine machinery, and the `ToolExecutor` seam used by executor-backed tool effects. The daemon is its only direct workspace consumer.
2. **Daemon kernel** — `theway-daemon` (bin `thewayd`): the single kernel.
   Harness assembly, the executor implementations with the local/sandbox tool
   policy, all tool bodies, the trigger/cron runtime, session lifecycle, DAG
   persistence, skills/templates, MCP/LSP wiring, and the transport servers.
3. **Transport protocol + clients** — `theway-transport` (wire model,
   gRPC/HTTP/SSE/WS carriers, and the shared client-contract modules) and
   `theway-tui` (the ratatui client binary `theway`).

`theway-contract` is the shared leaf contract for raw session persistence, persisted DAG snapshots, sidecar models, and paths. `theway-storage` implements those persistence contracts and depends only on `theway-contract` among runtime workspace crates; the daemon adapts storage records to core runtime types.

## Crate layout

| Crate | Package | Role |
|-------|---------|------|
| [`crates/theway-core`](../crates/theway-core/README.md) | `theway-core` | Agent engine and harness, runtime ports, typed sessions, multiagent orchestration, and no host tool bodies or exporters. |
| [`crates/theway-daemon`](../crates/theway-daemon/README.md) | `theway-daemon` | `thewayd` composition root: runtime assembly, executors, tools, automation, persistence adapters, MCP/LSP/hooks, observability, and protocol servers. |
| [`crates/theway-transport`](../crates/theway-transport/README.md) | `theway-transport` | Cross-client wire model and operations carried over gRPC, HTTP/JSON-RPC, SSE, and WebSocket; no MCP implementation. |
| [`crates/theway-tui`](../crates/theway-tui/README.md) | `theway-tui` | `theway` terminal client/controller, daemon discovery, controller tool/storage services, and offline session commands. |
| [`crates/theway-contract`](../crates/theway-contract/README.md) | `theway-contract` | Leaf persistence records, interfaces, automation sidecars, and path derivation with no workspace dependencies. |
| [`crates/theway-extensions`](../crates/theway-extensions/README.md) | `theway-extensions` | Official runtime extension packages embedded as build-time data; the daemon provisions the shipped ones into the managed extensions layer. |
| [`crates/theway-storage`](../crates/theway-storage/README.md) | `theway-storage` | SQLite session/DAG persistence and session archives implementing contract interfaces without core or transport dependencies. |
| [`crates/theway-llm-provider`](../crates/theway-llm-provider/README.md) | `theway-llm-provider` | Normalized streaming LLM client, provider implementations, message transforms, and model/image catalogs. |
| [`crates/theway-mcp`](../crates/theway-mcp/README.md) | `theway-mcp` | External MCP stdio client, JSON-RPC framing, tool discovery, and tool calls. |
| [`crates/theway-probe`](../crates/theway-probe/README.md) | `theway-probe` | gRPC serviceability probe for health, watch, multi-session, and state checks. |
| [`crates/theway-markdown-core`](../crates/theway-markdown-core/README.md) | `theway-markdown-core` | Headless Markdown parser policy, analysis, statistics, and structural diagnostics. |
| [`crates/theway-markdown`](../crates/theway-markdown/README.md) | `theway-markdown` | Streaming terminal Markdown renderer with syntax, math, diagrams, links, and source mapping. |
| [`crates/theway-pager-render`](../crates/theway-pager-render/README.md) | `theway-pager-render` | Ratatui feed/pager line, color, scrollbar, OSC 8, and path primitives. |
| [`crates/theway-ratatui-textarea`](../crates/theway-ratatui-textarea/README.md) | `theway-ratatui-textarea` | Grapheme-aware multiline editing and ratatui widget state/rendering. |
| [`crates/mermaid-parser`](../crates/mermaid-parser/README.md) | `mermaid-rs-parser` | Vendored Mermaid parse stage used behind core's DAG flowchart adapter. |
| [`crates/tests-bridge-macro`](../crates/tests-bridge-macro/README.md) | `tests-bridge-macro` | Proc macro that anchors mirrored unit-test modules at the owning crate root. |

Dependency direction:

```text
theway-tui       ──► theway-transport, theway-storage, theway-contract
theway-daemon    ──► theway-core, theway-storage, theway-transport, theway-mcp,
                     theway-contract, theway-llm-provider            (bin `thewayd`)
theway-transport ──► theway-contract, theway-llm-provider    (never core/storage)
theway-storage   ──► theway-contract                         (never core/transport)
theway-core      ──► theway-contract, theway-llm-provider
theway-contract  ──► (none — pure leaf)
```

## Layer 1 — daemon runtime core (`theway-core`)

The core crate owns the agent runtime and the interfaces everything else
programs against:

- **Bare agent**: `Agent` + agent loop (`run_loop`), message/content types,
  the `AgentTool` / tool-call contract, lifecycle hooks, permission
  classification.
- **Harness layer** (default feature `harness`): `AgentHarness` composes
  Agent + session + skills + compaction + permission policy + lifecycle;
  session contracts (`SessionStorage`, `SessionRepo`, session metadata) with
  an in-memory default for embedders and tests.
- **Multiagent orchestration** (`multiagent`): DAG/goal graph engine,
  node launcher, job registry + live control, nested agent runner, goal-mode
  hook. The engine machinery lives here; the model-facing tool bodies
  (`dag_*`, `subagent`, skills, memory, MCP adapter) live in the daemon's
  tool assembly.
- **Runtime observation interface** (`theway_core::observability`): `RuntimeObserver` receives content-safe start/finish records for agent runs, turns, LLM requests, tool execution, compaction, subagent jobs, DAG runs, and DAG nodes. `AgentHarnessOptions`, `SubagentJobRegistry`, and `DagEngine` accept one shared observer; the no-op implementation keeps core usable without an exporter. Product streams (`LoopEvent`, `SessionEvent`, `SubagentJobEvent`, and `DagEvent`) keep their existing UI, persistence, and wire semantics.
- **Executor interface** (`theway_core::executor`): the [`ToolExecutor`]
  trait decouples tool effects from the runtime. Trait surface (async,
  object-safe, `Send + Sync`, shareable as `Arc<dyn ToolExecutor>`):

| Method | Effect |
|--------|--------|
| `kind()` | Reports the execution environment (`ExecutorKind::Local` / `Sandbox`). |
| `read_file(path)` | Read a file as UTF-8 text. |
| `write_file(path, content)` | Create/overwrite a file. |
| `run_command(cwd, argv, timeout)` | Run a process with a wall-clock timeout; returns captured `CommandOutput { stdout, stderr, exit_code }`. |
| `list_dir(path)` | List directory entry names. |
| `grep(pattern, path)` | Regex search under a path. |
| `find(glob, path)` | Glob file search under a path. |
| `git(args)` | Run a git invocation in the executor's repository context. |

The trait is a *seam*, not an implementation: core defines no local
fs/process behavior and ships no executor. Implementations are supplied by
the daemon kernel; tests and embedded consumers may provide their own.

## Layer 2 — the daemon kernel (`theway-daemon`)

`thewayd` is the single kernel: one process owns the harness, sessions,
tools, triggers, cron, and DAG runtime, and serves them over the transports.
The TUI and any other client are consumers of this kernel, never peers.

### Runtime observability

`thewayd` creates one `DaemonRuntimeObserver` in `crates/theway-daemon/src/bin/thewayd.rs` and injects it into the main and resumed harnesses, `SubagentJobRegistry`, and `DagEngine`. `crates/theway-daemon/src/observability.rs` uses a bounded non-blocking queue, maps core operation identities to parented OpenTelemetry spans, emits stable structured log fields, and records counters, histograms, active-operation gauges, token/activity measurements, and dropped-observation counts. Queue or exporter failures do not change runtime results.

The OpenTelemetry trace exporter uses OTLP over HTTP/protobuf and activates when `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is non-empty; the metric exporter activates when `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` is non-empty (a traces-only consumer such as Langfuse gets no metric exporter). The SDK resource uses `service.name=thewayd`, the workspace version, and a process-specific `service.instance.id`. Shutdown drains the observation queue and calls both SDK providers' `shutdown` methods within bounded timeouts.

Set `THEWAY_METRICS_ADDR` to a socket address such as `127.0.0.1:9464` to expose Prometheus text format at `GET /metrics`. This listener belongs to the daemon and is independent of the selected client transport. `THEWAY_OBSERVABILITY_QUEUE_CAPACITY` sets the bounded observation queue size and defaults to `4096`; invalid or zero values use the default.

Metric labels contain operation, outcome, stable error category, measurement, token direction, and the configured provider/model pair. Session, run, job, node, tool-call, trace, operation, agent, and tool identifiers are trace/log attributes only. By default, prompts, messages, tool arguments, tool results, generated text, and raw error strings are absent from runtime observations and exporter records; failures use `ErrorCategory` and `OperationOutcome`.

Set `THEWAY_OBSERVABILITY_FULL_CONTENT=true` to opt in to full-context export: operation producers attach JSON input/output payloads (`llm.request` normalized request + assistant message, `tool.execute` arguments + result, `multiagent.job` transcript, `dag.node`/`dag.run` task and output), and the OTLP worker writes them to `langfuse.observation.input` / `langfuse.observation.output` span attributes with `langfuse.observation.type` (`generation` or `span`) and a trace name on the root span. Content attributes are capped at one million characters; a clipped payload is flagged with `theway.content.truncated`. The gate is off by default, and Prometheus/structured logs stay content-free in both modes.

### Daemon path context

Every host path the kernel needs is resolved ONCE at the CLI boundary —
`DaemonPaths::from_cli` in `crates/theway-daemon/src/paths.rs`, called from
`bin/thewayd.rs` — and then handed to kernel modules as plain path values;
the environment (`HOME` / `THEWAY_DIR`) is consulted only inside `from_cli`.

| Field | Resolution |
|-------|------------|
| `base` | `$THEWAY_DIR` when set, else `<home>/.theway` — the theway base dir (`config.toml`, `skill-overrides.json`, `skills/`, `extensions/`, …). |
| `home` | the `--home` flag when given, else `$HOME` — the user-level `.agents` / `.claude` config + skill roots. |
| `work_dir` | the `--cwd` flag when given, else the process cwd — session repo + tool execution; canonicalized best-effort, and a failed canonicalize keeps the original value so the composition root can still fail with a "cd into …" error. |
| `extra_skill_dirs` | repeatable `--skills-dir` flags, kept in CLI order. The ONLY runtime-mutable part of the context: replaceable through the gRPC `SetSkillDirs` RPC (see [gRPC path context](#grpc-path-context)); shared behind an `Arc<RwLock<..>>` so every `DaemonPaths` clone observes the current value. |

`home` / `base` / `work_dir` are fixed at daemon startup: the TUI forwards
`--home` (when set) and each `--skills-dir` verbatim in the launch arguments
when it spawns `thewayd`; attaching to an already-running daemon never
changes that daemon's existing configuration. Consumers wired from the
context include the skill scan and the `/skills reload` closure, the skill
tool family, the config readers and skill overrides, TS extension discovery,
the file-command user root, and `SessionHarnessFactory`. The skill scan reads
the extras through `DaemonPaths::current_extra_skill_dirs()`, snapshotting
once per scan so a concurrent `SetSkillDirs` lands on the NEXT (hot-)reload,
never mid-scan.

**Skill scan roots.** `skills::skills_dirs(&DaemonPaths)` orders the roots
highest priority first; `skills::load_all` walks them in that order and the
FIRST loaded copy of a name wins (missing directories are skipped):

| Priority | Root | Source |
|----------|------|--------|
| 1 (highest) | each `--skills-dir` extra, in CLI order | `User` |
| 2 | `<work_dir>/.agents/skills` | `Project` |
| 3 | `<work_dir>/.theway/skills` | `Project` |
| 4 | `<work_dir>/.codex/skills` | `Project` |
| 5 | `<work_dir>/.claude/skills` | `Project` |
| 6 | `<base>/skills` — the native theway install target | `User` |
| 7 | `<home>/.agents/skills` | `User` |
| 8 | `<home>/skills` | `User` |
| 9 | `<home>/.codex/skills` | `User` |
| 10 (lowest) | `<home>/.claude/skills` | `User` |

Extras carry `SkillSource::User` (the enum has no dedicated Extra variant);
precedence comes purely from the scan order, while the source tag is for
administration/observability. Opt-in built-in skills (`--builtin-skill`,
`[builtin_skills]` in `config.toml`) merge below every filesystem root — a
same-name skill from any root shadows the built-in. `sandbox`-only builds
load no skills and log an explicit warn.

**Install / builder / remove targets agree with the scan.** The skill tool
family is constructed with explicit paths from `DaemonPaths::base`
(`tools::assembly::skill_family`), so no member reads `THEWAY_DIR` / `HOME`
at construction time: `install_skill` and `skill_builder` write
`<base>/skills/<name>/SKILL.md`, and `remove_skill` deletes only a direct
child of `<base>/skills` (the deletion target is derived from the resolved
skill's recorded file path, never from the caller-supplied name). Because
`<base>/skills` is scan root 6 above, an installed or built skill is
discovered by the next startup or `/skills reload` (the reload closure
captures the same `DaemonPaths`).

**Session ↔ work_dir binding.** A session's recorded `cwd` metadata is its
work_dir: creation stamps it with this daemon's work_dir. On switch,
`SessionHarnessFactory::build` validates the target session against this
daemon's work_dir (`check_work_dir_binding`) before any harness state is
touched: both sides are canonicalized before comparing (falling back to the
raw paths when canonicalization fails), a mismatch refuses the switch with an
error naming both directories, and a session without `cwd` metadata (legacy
data) passes through so historical sessions are never locked out.

**Exception — the discovery contract stays env-driven.**
`theway_contract::config::base_dir()` (`${THEWAY_DIR:-$HOME/.theway}`,
re-exported by `theway_transport::{client, config}`) is consulted at call
time on purpose for the client↔daemon discovery contract: the TUI/CLI client
derives the same per-cwd port file (`<base>/daemon-port-<cwd-hash>`) from
its own process environment to find a running daemon, and the inbox path
follows the same derivation — both sides must stay identical by
construction, so the call sites implementing that contract are exempt from
the CLI-boundary rule. Host surfaces outside the path context (prompt
templates, `mcp.toml`, `hooks.toml`, `models.json`, LSP config, log /
bug-report / export destinations, and the `/skills install` / `/skills
remove` command paths, which construct the tools through their default
constructors) take their base from the same shared contract derivation.

### gRPC path context

The path context is served on the gRPC `SessionService` as two RPCs
(issue #68):

| RPC | Semantics |
|-----|-----------|
| `GetPathContext` | Read-only snapshot: the startup-fixed `home` / `base` / `work_dir` plus the CURRENT skill search directories (`skills_dirs`). |
| `SetSkillDirs` | Replace the extra skill directories dynamically; returns once the command is queued (`accepted`). |

Both sides observe ONE shared `Arc<RwLock<WirePathContext>>`: `TurnHost`
seeds it from `DaemonPaths` at startup (startup extras = CLI `--skills-dir`)
and clones the same handle into `TransportEndpoints` for the servers.
`SetSkillDirs` applies in two steps:

1. **Optimistic** (gRPC handler): write the new dirs into the shared state —
   `GetPathContext` readers see them immediately — and enqueue
   `WireCommand::SetSkillDirs`.
2. **Authoritative** (serialized event loop): `TurnHost::handle_set_skill_dirs`
   replaces the `DaemonPaths` extras, refreshes the shared path context,
   aborts any in-flight turn (its context predates the new catalog), and
   hot-reloads the skill catalog through the harness's reload closure — the
   same loader the startup scan and `/skills reload` use, so the fresh scan
   picks the new extras up with the usual priority order.

`home` / `base` / `work_dir` are never mutated at runtime; an empty dir list
is a valid update (clears the extras). The HTTP/WS surfaces expose the same
two operations as JSON-RPC methods (`session.get_path_context` /
`session.set_skill_dirs`); MCP keeps its existing tool surface.

### Daemon settings view

The daemon configuration is served on the gRPC `SettingsService`
(issue #72, defined in `proto/settings.proto`):

| RPC | Semantics |
|-----|-----------|
| `GetConfig` | Read-only snapshot of the daemon's current configuration view (`DaemonConfig`): model selection (`provider` / `model` / `base_url` / `thinking`), skills (`builtin_skills` / `skills_dirs`), `trigger_poll_secs`, and `tui_max_feed_lines`. |
| `SetConfig` | Push a partial update; returns once the command is queued (`accepted`). |
| `Configure` | Same operation as `SetConfig`, kept so the JSON-RPC / WS surfaces can align on a `configure` verb. |

Update semantics follow proto3 presence: a present `optional` field replaces
the current value, an absent one keeps it; `repeated` fields apply only when
non-empty (clearing goes through the dedicated surfaces, e.g. `SetSkillDirs`
with an empty list).

Both sides observe ONE shared `Arc<RwLock<WireDaemonConfig>>`: `TurnHost`
seeds it at startup from the active model, the extra skill dirs, the
configured trigger poll interval, and `[tui] max_feed_lines`, and clones the
same handle into `TransportEndpoints` for the servers. `SetConfig` /
`Configure` apply in the same two steps as `SetSkillDirs`:

1. **Optimistic** (transport handler): merge the patch into the shared view —
   `GetConfig` readers see it immediately — and enqueue
   `WireCommand::Configure`.
2. **Authoritative** (serialized event loop): `TurnHost::handle_configure`
   merges the patch again and runs the per-field appliers that already exist
   on the loop — skills dirs through the `SetSkillDirs` path, model
   selection through the `SetModel` path, the dynamic-trigger poll interval,
   and the TUI scrollback cap (carried by the next snapshot). Fields without
   an applier yet land in the view for later phases (persisting to
   `config.toml` is deliberately deferred).

The HTTP/WS surfaces expose the same operations as JSON-RPC methods
(`settings.get_config` / `settings.set_config` / `settings.configure`, bare
names also accepted); the `set_config` / `configure` params accept either the
config fields directly or a nested `{"config": {...}}` object.

### Tool-operation surface

File/tool operations cross the transport on the gRPC `ToolService`
(issue #75, defined in `proto/tools.proto`): `ReadFile` / `WriteFile` /
`EditFile` / `ExecCommand` / `ListDir` / `Grep` / `Find`, cross-session
memory (`MemorySave` / `MemoryList` / `MemoryRead` / `MemoryForget`), and
two-phase `SkillInstall` (read-only preview unless `confirm`, same safety
model as the `install_skill` agent tool). `ExecCommand` is server-streaming:
zero or more `ExecOutputFrame` output chunks followed by the terminal exit
frame.

The server side delegates to the `ToolOps` handler seam
(`theway_transport::transport`): the transport crate converts proto
messages to the `WireTool*` serde twins (`crate::tools` codecs) and stays
free of FS/process policy. The daemon wires `ForwardingToolOps`
(`crates/theway-daemon/src/forwarding_tool_ops.rs`): it reads the
controller's `tool_service_addr` from the shared daemon config and
forwards every file/process operation to that endpoint. The TUI/controller
serves the endpoint with `LocalToolOps`
(`crates/theway-tui/src/local_tool_ops.rs`), which executes read/write/
edit/exec/list/grep/find/memory/skill-install on the client side.

The HTTP/WS surfaces expose the same operations as unary JSON-RPC methods
(`tool.read_file` / `tool.write_file` / `tool.edit_file` /
`tool.exec_command` / `tool.list_dir` / `tool.grep` / `tool.find` /
`tool.memory_*` / `tool.skill_install`, bare names also accepted);
`exec_command` collects the frame stream into the unary
`WireToolExecResult` shape. Errors map consistently on both surfaces:
`NotFound` → gRPC `NOT_FOUND` / `-32004`, `InvalidArgument` →
`INVALID_ARGUMENT` / `-32602`, anything else → `INTERNAL` / `-32000`.

### Executors and the tool policy

Issue #78: the daemon no longer executes file/process operations locally.
File/tool operations are forwarded over the transport to the controller's
`ToolService` endpoint; the controller-side executor is the one that
touches the local filesystem and process table. The daemon's `local` /
`sandbox` cargo features are retained as compatibility names for the
controller-side executor selection; the daemon build itself is
execution-agnostic and delegates through `ForwardingToolOps`.

The legacy `crate::executor::local::LocalExecutor` / `SandboxExecutor`
implementations remain for tests and controller-side embedding, but the
daemon's tool-operation surface no longer calls them directly.

All tool bodies live in `src/tools/`. The policy splits them by how they
reach the OS:

| Tools | `local` build | `sandbox`-only build |
|-------|---------------|----------------------|
| Executor-backed file/git tools: `read`, `write`, `edit`, `outline`, `git` | registered; effects go through `LocalExecutor` | registered; effects go through the `SandboxExecutor` seam and fail with `UnsupportedKind` |
| Direct-OS tools (`LOCAL_ONLY_TOOL_NAMES`): `bash`, `exec`, `get_output`, `kill_shell`, `write_to_process`, `ls`, `grep`, `find` | registered | **not registered — fail closed.** They bypass the `ToolExecutor` seam and would touch the host FS/process table directly, so a `tracing::warn` names every omitted tool; never a silent drop. |
| Network-only tools: `web_fetch`, `web_search` | registered | registered (no host FS/process side effects) |
| Environment-agnostic engine tools: `dag_*`, `subagent`, the read-only `skill` lookup, `reload`, MCP adapter, trigger/cron management | registered | registered |
| Direct-FS engine tools (`LOCAL_ONLY_ENGINE_TOOL_NAMES`): `memory`, `install_skill`, `skill_builder`, `set_skill_state`, `remove_skill` | registered | **not registered — fail closed**; the omitted names are logged |

`bash` and the `exec_shell` family keep their own process-group kill + cancel
semantics (the trait's `run_command` kills only the direct child), and
`ls` / `grep` / `find` use richer directory/walk surfaces than the trait
exposes — which is exactly why they are excluded rather than stubbed in
sandbox-only builds.

### Trigger / cron / session / DAG runtime

- **Trigger engine** (`trigger_engine` + `triggers`): dynamic trigger rules,
  dedup/cycle suppression, permission hooks, audit records, sub-agent
  execution and result promotion. Source adapters: local dynamic checks
  (polled on the configured interval), MCP server-push notifications
  (`NotificationHook`), and cron ticks.
- **Cron scheduler** (`triggers::cron`): session-scoped jobs stored in the
  session's `.cron.toml` sidecar; due jobs enter the serialized turn queue.
  `--stateful` jobs keep per-job loop notes (`.loop-<job-id>.md`) and report
  findings to the triage inbox.
- **Session lifecycle** (`session_ops`, `turn::session_factory`,
  `agent_session`): resume/create/switch/delete against the SQLite session
  repository; each session gets a fully-wired `AgentHarness`. Switching
  validates the session↔work_dir binding (see
  [Daemon path context](#daemon-path-context)).
- **DAG persistence** (`dag_persist`): debounced writer behind the core
  `DagPersistSink` contract; run state lives per session in
  `<cwd>/.pi/graph-engineering-state-<sessionId>.db`.
- **Supporting surfaces**: skills loading and prompt-template loading — the
  local scans in the standalone daemon (skills: multi-root priority scan,
  see [Daemon path context](#daemon-path-context); templates: dual-root
  project ↻ user). In controller-provisioned mode the daemon does zero local
  file IO for either catalog: the TUI scans and provisions both through the
  settings surface (`WireDaemonConfig.skills` / `templates`, carried on both
  gRPC and JSON-RPC). MCP loader + LSP supervisor, lifecycle hooks
  (`hooks`, `hook_executors`), TS extension host, and runtime observability exporters.

The daemon re-exports the shared client-contract modules
(`theway_transport::{auth, config, history, mentions}`) and the session
archive surface (`theway_storage::session_archive`) for its internal
`crate::…` paths; external consumers use the owning crates directly.

## Layer 3 — transport protocol + clients

### `theway-transport`

Two zones in one crate:

- **Protocol zone**: the wire model (`wire`) and the transports around it —
  gRPC (`grpc`, six domain services `CommandService` / `SessionService` /
  `SettingsService` / `ToolService` / `GraphEngineService` / `EventService`
  plus `grpc.health.v1.Health`;
  `SessionService` also serves the daemon path context — `GetPathContext` /
  `SetSkillDirs`, see [gRPC path context](#grpc-path-context);
  `SettingsService` serves the daemon configuration view — `GetConfig` /
  `SetConfig` / `Configure`, see [Daemon settings view](#daemon-settings-view);
  `ToolService` forwards file/tool operations — `ReadFile` / `WriteFile` /
  `EditFile` / `ExecCommand` (streaming) / `ListDir` / `Grep` / `Find` /
  `Memory*` / `SkillInstall`, see
  [Tool-operation surface](#tool-operation-surface)),
  HTTP/SSE/WS (`http` / `ws`), MCP server (`mcp`), the daemon-discovery
  client (`client`: per-cwd `<base>/daemon-port-<cwd-hash>` file, default
  port `44777`), and the inbox reader (`inbox`).
- **Shared zone**: client/daemon contract helpers that are not protocol —
  `auth`, `bug_report`, `commands` (slash-command framework + local command
  set), `config`, `feed`, `history`, `images`, `mentions`, `triggers`. The
  purest pieces (trigger/cron sidecar models and the path layout) are
  re-exported from `theway-contract`, so storage and the daemon can share
  them without depending on this crate.

### Single-version protocol: `SessionSnapshot` + `ExternalProtocolOps`

All three protocol surfaces (gRPC, HTTP JSON-RPC, MCP stdio) share one
non-streaming application service:

- `theway_transport::ExternalProtocolOps` combines `CommandOps`, `SessionOps`,
  `SessionObservabilityOps`, `GraphOps`, `ToolOps`, `StorageOps`, and
  `SettingsOps`. The daemon builds `DaemonExternalProtocolOps` once and
  injects the same `Arc<dyn ExternalProtocolOps>` into `GrpcState`,
  `HttpState`, and the MCP `ToolDispatcher`; each protocol only parses
  parameters, maps errors, and serializes results.
- `SessionSnapshot` is the only snapshot shape. `GetSnapshot` (gRPC),
  `session.get_snapshot` (JSON-RPC), and MCP `session_get_snapshot` return the
  authoritative current state: `runtime`, `feed`, `system_context`, `dags`,
  and `subagents` come from the live projection; `info`, graph nodes,
  `active_node_id`, and `lineage` come from the session resource plane.
- Full message history is not carried by the snapshot. `ListSessionMessages`
  (gRPC), `session.list_messages` (JSON-RPC), and MCP
  `session_list_messages` page through the active branch with a `limit` and an
  exclusive `before_entry_id` cursor; the server caps `limit` at 500 and
  returns `blocks`, `next_before_entry_id`, `has_more`, and `total`.
- `StreamEvents` pushes `SessionSnapshot` directly. The first frame and any
  lagged-resync frame are full authoritative snapshots; incremental frames
  clone the previous frame and update only the `feed` plane. SSE `/events`
  and WebSocket status frames use the serde twin `WireSessionSnapshot` JSON.
- The previous flat snapshot and history-read surface was removed; clients
  use `GetSnapshot` + `ListSessionMessages`.

### `theway-tui` — the terminal client

The `theway` binary is a pure client of the kernel: on startup it reuses a
running daemon (discovered via the per-cwd port file or the default port),
or spawns `thewayd` in the current directory and waits for readiness — when
spawning, it forwards `--home` (when set) and each repeatable `--skills-dir`
verbatim into the daemon launch arguments. It renders the conversation feed
(Markdown via `theway-markdown`), handles client-local surfaces (`/login`,
feed scrollback, resume picker), and forwards everything else to the daemon. The daemon keeps running after the
TUI exits; multiple clients can share one daemon.

**Offline session maintenance exception**: session archive export/import
(`theway session export|import`) and the standalone session queries
(`--list-sessions`, `--list-all-sessions`, `--delete-session`) run without
the daemon — the CLI opens the local SQLite session repository directly.
Standalone session queries try the running daemon's RPC first and fall back
to the local repo when no daemon is up; export/import always go repo-direct.

## Shared contract (`theway-contract`) and storage layering

`theway-contract` is a pure leaf crate — persistence interfaces and records, sidecar data models, and path functions; no engine, protocol, runtime, or workspace dependencies:

- `session` — backend-neutral raw session records plus `SessionReader` / `SessionStore`; `theway-core::PersistentSessionStorage` converts them to typed runtime entries.
- `dag` — engine-independent persisted DAG run/node snapshots and state-path layout; the daemon projects core engine state into these records.
- `triggers` — the session-scoped automation models (dynamic trigger rules,
  cron jobs) serialized into session sidecars and `.theway-session`
  archives. `theway_transport::triggers` re-exports them.
- `config` — the single base-dir / path-layout contract
  (`${THEWAY_DIR:-$HOME/.theway}`, `<base>/sessions/<cwd-hash>/…`,
  `cwd_hash`). `theway_transport::{client, config}` re-export it.

`theway-storage` implements durable backends directly against the leaf contract's records and interfaces:

- `sqlite_repo` / `sqlite_storage` — one Turso SQLite database per session and implementations of `SessionReader` / `SessionStore`:
  `<base>/sessions/<cwd-hash>/<uuidv7>.db` (a `meta` key/value table + an
  append-only `entries` table mirroring the session tree).
- `session_archive` — `.theway-session` export/import bundles (transcript +
  automation sidecars).
- `sqlite_dag` — persisted `PersistedRun` snapshots consumed by the daemon's core-to-storage adapter.

Storage's dependency rule is the layering guarantee here: it depends on `theway-contract` and **never** on `theway-core` or `theway-transport`. `scripts/check-workspace-layering.py` enforces this boundary and that `theway-daemon` remains core's only direct workspace consumer.

## Session storage layout

Base dir `${THEWAY_DIR:-$HOME/.theway}`; sessions are scoped per project by
a cwd hash:

| Path | What |
|------|------|
| `sessions/<cwd-hash>/<uuidv7>.db` | One SQLite database per session (append-only entry tree + metadata). |
| `sessions/<cwd-hash>/<uuidv7>.triggers.json` | Session-scoped dynamic trigger rules (same stem as the `.db`). |
| `sessions/<cwd-hash>/<uuidv7>.cron.toml` | Session-scoped cron jobs (same stem as the `.db`). |
| `sessions/<cwd-hash>/<uuidv7>.loop-<job-id>.md` | Loop notes kept by a stateful cron job (same stem as the `.db`). |
| `sessions/<cwd-hash>/<uuidv7>.endpoints.json` | Session-scoped endpoint bindings (same stem as the `.db`). |
| `inbox.jsonl` | Global triage inbox written by stateful loops. |
| `daemon-port-<cwd-hash>` | Port + pid of the daemon bound for that cwd. |

Sidecars are derived from the session database path by extension swap
(`Path::with_extension`), so a session's automation always travels with its
`<uuidv7>` stem.
