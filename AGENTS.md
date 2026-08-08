# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust 2024 Cargo workspace. The root `Cargo.toml` lists four members:

- `crates/llm-provider`: `theway-llm-provider`, the unified streaming LLM client, provider integrations, OAuth helpers, model catalogs, and utilities.
- `crates/core`: `theway-core`, the agent runtime, harness, session storage, skills loading, compaction, and lifecycle hooks.
- `crates/harness`: `theway`, the `theway` CLI binary, REPL TUI, tools, config, and session handling.

Each crate keeps implementation in `src/`, integration tests in `tests/`, and runnable examples in `examples/` where present. Provider model data and generated Rust live under `crates/llm-provider/src/`; use `crates/llm-provider/scripts/regen_models.sh` when regenerating model catalogs.

## Build, Test, and Development Commands

- `cargo build --workspace`: build all workspace crates.
- `cargo build --release`: produce the optimized `target/release/theway` CLI.
- `cargo test --workspace`: run all crate tests.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint libraries, binaries, tests, and examples with warnings as errors.
- `cargo fmt --all --check`: verify Rust formatting.
- `./target/release/theway --help`: inspect CLI flags after a release build.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting and Rust 2024 idioms. Keep module and file names in `snake_case`; public types and traits in `PascalCase`; functions, variables, and test names in `snake_case`. Prefer crate-local patterns and shared workspace dependencies before adding new dependencies. Keep provider-specific code under `crates/llm-provider/src/providers/` and CLI tools under `crates/harness/src/tools/`.

## Testing Guidelines

Place integration tests in the relevant crate’s `tests/` directory and keep unit tests close to the code they exercise. Tests should avoid real network calls unless explicitly gated; CI clears provider API-key environment variables to catch accidental live calls. Before opening a PR, run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --all --check`.

## Commit & Pull Request Guidelines

The current history only shows an initial commit, so no strict commit convention is established. Use concise imperative subjects, for example `Add session storage tests` or `Fix Anthropic SSE parsing`. Pull requests should include a short summary, test results, linked issues when relevant, and screenshots or terminal output for CLI-visible behavior changes.

## Security & Configuration Tips

Do not commit API keys or local session data. The CLI reads provider keys from environment variables such as `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and related provider-specific keys. Runtime data is written under `~/.theway/` by default, or under `$THEWAY_DIR` when set.

## Subagent Orchestration Ops (DAG / task tool)

Hard-won lessons from multi-repo orchestration runs (theway-public, 2026-08). Applies to
any orchestrator driving the built-in subagents (task tool / dag_*):

1. **Multi-directory tasks must pin the target directory.** Subagents default to the
   orchestrator's cwd. When a task targets a different repo/directory, the task prompt
   must (a) start with an explicit `cd <absolute target>`, (b) forbid touching the
   default cwd, and (c) name the files it may modify. Built-in subagent prompts already
   carry this operating discipline (see `OPERATING_DISCIPLINE` in
   `crates/harness/src/tools/subagent_specs.rs`); the orchestrator still must supply the
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
