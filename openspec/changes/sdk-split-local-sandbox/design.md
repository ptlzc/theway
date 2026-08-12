## Context

See proposal.md — Why. Current state (after #13): `crates/theway-server`
(package `theway`, bin `thewayd`) holds both the daemon runtime and the SDK
surface the TUI consumes. The TUI references ~40 `theway::` items, all in the
local-surface set (session/session_archive/auth/history/feed/images/mentions/
bug_report/commands type layer). `theway-core` already hosts the agent
harness, session and (per `tools-into-core`) tool definitions; the daemon
assembles tools under `theway-server/src/tools/` bound to local fs/process.

## Goals / Non-Goals

**Goals:**
- Split the SDK (`common`/`local`/`sandbox`) out of the daemon crate; rename
  the daemon crate to `theway-daemon`.
- Introduce the `ToolExecutor` abstraction in `theway-core` (tools compile
  against it) with `LocalExecutor` + sandbox stub implementations in the SDK.
- Client dependency boundary: `theway-tui` depends only on the SDK; its dep
  graph contains zero daemon runtime code.
- Command registry layering: SDK local commands + daemon additions.

**Non-Goals:**
- No e2b / remote sandbox implementation in this change (stub only).
- No behavior change to `thewayd` or the TUI surfaces (pure refactor of
  structure; `use theway::…` client paths stay valid).
- No `web`/HTTP surface changes; wire model untouched.

## Decisions

1. **Executor trait lives in `theway-core::executor`** (not the SDK): tool
   definitions compile against the trait in core, keeping core self-contained
   and letting wasm/embedded consumers implement their own executors. The SDK
   provides the reference `LocalExecutor` and the `sandbox` stub. Trait
   surface (first cut): `kind()`, `read_file`, `write_file`, `run_command`
   (cwd + timeout), `list_dir`, `grep`, `find`, `git`. Async, `Send + Sync`.
   Tool bodies in core take `&dyn ToolExecutor` (or an `Arc<dyn>`), replacing
   direct std calls.

2. **SDK crate layout** (`crates/theway-sdk`, package + lib `theway`):
   ```
   src/
     common/        executor re-export, session types, config, feed, commands
                    framework (Registry/SlashCommand/CommandOutcome),
                    wire re-exports
     local/         executor/local.rs (LocalExecutor), session repo wrappers,
                    auth, history, images, mentions, bug_report,
                    commands/local.rs (quit/clear/help/login/logout/session)
     sandbox/       executor/sandbox.rs (stub, unsupported error)
   ```
   Module move list (from `theway-server/src`): `session`, `session_archive`,
   `auth`, `stream_auth`, `history`, `images`, `mentions`, `bug_report`,
   `config`, `config_readers`, `app/feed` → `common/feed` (or kept as
   `app::feed` re-export for path compat), `commands` split (types →
   `common/commands`, local impls → `local/commands`, daemon impls stay).

3. **Path compatibility**: the SDK keeps the crate name `theway`, so
   `theway::session`, `theway::app::feed`, `theway::commands` etc. resolve
   unchanged. `app::feed` is re-exported under its old path (`theway::app::feed`)
   so TUI/daemon code needs only Cargo.toml path edits, not source edits.

4. **Command layering**: `Registry::with_builtins()` moves to the SDK as
   `Registry::local()` (local commands only). The daemon adds
   `Registry::with_daemon_commands()` which starts from `local()` and appends
   the runtime commands (goal/model/triggers/skills/cron/…). The TUI builds
   its completer from `local()` + the static daemon-command name table
   (existing `DAEMON_COMMANDS` constant stays as a hint list).

5. **Daemon crate rename**: `crates/theway-server` → `crates/theway-daemon`,
   package `theway-daemon`, lib `theway_daemon` (the bin stays `thewayd`).
   The daemon crate depends on `theway` (SDK), `theway-core`,
   `theway-storage`, `theway-transport`. Its lib keeps the runtime-only
   modules and re-exports nothing the client needs.

6. **Migration order** (keeps the workspace compiling at every step):
   1. Add `theway-core::executor` (trait + types, no behavior change).
   2. Add `crates/theway-sdk`; move the local-surface modules + tests; SDK
      compiles standalone (depends on core/storage/transport).
   3. Point `theway-server` at the SDK for shared modules (delete moved
      code); TUI switches its dependency path to `theway` (SDK).
   4. Rename `theway-server` → `theway-daemon` (Cargo.toml, workspace
      members, CI paths).
   5. Command layering (SDK `local()`, daemon `with_daemon_commands()`).
   6. `LocalExecutor` adoption: daemon tool assembly binds tools to the
      executor; sandbox stub module in the SDK.
   7. Docs (README crate table, architecture note) + full verification.

## Risks / Trade-offs

- **Executor adoption is the invasive part**: moving tool bodies from direct
  std calls to `&dyn ToolExecutor` touches every tool (~15 files). Mitigate:
  land the trait + `LocalExecutor` first with a thin adapter that behaves
  identically (std-backed), then migrate tool bodies mechanically.
- **Command split churn**: `CommandCtx` (fields: harness, trigger executor,
  session, cwd…) is used by daemon commands; the type moves to SDK common but
  is only *constructed* by the daemon — keep it in `common/commands` with the
  daemon-only fields behind the daemon crate's construction site.
- **`app::feed` depends on ratatui**: acceptable (TUI already depends on it;
  the SDK is a client-side crate); alternatively move the pure model
  (`WireFeedBlock`/`Level` are already in transport) and keep the ratatui
  renderer in the TUI. Decision: keep `Feed` in the SDK (it is the shared
  transcript model both TUI and daemon snapshots use).
- **Sandbox stub cost**: near zero (a trait impl returning an error), but the
  seam must be real — the daemon's tool assembly must *actually* be executor-
  driven, not just documented, or the sandbox switch later becomes a rewrite.
