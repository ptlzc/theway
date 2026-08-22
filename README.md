# theway

`theway` is a local agent runtime for developer workflows: run it inside a project, inspect and edit files, run shell commands, keep resumable sessions, and use different model providers — including local OpenAI-compatible servers.

It is a terminal-first AI coding agent with slash commands, session history, skills, MCP tools, cron/triggers, and local automation. The client (`theway`) and daemon (`thewayd`) are separate processes: the daemon owns the agent runtime, and the TUI is a pure client that spawns or reuses it.

**Highlight: [Loops — stateful cron jobs + a triage inbox](docs/loops.md).** Give a recurring job a memory across runs and route its findings into an inbox you triage like email.

**Highlight: [theway + DS4 — KV prefix-cache optimizations for local models](docs/ds4.md).** Keep long local-model sessions prefilling only what is new.

## Install / build

```bash
git clone https://github.com/ptlzc/theway.git
cd theway
cargo build --release
```

The CLI binary is at `./target/release/theway`.

## Quick start

Start the daemon in the project directory:

```bash
./target/release/thewayd --cwd /path/to/project
```

Or start the TUI, which spawns or reuses the daemon:

```bash
./target/release/theway
./target/release/theway --provider anthropic --model claude-haiku-4-5
./target/release/theway --thinking high
./target/release/theway --resume
```

A manually started daemon stays running after the TUI exits, so multiple clients can share it. A daemon spawned by the TUI is controller-backed and stops when the TUI exits. See [docs/startup-modes.md](docs/startup-modes.md) for the startup modes. Stop a running daemon with `Ctrl-C` / `SIGTERM`.

The daemon's gRPC surface is split into four domain services — `CommandService`, `SessionService`, `GraphEngineService`, and `EventService` — plus the standard `grpc.health.v1.Health` service. `SessionService` also exposes the daemon path context: `GetPathContext` returns startup-fixed paths and current skill directories, and `SetSkillDirs` hot-reloads the skill catalog. See [docs/architecture.md](docs/architecture.md#daemon-path-context).

## Configure a model

Set an API key before starting, or store it from inside the REPL:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
# or: OPENAI_API_KEY, OPENROUTER_API_KEY, GROQ_API_KEY,
#     MISTRAL_API_KEY, GEMINI_API_KEY, GOOGLE_API_KEY
```

```text
/login anthropic sk-ant-...
```

### Local OpenAI-compatible models

Add a model definition to `~/.theway/models.json` (user-global) or `<project>/.theway/models.json` (project-local, higher precedence), then select it with `--provider` and `--model`.

Example for [DS4](https://github.com/antirez/ds4):

```json
{
  "models": [
    {
      "id": "deepseek-v4-flash",
      "name": "DeepSeek V4 Flash (local DS4)",
      "api": "openai-responses",
      "provider": "ds4",
      "baseUrl": "http://127.0.0.1:8000/v1",
      "reasoning": true,
      "thinkingLevelMap": {
        "off": null,
        "minimal": "low",
        "low": "low",
        "medium": "medium",
        "high": "high",
        "xhigh": "xhigh"
      },
      "input": ["text"],
      "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
      "contextWindow": 100000,
      "maxTokens": 384000,
      "compat": {
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "supportsReasoningEffort": true,
        "supportsUsageInStreaming": true,
        "maxTokensField": "max_tokens",
        "supportsStrictMode": false,
        "thinkingFormat": "deepseek",
        "requiresReasoningContentOnAssistantMessages": true
      }
    }
  ]
}
```

Run with a local placeholder key:

```bash
export DS4_API_KEY=dsv4-local
./target/release/theway --provider ds4 --model deepseek-v4-flash --base-url http://127.0.0.1:8000/v1
```

`--base-url`, `DS4_BASE_URL` (or `DS4_URL`) registers the conventional `ds4` descriptor without a `models.json`. See [docs/ds4.md](docs/ds4.md) for cache-reuse details.

## Slash commands

Inside the REPL, slash commands control the session:

| Command | What it does |
|---------|--------------|
| `/help` | Show all commands |
| `/model [provider:model-id]` | Show or switch model |
| `/thinking` | Show or set thinking level: off, minimal, low, medium, high, xhigh |
| `/sessions` | List sessions for the current project |
| `/save [path]` | Export the transcript to Markdown |
| `/compact [instructions]` | Compact long context |
| `/undo` | Remove the most recent user/assistant turn |
| `/cost` | Show token and cost totals |
| `/login <provider> <api-key>` | Store an API key |
| `/logout <provider>` | Remove a stored API key |
| `/triggers` | Show trigger rules, sources, running actions, and audit |
| `/triggers rules` | List dynamic trigger ids and state |
| `/triggers enable <id>` / `/triggers disable <id>` | Resume or pause a trigger |
| `/triggers remove <id>` | Delete a trigger |
| `/cron` | List local scheduled jobs |
| `/cron add [--stateful] "<minute hour dom month dow>" <prompt>` | Run a prompt on a local schedule; `--stateful` makes it a loop with memory |
| `/cron enable <id>` / `/cron disable <id>` | Resume or pause a scheduled job |
| `/cron remove <id>` | Delete a scheduled job |
| `/inbox [all\|claim <n>\|dismiss <n>\|clear]` | Triage findings reported by stateful loops |
| `/quit` | Exit the TUI (the daemon keeps running) |

Local-only commands handled by the TUI itself: `/login`, `/session export|import`, and `/session switch <id>`. Everything else forwards to the daemon.

## What theway can do

- Read, write, and edit files
- List files and search with grep/find
- Run shell commands
- Manage persistent memory
- Delegate focused sub-tasks
- Resume SQLite-backed sessions per project
- Attach images to the first prompt with `--image`
- Create session-scoped natural-language triggers and cron jobs
- Run stateful loops with a triage inbox; see [docs/loops.md](docs/loops.md)
- Receive server-pushed MCP notifications and normalize them into the trigger runtime
- Run local command hooks or HTTP webhooks on lifecycle events; see [docs/hooks.md](docs/hooks.md)
- Render assistant output through the Markdown/Grok Build ports

## Automation

### Triggers

Triggers turn natural language into dynamic rules:

```text
when $HOME/helloworld exists, print its contents
```

Dynamic triggers fire once by default; ask for a repeating trigger when needed. Trigger actions run in a separate sub-agent that inherits the parent model, tools, and skill catalog. Local dynamic checks poll every 10 minutes by default; configure the interval in `~/.theway/config.toml`:

```toml
[triggers]
poll_interval_secs = 600
```

MCP server-push notifications are trigger sources too. Raw notification params are not persisted as chat content or trigger audit; unknown/custom notifications keep only bounded summaries unless the server provides `_meta.theway_summary`.

### Cron and loops

Cron jobs are time-based automations stored next to the active session:

```text
/cron add "*/30 * * * *" summarize the repo state
/cron list
/cron disable cron-...
```

Add `--stateful` to turn a job into a loop that keeps notes between runs and reports findings to the inbox:

```text
/cron add --stateful "0 9 * * *" check the repo issues and report anything new since the last run
/inbox
/inbox claim 1
```

See [docs/loops.md](docs/loops.md) for the full guide.

## Files and storage

Local state lives under `~/.theway` by default; set `THEWAY_DIR` to change it.

| Path | What |
|------|------|
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.db` | Session history — one SQLite database per session |
| `~/.theway/memory/*.md` | Cross-session memory |
| `~/.theway/auth.json` | Stored API keys from `/login` |
| `~/.theway/models.json` | User-global local/custom model definitions |
| `~/.theway/history` | Prompt history |
| `~/.theway/mcp.toml` | MCP server config |
| `~/.theway/hooks.toml` | Optional command/webhook hooks |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.triggers.json` | Session-scoped trigger rules |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.cron.toml` | Session-scoped cron jobs |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.loop-<job-id>.md` | Loop state |
| `~/.theway/inbox.jsonl` | Global triage inbox |
| `~/.theway/daemon-port-<cwd-hash>` | Port + pid for the running daemon |
| `~/.theway/config.toml` | Optional user config |

The daemon resolves its host paths once at startup (`--cwd`, `--home`, repeatable `--skills-dir`). The only runtime-mutable part is the extra skill dirs via `SetSkillDirs`. See [docs/architecture.md](docs/architecture.md#daemon-path-context).

## Workspace layout

| Crate | Package | What |
|-------|---------|------|
| [`crates/theway-core`](crates/theway-core/README.md) | `theway-core` | Daemon runtime core: agent loop and harness, runtime ports, typed sessions, and multiagent DAG engine. |
| [`crates/theway-daemon`](crates/theway-daemon/README.md) | `theway-daemon` | `thewayd` composition root: runtime assembly, tools, automation, orchestration, integrations, observability, and protocol servers. |
| [`crates/theway-transport`](crates/theway-transport/README.md) | `theway-transport` | Cross-client wire model plus gRPC, HTTP/JSON-RPC, SSE, and WebSocket carriers. |
| [`crates/theway-tui`](crates/theway-tui/README.md) | `theway-tui` | `theway` terminal client/controller and offline session-maintenance commands. |
| [`crates/theway-contract`](crates/theway-contract/README.md) | `theway-contract` | Leaf persistence and path contracts with no workspace dependencies. |
| [`crates/theway-storage`](crates/theway-storage/README.md) | `theway-storage` | SQLite session and DAG persistence plus session archives. |
| [`crates/theway-llm-provider`](crates/theway-llm-provider/README.md) | `theway-llm-provider` | Normalized streaming LLM client, provider integrations, and model catalogs. |
| [`crates/theway-mcp`](crates/theway-mcp/README.md) | `theway-mcp` | MCP stdio client and JSON-RPC framing. |
| [`crates/theway-probe`](crates/theway-probe/README.md) | `theway-probe` | gRPC serviceability probe. |
| [`crates/theway-markdown-core`](crates/theway-markdown-core/README.md) | `theway-markdown-core` | Headless Markdown parser policy, analysis, and diagnostics. |
| [`crates/theway-markdown`](crates/theway-markdown/README.md) | `theway-markdown` | Streaming terminal Markdown renderer. |
| [`crates/theway-pager-render`](crates/theway-pager-render/README.md) | `theway-pager-render` | Ratatui pager and feed rendering primitives. |
| [`crates/theway-ratatui-textarea`](crates/theway-ratatui-textarea/README.md) | `theway-ratatui-textarea` | Grapheme-aware multiline editor and ratatui widget. |
| [`crates/mermaid-parser`](crates/mermaid-parser/README.md) | `mermaid-rs-parser` | Vendored Mermaid source-to-IR parse stage. |
| [`crates/tests-bridge-macro`](crates/tests-bridge-macro/README.md) | `tests-bridge-macro` | Proc macro for crate-root-anchored mirrored unit-test suites. |

See [docs/architecture.md](docs/architecture.md) for the full three-layer layout, tool policy matrix, and storage contract.

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Testing standards live in [docs/testing.md](docs/testing.md) and [docs/rust-test-files.md](docs/rust-test-files.md). Extension authoring is documented in [docs/extensions.md](docs/extensions.md); DAG orchestration in [docs/graph-engineering-mode.md](docs/graph-engineering-mode.md).

## License

[MIT](LICENSE)
