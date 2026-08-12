## Why

`crates/theway-server` is a leftover name: after #13 (TUI-as-daemon-client) it
is no longer a "server for a UI" — it is the `thewayd` daemon runtime **plus**
a `theway` SDK library the TUI depends on for its local surfaces (session,
auth, history, feed, commands, ~40 references). The directory name misleads,
and the SDK surface is entangled with daemon-only runtime code (tools,
triggers, skills, MCP, LSP) that a pure client should never see.

At the same time the next execution-model step is visible: tools should run
**either locally (local editing mode, today) or in a remote sandbox (future
e2b)**. The SDK should be split by execution environment so the runtime,
snapshots, feed and command framework stay environment-agnostic and a future
sandbox mode only swaps the tool executor — not the client, not the harness.

## What Changes

- **New crate `crates/theway-sdk`** (package `theway`, lib `theway`): the
  shared SDK, organized in three layers by execution environment:
  - `common/` — environment-agnostic: `ToolExecutor` trait, session types,
    config, feed model, command framework, wire re-exports.
  - `local/` — local editing mode: `LocalExecutor` (std fs + process),
    `SqliteSessionRepo`, auth, history, images, mentions, local slash
    commands (quit/clear/help/login/logout/session list).
  - `sandbox/` — remote sandbox mode (future e2b): `SandboxExecutor` trait
    extension + stub (returns unsupported); no real implementation yet.
- **`crates/theway-server` → `crates/theway-daemon`** (package
  `theway-daemon`, lib `theway_daemon`, bin `thewayd` unchanged): keeps the
  daemon-only runtime — `app/{daemon,kernel,listener,relay,session_factory}`,
  tools, trigger engine, skills/templates, MCP loader, LSP, goal/control
  plane, DAG persist, session_ops, system prompt, oauth/otlp/readline/
  extensions/markdown, and the daemon-side slash command implementations.
- **BREAKING (paths)**: `theway::session` / `theway::session_archive` /
  `theway::auth` / `theway::history` / `theway::images` / `theway::mentions` /
  `theway::bug_report` / `theway::config` / `theway::app::feed` /
  `theway::commands` (type layer) resolve from the SDK crate — the crate name
  `theway` is kept, so `use theway::…` paths in clients stay valid.
- **`ToolExecutor` trait lands in `theway-core::executor`** (tools compile
  against the trait in core); the SDK provides `LocalExecutor` and the
  sandbox stub. Daemon assembly injects the executor; `AgentHarness`,
  sessions, snapshots and the command framework never see the environment.
- **Command layering**: the SDK registers local commands only; the daemon
  appends daemon commands (`Registry::with_daemon_commands()`) on top. The
  TUI's static `DAEMON_COMMANDS` completion table retires (or stays as a
  hint-only list).
- **TUI dependency change**: `theway-tui` depends on `theway` (SDK) only —
  the dependency on the daemon crate is removed, making the client truly
  pure (its dep graph no longer contains any runtime code).
- **`sandbox/` is a stub**: no e2b integration in this change; the directory
  and trait define the seam. e2b wiring is a separate future change.

## Capabilities

### New Capabilities
- `sdk/executor`: Tool execution environment abstraction — `ToolExecutor`
  trait (read/write/command/grep/find/ls/git…), `ExecutorKind`, local
  implementation, sandbox stub; tools bind an executor at assembly, the
  runtime and wire model are environment-agnostic.
- `sdk/layout`: SDK crate layout — `common` / `local` / `sandbox` layers,
  crate split (`theway-sdk` + `theway-daemon`), command registry layering
  (SDK local commands + daemon additions), client dependency boundary.

## Impact

- `crates/theway-sdk` (new): `session`, `session_archive`, `auth`,
  `stream_auth`, `history`, `images`, `mentions`, `bug_report`, `config`,
  `config_readers`, `app::feed`, `commands` type layer + local commands,
  `executor::{local, sandbox}`.
- `crates/theway-daemon` (renamed from theway-server): everything daemon-only;
  `thewayd` bin unchanged in behavior; depends on `theway` (SDK) +
  `theway-core` + `theway-storage` + `theway-transport`.
- `crates/theway-tui`: dependency switches from `theway-server` to `theway`
  (SDK); `use theway::…` paths unchanged.
- `theway-core`: new `executor` module (trait + types only).
- Workspace: members `theway-server` → `theway-daemon`, add `theway-sdk`.
- Tests: session/feed/auth/commands tests move with their modules; executor
  gets its own tests (LocalExecutor against a temp dir).
- CI/Makefile/docs: crate path references updated.
