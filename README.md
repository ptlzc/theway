# theway

`theway` is a terminal-first AI coding agent for local developer workflows. It reads and edits files, runs shell commands, keeps resumable SQLite-backed sessions, and works with multiple model providers, including local OpenAI-compatible servers.

The client (`theway`) and the daemon (`thewayd`) are separate processes. The daemon owns the agent runtime, tools, sessions, skills, and automation; the TUI is a pure client that spawns or reuses a daemon.

## Install and use

### Run from source

```bash
git clone https://github.com/ptlzc/theway.git
cd theway
cargo build --release
```

The CLI binary is at `./target/release/theway`.

### Install locally

```bash
./scripts/install.sh
```

This installs `theway`, `thewayd`, and the `tw` shorthand into `~/.cargo/bin`.

### Start

```bash
theway                  # open the TUI in the current project
theway --resume         # pick a previous session
theway --thinking high
theway --provider anthropic --model claude-haiku-4-5
```

On first use, `/model` opens the model picker. `/model <provider:model-id>` switches directly and saves the selection as the next startup default. Set an API key with `export <PROVIDER>_API_KEY=...` or `/login <provider> <key>`.

Inside the REPL, `/help` lists every slash command. The most-used ones are `/model`, `/thinking`, `/sessions`, `/compact`, `/triggers`, `/cron`, and `/quit`. See [docs/startup-modes.md](docs/startup-modes.md) for daemon lifecycle and spawn modes.

## What it can do

- Read, write, edit, and search files; run shell commands
- Keep per-project resumable sessions and cross-session memory
- Attach images to the first prompt with `--image`
- Load skills and provision MCP servers from `~/.theway/config.toml`
- Delegate work to subagents and orchestrate DAGs
- Create session-scoped triggers and cron jobs, including stateful loops with a triage inbox
- Run command hooks and HTTP webhooks on lifecycle events
- Work with OpenAI-compatible local model servers; see [docs/ds4.md](docs/ds4.md)

## Documentation

- [GitHub Wiki](https://github.com/ptlzc/theway/wiki) — tutorials, FAQ, recipes, and runbooks
- [Architecture](docs/architecture.md) — crate layout, tool policy, daemon path context, storage
- [Startup modes](docs/startup-modes.md) — daemon spawn/reuse and controller lifecycle
- [Loops](docs/loops.md) — stateful cron jobs and the triage inbox
- [DS4](docs/ds4.md) — KV prefix-cache optimizations for local models
- [Hooks](docs/hooks.md) — local command hooks and HTTP webhooks
- [Extensions](docs/extensions.md) — runtime extension authoring
- [Graph engineering mode](docs/graph-engineering-mode.md) — subagent and DAG orchestration
- [Testing](docs/testing.md) and [Rust test files](docs/rust-test-files.md) — test layout and standards

## Community and support

- Report bugs or request features through [GitHub Issues](https://github.com/ptlzc/theway/issues).
- Discuss ideas in [GitHub Discussions](https://github.com/ptlzc/theway/discussions).

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The [`Makefile`](Makefile) wraps the same CI pipeline; `make ci` runs it locally.

## License

[MIT](LICENSE)
