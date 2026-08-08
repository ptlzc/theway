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
