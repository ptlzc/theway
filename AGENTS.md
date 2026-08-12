# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust 2024 Cargo workspace. The root `Cargo.toml` lists its members; the key ones:

- `crates/theway-llm-provider`: `theway-llm-provider`, the unified streaming LLM client, provider integrations, OAuth helpers, model catalogs, and utilities.
- `crates/theway-core`: `theway-core`, the agent runtime, harness, session storage, skills loading, compaction, and lifecycle hooks.
- `crates/theway-sdk`: `theway`, the client SDK library — session/config/auth/history types, the slash-command framework, the `LocalExecutor`, and the sandbox stub. The TUI and external embedders depend on it.
- `crates/theway-daemon`: `theway-daemon`, the headless daemon runtime (bin `thewayd`, server-first) — harness assembly, tools, triggers, skills, MCP/LSP wiring, consuming the `theway` SDK for the client-facing surface.

Each crate keeps implementation in `src/`, integration tests in `tests/`, and runnable examples in `examples/` where present. Provider model data and generated Rust live under `crates/theway-llm-provider/src/`; use `crates/theway-llm-provider/scripts/regen_models.sh` when regenerating model catalogs.

## File Size Governance (>800 lines)

Source and test files must stay under ~800 lines; larger files must be split into a **directory** (`foo.rs` → `foo/mod.rs` + domain submodules for src; `tests/<name>/mod.rs` + domain submodule files for tests), splitting by domain/module, never mechanically. Exceptions (third-party vendored/extracted code that must stay monolithic):

- `crates/mermaid-parser/src/parser.rs` — extracted from the third-party `mmdr` parser (vendored mermaid parse stage); kept as one file to stay diff-compatible with upstream extraction. Do NOT split it; do not re-apply the 800-line rule to it.
- `crates/theway-core/src/agent/assembly.rs` — the `AgentHarness` composer (Agent + Session + skills + compaction + permission + lifecycle events), flattened from `agent/assembly/` by owner decision; kept monolithic so the composed agent API reads as one unit. Do NOT split it; do not re-apply the 800-line rule to it.

## Build, Test, and Development Commands

- `cargo build --workspace`: build all workspace crates.
- `cargo build --release`: produce the optimized `target/release/theway` CLI.
- `cargo test --workspace`: run all crate tests.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint libraries, binaries, tests, and examples with warnings as errors.
- `cargo fmt --all --check`: verify Rust formatting.
- `./target/release/theway --help`: inspect CLI flags after a release build.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting and Rust 2024 idioms. Keep module and file names in `snake_case`; public types and traits in `PascalCase`; functions, variables, and test names in `snake_case`. Prefer crate-local patterns and shared workspace dependencies before adding new dependencies. Keep provider-specific code under `crates/theway-llm-provider/src/providers/` and daemon tool implementations under `crates/theway-daemon/src/tools/`.

## Testing Guidelines

Test file layout, naming, and structure follow [`docs/RUST_TEST_FILES.md`](docs/RUST_TEST_FILES.md) — the single source of truth (adapted from .NET conventions: src/test separation, 1:1 mirroring, `被测方法_场景_预期` naming, AAA). Key rules:

- **No `tests/` directories under `src/`**: multi-file module suites live in `crates/<name>/tests/<mirrored-src-path>/`, bridged from the src module by a single `#[path]` line (unit-test semantics preserved).
- **Tests stay in `tests/`** (crate-level); inline `mod tests { }` is allowed only for lightweight unit assertions.
- Tests should avoid real network calls unless explicitly gated; CI clears provider API-key environment variables to catch accidental live calls.
- Before opening a PR, run `cargo test --workspace --no-fail-fast`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --all --check`.

## Commit & Pull Request Guidelines

The current history only shows an initial commit, so no strict commit convention is established. Use concise imperative subjects, for example `Add session storage tests` or `Fix Anthropic SSE parsing`. Pull requests should include a short summary, test results, linked issues when relevant, and screenshots or terminal output for CLI-visible behavior changes.

## Git 协作 (本项目 — worktree 工作模式)

> 通用 Git 纪律 (commit 规范 / `git log` 事实源 / 冲突排查协议 / 大任务逐步提交 / worktree 小步 merge) 见全局 `~/.pi/agent/AGENTS.md`。以下为本项目硬约束,优先级高于上游规则:

- **Issue-first**: 本仓库是 GitHub (`github.com:ptlzc/theway`),已认证 `gh` 时接到任务先 `gh issue create` 记录需求,拿到 issue 编号再实现;commit message 引用 issue (如 `feat(#12): ...`),完成时 `gh issue close`。无 issue 不开始实现。
- **直推 main**: 提交后立即 `git push origin main`;commit message 用 Conventional Commits 并引用 issue 线。
- **禁止动他人改动**: 其他 agent 的未提交变更保留原样,禁止修改 / 丢弃 / 还原 (`git checkout --` / `git restore` / `git reset --hard` 一律禁止,详见全局 AGENTS.md 冲突排查协议)。
- **大功能按归属分笔提交** (core / server / 文档各一笔),不 `git add -A` 混入 `.pi/` 状态文件与 `.agents/` 记忆 (已 gitignore,stage 仍须精确到文件)。
- **worktree 工作模式**: 每个并行子任务一个独立 worktree,用 [`scripts/wt.sh`](scripts/wt.sh) 管理 — `wt start <标题>` 一条命令建 issue + worktree + 分支 (`wt wt <id>` 为已有 issue 建),`wt push` / `wt mr` / `wt merge --rm-wt` / `wt cleanup` 走完推送→PR→合并→清理闭环;每完成一小步即同步到主分支并推送;worktree 用完即删 (`git worktree remove`),不留垃圾分支。
- **Merge 前置检查**: 任何 `git checkout` / `git merge` 之前先 `git status --porcelain` 确认目标分支干净;有未提交改动先 `git add` (stage) / `git commit` / `git stash push` 固化,禁止覆盖 (详见全局 AGENTS.md)。

## Security & Configuration Tips

Do not commit API keys or local session data. The CLI reads provider keys from environment variables such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and related provider-specific keys. Runtime data is written under `~/.theway/` by default, or under `$THEWAY_DIR` when set.

## Subagent Orchestration Ops (DAG / subagent tool)

Hard-won lessons from multi-repo orchestration runs (theway-public, 2026-08). Applies to
any orchestrator driving the built-in subagents (subagent tool / dag_*):

1. **Multi-directory tasks must pin the target directory.** Subagents default to the
   orchestrator's cwd. When a task targets a different repo/directory, the task prompt
   must (a) start with an explicit `cd <absolute target>`, (b) forbid touching the
   default cwd, and (c) name the files it may modify. Built-in subagent prompts already
   carry this operating discipline (see `OPERATING_DISCIPLINE` in
   `crates/theway-daemon/src/tools/subagent_specs.rs`); the orchestrator still must supply the
   concrete target path — subagents cannot guess it.
2. **Stalled nodes are the orchestrator's to handle.** A node whose token/round counter
   stops growing across two inspection cycles is stalled. Don't wait on it: skip the node
   (its downstream treats it as done), take over its remaining work yourself, and fix
   whatever it produced that is wrong. Subagents sometimes leave a build half-fixed or a
   commit pushed early — verify, don't trust.
3. **Only the final publish node may write version control.** Subagents must not
   `git add/commit/push` unless a task explicitly authorizes them. The orchestrator owns
   commits; a `publish`-style node at the end of the DAG is the single allowed writer.
4. **DAG node task text is the contract.** `dependsOn` must be declared on every
   non-root node (a missing `dependsOn` runs everything in parallel). Task text must
   include absolute paths, forbidden operations, and the acceptance check — the node
   label is not the task.
