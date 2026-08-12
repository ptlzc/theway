# SDK 拆分 DAG 编排任务清单

> 编排原则见 `openspec/config.yaml`（orchestration 扩展）。节点 ID =
> `<阶段数字>-<kebab-case 语义名>`；每个非根节点必须声明 `[depends: ...]`；
> 并行节点仅限文件不相交（lib.rs/Cargo.toml/共享 module 树不并行）。
>
> 并行组：P1 `1-core-executor ∥ 2-sdk-scaffold` ·
> P2 `3-move-modules ∥ 4-local-executor` · P5 `7-rename-daemon ∥ 8-executor-assembly`
> 其余串行（共享文件约束）。

## Phase 1 — 并行根节点

 - [x] **`1-core-executor`** · agent: executor · [depends: -]
      Add `theway-core::executor`: `ToolExecutor` trait (`kind`,
      read_file, write_file, run_command with cwd+timeout, list_dir, grep,
      find, git), `ExecutorKind`, `CommandOutput`/result types;
      `#[async_trait]` markers; trait-shape unit tests (fake executor
      recording calls, kind reporting, error propagation — no real fs in
      core tests).
      只改: `crates/theway-core/src/executor/*` (+ lib.rs mod 声明)。
      验收: `cargo test -p theway-core` 通过。

 - [x] **`2-sdk-scaffold`** · agent: executor · [depends: -]
      Scaffold `crates/theway-sdk` (package+lib `theway`, workspace member,
      lints, dev-deps tests-bridge/tempfile); deps: theway-core,
      theway-storage, theway-transport, theway-llm-provider. lib.rs 三层骨架:
      `common/mod.rs` / `local/mod.rs` / `sandbox/mod.rs` 空占位模块
      (后续节点只填子文件，不动 lib.rs)。
      只改: `crates/theway-sdk/*`, workspace `Cargo.toml`。
      验收: `cargo check -p theway` (SDK) 通过。

## Phase 2 — 并行: 模块搬迁 ∥ 本地执行器

 - [x] **`3-move-modules`** · agent: executor · [depends: 2-sdk-scaffold]
      Move the local-surface modules + tests from theway-server → SDK
      `common/` (依赖 lib.rs 骨架已由 2 建好): `session`,
      `session_archive`, `auth`, `stream_auth`, `history`, `images`,
      `mentions`, `bug_report`, `config`, `config_readers`, `app/feed`
      (保留 `theway::app::feed` re-export 路径); theway-server 删除已搬
      模块。测试随模块迁移 (session/feed/auth 等)。
      只改: `crates/theway-sdk/src/common/*`, `crates/theway-server/src/*`
      (删减), 两端 lib.rs 模块声明。
      验收: `cargo check -p theway -p theway-tui` 通过 (TUI 路径不变)。

 - [x] **`4-local-executor`** · agent: executor · [depends: 1-core-executor, 2-sdk-scaffold]
      `LocalExecutor` in SDK `local/executor` (std fs + process, cwd +
      timeout semantics matching current tool behavior) + sandbox stub
      `sandbox/executor` (unsupported error, 不挂起) + tests (temp dir
      round-trip, timeout, unsupported kind)。
      只改: `crates/theway-sdk/src/local/executor.rs`,
      `crates/theway-sdk/src/sandbox/executor.rs` (+ 测试文件)。
      验收: SDK executor 测试通过; `cargo check -p theway` 通过。

## Phase 3 — 命令分层

 - [x] **`5-commands-layer`** · agent: executor · [depends: 3-move-modules]
      Command framework to SDK `common/commands`: `Registry`,
      `SlashCommand` trait, `CommandOutcome`, `CommandCtx` types + pure
      helpers (parse_model_spec, save_api_key, attach_skill_prompt,
      model_credential_hint, cli_model_help_text, THINKING_LEVEL_VALUES);
      `Registry::local()` registers quit/clear/help/login/logout/
      session-list commands (impls that need no harness move to SDK
      `local/commands`); daemon command impls (goal/model/triggers/skills/
      cron/…) stay in the daemon crate。
      只改: `crates/theway-sdk/src/common/commands/*`,
      `crates/theway-sdk/src/local/commands/*`,
      `crates/theway-server/src/commands/*` (删减)。
      验收: `cargo check -p theway -p theway-tui` 通过。

## Phase 4 — daemon 瘦身

 - [x] **`6-daemon-slim`** · agent: executor · [depends: 3-move-modules, 5-commands-layer]
      theway-server: 删除已搬模块, 共享代码从 `theway` (SDK) 导入;
      `Registry::with_daemon_commands()` 在 `Registry::local()` 之上追加
      daemon 命令; thewayd 装配切换到它。daemon-only 代码保留:
      app/*, tools, trigger_engine, triggers, skills, templates, mcp_loader,
      lsp*, goal, control_plane, dag_persist, session_ops, system_prompt,
      oauth/otlp/readline/extensions/markdown。
      只改: `crates/theway-server/src/*` (lib.rs/装配/命令注册)。
      验收: `cargo build -p theway` (含 bin thewayd) 通过; 行为不变。

## Phase 5 — 并行: 改名 ∥ executor 装配

 - [x] **`7-rename-daemon`** · agent: executor · [depends: 6-daemon-slim]
      Rename `crates/theway-server` → `crates/theway-daemon` (package
      `theway-daemon`, lib `theway_daemon`, bin `thewayd` unchanged);
      workspace members + 所有 path 引用 (Cargo.toml, CI, Makefile, docs)。
      只改: 目录名, workspace `Cargo.toml`, CI/Makefile/docs 路径。
      验收: `cargo build --workspace` 通过。

 - [x] **`8-executor-assembly`** · agent: executor · [depends: 4-local-executor, 6-daemon-slim]
      Daemon tool 装配绑定 `LocalExecutor` (via `theway_core::executor`);
      先 adapter (std-backed, 行为一致) 再机械迁移 tool bodies 从直接
      std 调用到 `&dyn ToolExecutor`; sandbox 模式选择是配置缝
      (本次不使用)。
      只改: `crates/theway-daemon/src/tools/*`,
      `crates/theway-daemon/src/bin/thewayd.rs` (装配)。
      验收: `cargo test -p theway-daemon` tools 相关通过; 冒烟工具行为一致。

## Phase 6 — TUI 依赖边界

 - [x] **`9-tui-boundary`** · agent: executor · [depends: 7-rename-daemon]
      theway-tui Cargo.toml: dependency `theway-server` → `theway` (SDK
      path); 源码路径不变 (crate 名 theway 保留)。cargo tree 验证:
      TUI 依赖图无 theway-daemon / tools / trigger_engine。TUI completer
      从 `Registry::local()` + 静态 daemon 命令表 (DAEMON_COMMANDS) 构建。
      只改: `crates/theway-tui/Cargo.toml`,
      `crates/theway-tui/src/ui/*` (completer)。
      验收: `cargo tree -p theway-tui` 无 daemon 运行时;
      `cargo test -p theway-tui` 通过。

## Phase 7 — 终态验收

 - [x] **`10-verify`** · agent: verify · [depends: 8-executor-assembly, 9-tui-boundary]
      Full verification (基于最新 HEAD 复核, 不照单全收节点报告):
      `cargo test --workspace`, `clippy --workspace --all-targets -- -D
      warnings`, `cargo fmt --all --check`; 冒烟 — `thewayd & theway`
      round-trip, /login + /session export/import 本地表面, executor kind
      报 local; spec 场景逐条对照 (executor 抽象 / SDK 布局 / 命令分层 /
      客户端依赖边界)。
      验收: 全绿; 失败项记录并修复或上报。

## Phase 8 — 收尾

 - [x] **`11-docs-close`** · agent: writer · [depends: 10-verify]
      Docs: README crate 表 + 架构说明 (SDK common/local/sandbox, daemon
      crate, executor 缝, 未来 e2b); 确认 `tools-into-core` change 兼容
      (工具定义在 core 绑 `theway_core::executor`); close issue #14。
      只改: README.md, docs/*, issue #14。
      验收: 文档与实现一致; issue #14 closed。

---

## 执行注记 (2026-08-12)

- 节点 4 追加 depends 3-move-modules (两者共享 SDK Cargo.toml, 按编排原则串行)。
- 节点 8 追加 depends 7-rename-daemon (其文件清单为 `crates/theway-daemon/...`, 仅改名后存在; tasks.md P5 的并行与文件清单自相矛盾)。
- Cargo 约束: package `theway` (server) 与 SDK 不能同名共存于 workspace → SDK 桥接期用 package `theway-sdk` / lib `theway_sdk`, node 7 原子改回 `theway`。
- node 3 遗留 (config_readers / session_archive 依赖 daemon 类型) 由 orchestrator 直接修复: 抽取 CronJob/DynamicTriggerRule 数据模型入 SDK common/triggers, session_archive + config_readers 完整搬迁 (commit c95f040)。
- node 7 将 bug_report 拆分为 SDK redactor + daemon builder (TUI 需要 `theway::bug_report::redact` 走 SDK 依赖)。
- 11 节点全部完成; 全量验证: cargo test --workspace 全绿, clippy -D warnings 零告警, fmt --all --check 干净, cargo tree -p theway-tui 无 daemon。
