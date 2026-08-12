## 1. Executor 抽象 (theway-core)

- [ ] 1.1 Add `theway-core::executor`: `ToolExecutor` trait (`kind`, read_file,
      write_file, run_command with cwd+timeout, list_dir, grep, find, git),
      `ExecutorKind` enum, `CommandOutput`/result types; `#[async_trait]`
      markers; no behavior change to existing callers
- [ ] 1.2 Unit tests for the trait shape: a fake executor recording calls,
      kind reporting, error propagation (no real fs in core tests)

## 2. SDK crate (crates/theway-sdk, package `theway`)

- [ ] 2.1 Scaffold `crates/theway-sdk` (package+lib `theway`, workspace
      member, lints, dev-deps tests-bridge/tempfile); deps: theway-core,
      theway-storage, theway-transport, theway-llm-provider
- [ ] 2.2 Move `session` module + tests (repo open/resume/create/list/delete,
      sidecar paths) from theway-server → SDK `common/session` (re-export as
      `theway::session`)
- [ ] 2.3 Move `session_archive` + tests → SDK `common/session_archive`
- [ ] 2.4 Move `auth`, `stream_auth` + tests → SDK `common/auth`
- [ ] 2.5 Move `history`, `images`, `mentions`, `bug_report` + tests → SDK
      `common/` (history/images/mentions/bug_report)
- [ ] 2.6 Move `config`, `config_readers` + tests → SDK `common/config`
- [ ] 2.7 Move `app/feed` (+ preview/render/types) + tests → SDK
      `common/feed`; keep `theway::app::feed` re-export path working
- [ ] 2.8 Command framework to SDK: `Registry`, `SlashCommand` trait,
      `CommandOutcome`, `CommandCtx` types + pure helpers (parse_model_spec,
      save_api_key, attach_skill_prompt, model_credential_hint,
      cli_model_help_text, THINKING_LEVEL_VALUES) → SDK `common/commands`;
      `Registry::local()` registers quit/clear/help/login/logout/session-list
      commands (move those impls that need no harness)
- [ ] 2.9 `LocalExecutor` in SDK `local/executor` (std fs + process, cwd +
      timeout semantics matching current tool behavior) + sandbox stub
      `sandbox/executor` (unsupported error) + tests
- [ ] 2.10 SDK lib.rs layering: `common`/`local`/`sandbox` module tree with
      re-exports; SDK compiles standalone + its own tests pass

## 3. Daemon crate 瘦身与改名

- [ ] 3.1 theway-server: delete moved modules, import shared ones from
      `theway` (SDK); daemon-only code (app/*, tools, trigger_engine,
      triggers, skills, templates, mcp_loader, lsp*, goal, control_plane,
      dag_persist, session_ops, system_prompt, oauth/otlp/readline/
      extensions/markdown, daemon slash command impls) stays
- [ ] 3.2 `Registry::with_daemon_commands()`: daemon appends runtime commands
      on top of `Registry::local()`; thewayd assembly switches to it
- [ ] 3.3 Rename `crates/theway-server` → `crates/theway-daemon` (package
      `theway-daemon`, lib `theway_daemon`, bin `thewayd` unchanged);
      workspace members + all path references (Cargo.toml, CI, Makefile,
      docs)
- [ ] 3.4 Daemon tool assembly binds tools to `LocalExecutor` via
      `theway_core::executor` (adapter first: identical std-backed behavior);
      sandbox mode selection is a config seam (unused until e2b)

## 4. TUI 依赖边界

- [ ] 4.1 theway-tui Cargo.toml: dependency `theway-server` → `theway` (SDK
      path); source paths unchanged (crate name `theway` preserved)
- [ ] 4.2 Verify TUI dep graph contains no daemon runtime (cargo tree check:
      no theway-daemon / tools / trigger_engine in the tree)
- [ ] 4.3 TUI completer: build from `Registry::local()` + static daemon
      command table (existing DAEMON_COMMANDS constant); drop any direct
      dependency on daemon command impls

## 5. 验证与收尾

- [ ] 5.1 Full verification: `cargo test --workspace`, `clippy -D warnings`,
      `fmt`; smoke — `thewayd & theway` round-trip, /login + /session
      export/import local surfaces, executor-kind reported as local
- [ ] 5.2 Docs: README crate table + architecture note (SDK common/local/
      sandbox, daemon crate, executor seam, future e2b)
- [ ] 5.3 Close issue #14; ensure `tools-into-core` change is compatible
      (tool definitions in core bind to `theway_core::executor`)
