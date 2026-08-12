# Architecture: SDK split, execution environments, and the executor seam

> Current structure established by the openspec changes `tools-into-core`
> (tool/engine tool definitions moved into `theway-core`) and
> `sdk-split-local-sandbox` (SDK crate split + `ToolExecutor` abstraction).
> The former `crates/theway-server` crate no longer exists: it was renamed to
> `crates/theway-daemon`, and its client-facing surface was split out into
> `crates/theway-sdk` (package `theway`).

## Crate layout

| Crate | Package | Role |
|-------|---------|------|
| `crates/theway-sdk` | `theway` | Client SDK — the client-facing surface, layered by execution environment (`common` / `local` / `sandbox`). The TUI and external embedders depend on this crate. |
| `crates/theway-daemon` | `theway-daemon` | Headless daemon runtime (bin `thewayd`): harness assembly, local tool bodies, trigger engine, skills/templates, MCP loader, LSP supervisor. Consumes the `theway` SDK for the client-facing surface. |
| `crates/theway-tui` | `theway-tui` | Terminal UI (the `theway` CLI binary) — a pure client of the daemon. |
| `crates/theway-core` | `theway-core` | Agent engine: bare `Agent` + agent loop, runtime/harness extras (session storage, compaction, permission policy, skills), engine tools (DAG/subagents/memory/MCP adapter/exec shells), and the `ToolExecutor` trait (`theway_core::executor`). |
| `crates/theway-llm-provider` | `theway-llm-provider` | Unified streaming LLM client and provider integrations. |
| `crates/theway-storage` | `theway-storage` | Session storage: hybrid JSONL + SQLite repositories. |
| `crates/theway-transport` | `theway-transport` | Wire surfaces: gRPC / HTTP / MCP transports served by the daemon. |
| `crates/theway-mcp` | `theway-mcp` | Minimal MCP client (stdio transport, JSON-RPC framing). |
| `crates/mermaid-parser` | `mermaid-rs-parser` | Vendored mermaid parser used for DAG specs. |

Dependency direction:

```text
theway-tui ──► theway (SDK) ──► theway-core ──► theway-llm-provider
                  │                  ▲
                  ▼                  │
           theway-storage      theway-daemon ──► theway (SDK) + theway-core
                                   │              + theway-storage + theway-transport
                                   ▼
                              bin `thewayd`
```

**Client dependency boundary**: `theway-tui` depends on the SDK (`theway`), not
on `theway-daemon` — the client's dependency graph contains zero daemon runtime
code (no tools, trigger engine, skills loader, MCP/LSP wiring). The daemon
depends on the SDK and re-exports the client-facing surface internally for its
own modules; external clients embed the SDK, never the daemon crate.

## Client SDK (`crates/theway-sdk`, package `theway`)

The SDK is organized in three layers by execution environment:

- **`common/`** — environment-agnostic surface shared by every mode: session
  archive types, config (+ config readers), the conversation feed model, the
  trigger model types, and the slash-command framework (`Registry`,
  `SlashCommand` trait, `CommandOutcome`, `CommandCtx`).
- **`local/`** — local editing mode (the default): the reference
  `LocalExecutor`, session repo wrappers, auth (+ stream auth), history,
  images, mentions, bug reporting, and the local slash commands
  (quit/clear/help/login/logout/session list).
- **`sandbox/`** — remote sandbox mode (future e2b): the `SandboxExecutor`
  stub implementing the `ToolExecutor` seam. See
  [Future: remote sandbox mode (e2b)](#future-remote-sandbox-mode-e2b).

**Path compatibility**: the SDK keeps the crate name `theway`, so pre-split
client paths resolve unchanged — `theway::session`, `theway::config`,
`theway::session_archive`, `theway::auth`, `theway::history`,
`theway::commands`, etc. The feed keeps its pre-split path through a shim:
`theway::app::feed` re-exports `common::feed`. Clients moved off the old
daemon crate by changing a `Cargo.toml` dependency path, not by editing
`use` statements.

## `ToolExecutor` abstraction (`theway_core::executor`)

Tool effects are decoupled from the agent runtime through the `ToolExecutor`
trait. The trait lives in **`theway-core`** (not the SDK): tool definitions
compile against it directly, keeping the engine self-contained and letting
wasm/embedded consumers provide their own executors. The SDK supplies the
reference `LocalExecutor` and the `SandboxExecutor` stub.

Trait surface (async, object-safe, `Send + Sync`, shareable as
`Arc<dyn ToolExecutor>`):

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

The trait is a *seam*, not an implementation: `theway-core` defines no local
fs/process behavior there. `AgentHarness`, sessions, snapshots, and the
command framework never see the execution environment — only the daemon's tool
assembly does.

## Daemon assembly: binding an executor (`crates/theway-daemon`)

`thewayd` picks the execution environment at assembly time
(`src/bin/thewayd.rs`):

```rust
let executor: Arc<dyn theway_core::executor::ToolExecutor> =
    Arc::new(theway::local::executor::LocalExecutor::default());
```

and hands it to the tool assembly (`src/tools.rs::local_tools(executor)`).

- **Executor-driven tools**: the file-content tools (`read`, `write`, `edit`,
  `outline`) and `git` dispatch their effects through the injected executor —
  swapping the executor swaps their execution environment without touching
  tool definitions. `LocalExecutor` mirrors the previous direct-std behavior:
  UTF-8 reads, parent-creating writes, concurrent stdout/stderr capture with
  kill-on-timeout, `.gitignore`-aware grep/find walks, relative paths resolved
  against the executor's cwd.
- **Local-only for now (first-cut trait)**: `bash` keeps its own
  process-group kill + cancel semantics (the trait's `run_command` kills only
  the direct child), and `ls` / `grep` / `find` / the engine `exec_shell`
  family use richer directory/walk surfaces than the trait's first cut
  exposes. These remain environment-specific daemon tools.
- **Engine tools** (DAG / subagents / skills / memory / MCP adapter) come from
  `theway_core::tools::assembly` and are environment-agnostic.

## Command layering

- **SDK**: `Registry::local()` registers the local command set — everything
  that runs without a daemon runtime: `/help`, `/clear`, `/quit`, `/login`,
  `/logout`, `/sessions`.
- **Daemon**: `Registry::with_daemon_commands()` starts from `local()` and
  appends the runtime commands (goal/model/triggers/skills/cron/…).
  `Registry::with_builtins()` is kept as a compatibility alias for the
  pre-split name.
- **TUI**: completion is built from the SDK `local()` set plus the static
  `DAEMON_COMMANDS` hint table; local commands are handled client-side,
  everything else is forwarded to the daemon.

## Future: remote sandbox mode (e2b)

Tools should run either **locally** (local editing mode, today) or **in a
remote sandbox** (future e2b). The seam is already real:

- `crates/theway-sdk/src/sandbox/` ships `SandboxExecutor`, a
  `ToolExecutor` implementation that reports `ExecutorKind::Sandbox` and
  rejects every operation promptly with `ExecutorError::UnsupportedKind`
  (never hangs). No e2b integration exists yet — the module and trait define
  the seam.
- The daemon's tool assembly dispatches through the same `ToolExecutor` trait
  regardless of environment, so landing e2b means: implement `ToolExecutor`
  against an e2b backend, select the executor at assembly time behind a
  configuration switch, and extend the trait surface where sandbox semantics
  differ.
- Because the harness, sessions, snapshots, feed, and command framework are
  environment-agnostic, a future sandbox mode swaps only the tool executor —
  not the client, not the harness, not the wire model.
