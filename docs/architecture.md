# Architecture: core interface, single daemon kernel, protocol + clients

theway is a three-layer agent runtime:

1. **Core interface + agent runtime** — `theway-core` on top of
   `theway-llm-provider`: the `Agent` loop, the `AgentHarness`, the session
   contracts, the multiagent DAG engine machinery, and the `ToolExecutor` seam
   used by executor-backed tool effects.
2. **Daemon kernel** — `theway-daemon` (bin `thewayd`): the single kernel.
   Harness assembly, the executor implementations with the local/sandbox tool
   policy, all tool bodies, the trigger/cron runtime, session lifecycle, DAG
   persistence, skills/templates, MCP/LSP wiring, and the transport servers.
3. **Transport protocol + clients** — `theway-transport` (wire model,
   gRPC/HTTP/WS/MCP transports, and the shared client-contract modules) and
   `theway-tui` (the ratatui client binary `theway`).

`theway-contract` is the shared leaf contract (sidecar data models + path
layout) used across layers; `theway-storage` holds the SQLite backends and
depends only on `theway-core` + `theway-contract` — never on the transport
stack.

## Crate layout

| Crate | Package | Role |
|-------|---------|------|
| `crates/theway-core` | `theway-core` | Agent engine: bare `Agent` + agent loop, `AgentHarness` layer (skills, prompt templates, sessions, compaction, permission policy), session storage contracts (`SessionStorage` / `SessionRepo`) with an in-memory default, multiagent orchestration (DAG/goal graph engine, job registry, nested runner), and the `ToolExecutor` trait (`theway_core::executor`). Defines no tool bodies and no local fs/process behavior. |
| `crates/theway-daemon` | `theway-daemon` | The single kernel (bin `thewayd`): harness assembly, executor implementations (`local` / `sandbox` features), all tool bodies (engine + local + web), trigger engine, cron scheduler, session ops, DAG persistence, skills/templates, MCP loader, LSP supervisor, hooks. Serves the gRPC / HTTP / MCP transports. |
| `crates/theway-transport` | `theway-transport` | Protocol layer: wire model + gRPC / HTTP/SSE / WebSocket / MCP transports, plus the shared client-contract modules (auth, bug report, commands, config, feed, history, images, mentions, triggers). |
| `crates/theway-tui` | `theway-tui` | Terminal client (the `theway` CLI binary): ratatui REPL, feed rendering, local commands. Connects to a running daemon or spawns `thewayd`; never links the daemon kernel. |
| `crates/theway-contract` | `theway-contract` | Shared contract leaf crate: session-scoped automation sidecar models (`triggers`) and the base-dir / cwd-hash path layout (`config`). No engine, no protocol, no runtime; depends on no workspace crate. |
| `crates/theway-storage` | `theway-storage` | Durable persistence: SQLite (Turso) session repository — one `<uuidv7>.db` per session — session archive export/import (`.theway-session`), and DAG run persistence. Depends on `theway-core` (contracts) and `theway-contract` (sidecar models + paths); never on `theway-transport`. |
| `crates/theway-llm-provider` | `theway-llm-provider` | Unified streaming LLM client and provider integrations (Anthropic / OpenAI / Google / Bedrock / Mistral and OpenAI-compatible endpoints). |
| `crates/theway-mcp` | `theway-mcp` | Minimal MCP client (stdio transport, JSON-RPC framing). |
| `crates/mermaid-parser` | `mermaid-rs-parser` | Vendored mermaid parser used for DAG specs. |

UI rendering crates consumed by the TUI: `theway-markdown` /
`theway-markdown-core` (streaming Markdown renderer), `theway-pager-render`
(render primitives), `theway-ratatui-textarea` (textarea widget).

Dependency direction:

```text
theway-tui       ──► theway-transport, theway-core, theway-storage, theway-llm-provider
theway-daemon    ──► theway-core, theway-storage, theway-transport, theway-mcp,
                     theway-contract, theway-llm-provider            (bin `thewayd`)
theway-transport ──► theway-core, theway-contract, theway-llm-provider
theway-storage   ──► theway-core, theway-contract            (never theway-transport)
theway-core      ──► theway-llm-provider
theway-contract  ──► (none — pure leaf)
```

## Layer 1 — core interface + agent runtime (`theway-core`)

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

### Executors and the tool policy

The kernel execution backend is a cargo feature:

- `local` (default): `crate::executor::local::LocalExecutor` drives tools
  straight against the local filesystem (`tokio::fs`) and process table
  (`tokio::process`).
- `sandbox`: `crate::executor::sandbox::SandboxExecutor` — every operation
  answers with an explicit `ExecutorError::UnsupportedKind` (the seam is
  wired; each call fails fast rather than touching the host).

`crate::executor::default_executor()` picks the executor by feature
(`local` wins when both are enabled); a build with neither feature is a
compile error.

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
- **Supporting surfaces**: skills loading (multi-root priority scan — see
  [Daemon path context](#daemon-path-context)), prompt-template loading
  (dual-root project ↻ user), MCP loader + LSP supervisor, lifecycle hooks
  (`hooks`, `hook_executors`), TS extension host, OTLP exporters.

The daemon re-exports the shared client-contract modules
(`theway_transport::{auth, config, history, mentions}`) and the session
archive surface (`theway_storage::session_archive`) for its internal
`crate::…` paths; external consumers use the owning crates directly.

## Layer 3 — transport protocol + clients

### `theway-transport`

Two zones in one crate:

- **Protocol zone**: the wire model (`wire`) and the transports around it —
  gRPC (`grpc`, four domain services `CommandService` / `SessionService` /
  `GraphEngineService` / `EventService` plus `grpc.health.v1.Health`;
  `SessionService` also serves the daemon path context — `GetPathContext` /
  `SetSkillDirs`, see [gRPC path context](#grpc-path-context)),
  HTTP/SSE/WS (`http` / `ws`), MCP server (`mcp`), the daemon-discovery
  client (`client`: per-cwd `<base>/daemon-port-<cwd-hash>` file, default
  port `44777`), and the inbox reader (`inbox`).
- **Shared zone**: client/daemon contract helpers that are not protocol —
  `auth`, `bug_report`, `commands` (slash-command framework + local command
  set), `config`, `feed`, `history`, `images`, `mentions`, `triggers`. The
  purest pieces (trigger/cron sidecar models and the path layout) are
  re-exported from `theway-contract`, so storage and the daemon can share
  them without depending on this crate.

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

`theway-contract` is a pure leaf crate — data models and path functions, no
engine, no protocol, no runtime, no workspace dependencies:

- `triggers` — the session-scoped automation models (dynamic trigger rules,
  cron jobs) serialized into session sidecars and `.theway-session`
  archives. `theway_transport::triggers` re-exports them.
- `config` — the single base-dir / path-layout contract
  (`${THEWAY_DIR:-$HOME/.theway}`, `<base>/sessions/<cwd-hash>/…`,
  `cwd_hash`). `theway_transport::{client, config}` re-export it.

`theway-storage` implements the durable backends against the core contracts
and the contract crate's models/paths:

- `sqlite_repo` / `sqlite_storage` — one Turso SQLite database per session:
  `<base>/sessions/<cwd-hash>/<uuidv7>.db` (a `meta` key/value table + an
  append-only `entries` table mirroring the session tree).
- `session_archive` — `.theway-session` export/import bundles (transcript +
  automation sidecars).
- `sqlite_dag` — the `DagPersistSink` backend consumed by the daemon's DAG
  persistence.

Storage's dependency rule is the layering guarantee here: it depends on
`theway-core` (traits + types) and `theway-contract` (sidecar models + path
layout), and **never** on `theway-transport` — session persistence must not
pull in the protocol stack.

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
