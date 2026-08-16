# theway

`theway` is an internal fork of [pie](https://github.com/earendil-works/pie) (a Rust rewrite of the original [pi](https://github.com/earendil-works/pi) project), maintained by theway as the local agent runtime for WorkMate (workmate-local). It is a terminal-based AI coding agent runtime: run it inside a project, ask it to inspect files, make edits,
run shell commands, remember preferences, and continue previous sessions.

The initial reason was that I had some proactive, long-term automated tasks to run on a local DS v4 model. Therefore, I needed a customizable agent runtime to support these custom tasks, such as triggers, to perform some simple automation, Over time, the project gradually became more and more usable, so I thought I might as well turn it into a proper project. Of course, most of the code in this project was written by AI. If you’re sensitive to AI-generated code or AI coding, feel free to simply ignore it.

Theway runs inside your local project directory, can inspect/edit files, run shell commands, keep resumable sessions, and use different model providers, including local OpenAI-compatible servers.

The goal is not just to build another chat UI for coding, but a local agent runtime for developer workflows: slash commands, session history, skills, MCP tools, cron/triggers, and local automation.

**Highlight: [Loops — stateful cron jobs + a triage inbox](docs/loops.md).** Give a
recurring job a memory across runs and route its findings into an inbox you triage like
email — agent loops that prompt *you*, instead of the other way around.

**Highlight: [theway + DS4 — KV prefix-cache optimizations for local models](docs/ds4.md).**
theway keeps its request stream byte-exact for DS4's prefix cache (reasoning replay,
transparent 409 recovery, honest cache accounting), so long local-model sessions prefill
only what's new instead of the whole conversation every turn.

## Install / build

```bash
git clone https://github.com/ptlzc/theway.git
cd theway
cargo build --release
```

The CLI binary will be at `./target/release/theway`.

## Configure a model

`theway` auto-detects the first available provider credential. Set an API key before starting:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
# or: OPENAI_API_KEY, OPENROUTER_API_KEY, GROQ_API_KEY,
#     MISTRAL_API_KEY, GEMINI_API_KEY, GOOGLE_API_KEY
```

You can also store a key from inside `theway`:

```text
/login anthropic sk-ant-...
```

### Local OpenAI-compatible models

`theway` can also use local OpenAI-compatible servers. Add a model definition to
`~/.theway/models.json` (user-global) or `<project>/.theway/models.json` (project-local, higher
precedence), then select it with `--provider` and `--model`.

Example for [DS4](https://github.com/antirez/ds4), the DeepSeek V4 Flash local
server. The Responses endpoint is the preferred OpenAI-compatible API for
Codex-style clients; chat completions also works for simpler integrations.

```bash
# In the DS4 checkout:
./ds4-server --ctx 100000 --kv-disk-dir /tmp/ds4-kv --kv-disk-space-mb 8192
# If launching from another directory, add: --chdir /path/to/ds4
```

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

Then run:

```bash
export DS4_API_KEY=dsv4-local
./target/release/theway --provider ds4 --model deepseek-v4-flash --base-url http://127.0.0.1:8000/v1
```

DS4 is local and accepts placeholder bearer tokens. You can also store the same
local placeholder with `/login ds4 dsv4-local`. Using the `ds4` provider keeps
local model credentials separate from real `OPENAI_API_KEY` credentials.

`--base-url`, `DS4_BASE_URL` (or `DS4_URL`) registers the conventional `ds4` /
`deepseek-v4-flash` descriptor without a `models.json` file. CLI `--base-url`
wins for the current run. Keep `models.json` when you need different limits,
compatibility flags, or a project-local override.

theway's client stream is tuned for DS4's byte-exact KV prefix cache — see
[docs/ds4.md](docs/ds4.md) for the cache-reuse optimizations and how to verify them
with `/cost`.

## Quick start

`theway` is a **pure client**: it connects to the `thewayd` daemon, which owns the
agent runtime (harness, session, tools, triggers). The client/daemon split mirrors
the crate layout — the TUI links `theway-transport` / `theway-core` /
`theway-storage`, never the daemon kernel; see [Workspace layout](#workspace-layout)
and [docs/architecture.md](docs/architecture.md). On startup the TUI reuses a
running daemon (discovered via `~/.theway/daemon-port-<cwd-hash>` or the default
port `44777`), or spawns `thewayd` in the current directory and waits for
readiness.
The daemon stays running after the TUI exits (multi-client sharing); stop it
with `Ctrl-C` / `SIGTERM`, or start it explicitly:

```bash
# Start the daemon yourself (same flags as the TUI: --provider/--model/--cwd/...)
./target/release/thewayd --cwd /path/to/project
./target/release/thewayd --port 0        # random port, published to the per-cwd ~/.theway/daemon-port-<cwd-hash>
./target/release/thewayd --http          # HTTP/WS surface (workmate) instead of gRPC
```

The daemon's gRPC surface is split into four domain services —
`theway.grpc.v1.CommandService`, `theway.grpc.v1.SessionService`,
`theway.grpc.v1.GraphEngineService`, and `theway.grpc.v1.EventService` — plus
the standard `grpc.health.v1.Health` service.

```bash
# Start the TUI in the current project (spawns/reuses the daemon)
./target/release/theway

# Choose a specific provider/model (forwarded to the daemon on spawn)
./target/release/theway --provider anthropic --model claude-haiku-4-5

# Enable extended thinking where supported
./target/release/theway --thinking high

# Resume the most recent session for this project
./target/release/theway --resume
```

> **Tool working directory**: tools execute in the **daemon's** cwd (start the
daemon in the project directory, or pass `--cwd`). The TUI no longer runs
commands in its own process.

> **Disconnects**: if the daemon dies, the TUI shows a `daemon offline` banner
and reconnects automatically when it comes back.

Once the REPL opens, type a request such as:

```text
summarize this repository
fix the failing tests
add a README example and run the relevant checks
when ~/build.done appears, run cargo test and show me the result
```

## Useful commands

Inside `theway`, slash commands control the session:

| Command | What it does |
|---------|--------------|
| `/help` | Show all commands |
| `/model [provider:model-id]` | Show or switch model |
| `/thinking` | Show or set thinking level, off, minimal, low, medium, high, xhigh |
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
| `/cron add [--stateful] "<minute hour dom month dow>" <prompt>` | Run a prompt on a local schedule; `--stateful` makes it a loop with memory, see [docs/loops.md](docs/loops.md) |
| `/cron enable <id>` / `/cron disable <id>` | Resume or pause a scheduled job |
| `/cron remove <id>` | Delete a scheduled job |
| `/inbox [all\|claim <n>\|dismiss <n>\|clear]` | Triage findings reported by stateful loops |
| `/quit` | Exit the TUI (the daemon keeps running) |

Local-only commands (the TUI handles these itself; everything else forwards to
the daemon): `/login` (writes `~/.theway/auth.json` — the daemon picks the key
up on its next turn), `/session export|import` (local SQLite repo),
`/session switch <id>` (daemon `SwitchSession` RPC).

CLI helpers:

```bash
./target/release/theway --help
./target/release/theway --list-sessions
./target/release/theway --list-all-sessions
./target/release/theway --delete-session <id>
./target/release/theway --image screenshot.png
```

## What theway can do

The agent has tools for common coding workflows:

- read, write, and edit files
- list files and search with grep/find
- run shell commands
- manage persistent memory
- delegate focused sub-tasks
- resume SQLite-backed sessions per project
- attach images to the first prompt with `--image`
- create session-scoped natural-language triggers that run actions when local checks or MCP
  push events match
- create session-scoped cron jobs that run prompts on a local schedule
- run stateful loops: recurring jobs that keep notes between runs and report findings to a
  triage inbox; see [docs/loops.md](docs/loops.md)
- receive server-pushed MCP notifications and normalize them into the same trigger runtime
- run local command hooks or HTTP webhooks on agent lifecycle events; see [docs/hooks.md](docs/hooks.md)
- render assistant output through crates/theway-markdown (ported from Grok Build: bold, italic, headings, inline code, lists, tables, blockquotes, strikethrough, link underlines, fenced syntax highlighting, mermaid, LaTeX pretty mode)

## Triggers and notifications

Triggers let you describe an automation in normal chat:

```text
when $HOME/helloworld exists, print its contents
```

`theway` turns that request into a dynamic trigger rule. Rules are stored next to the active
session, so a new session starts cleanly and `--resume` brings that session's rules back.
Dynamic triggers fire once by default; ask for a repeating trigger when you want it to keep
running.

Trigger actions run in a separate sub-agent. The sub-agent inherits the parent model, tools,
tool hooks, thinking level, and skill catalog, but it does not receive the full parent
conversation by default. Trigger output is shown in the TUI and written to trigger audit
records. If you explicitly ask for the result to be visible to future turns, the rule is
created with `promote_to_chat=true` and successful trigger output is inserted into the main
chat context with a `[Trigger ...]` prefix.

Local dynamic checks poll every 10 minutes by default, and only emit checks while at least
one enabled dynamic rule exists. Configure the interval in `~/.theway/config.toml`:

```toml
[triggers]
poll_interval_secs = 600
```

For one run, override it with:

```bash
./target/release/theway --trigger-poll-secs 60
```

The TUI conversation feed keeps a 3000-line scrollback by default. Raise (or lower)
the cap in `~/.theway/config.toml`:

```toml
[tui]
max_feed_lines = 10000
```

Notifications are trigger sources too. Each configured MCP server may expose a server-push
stream; `theway` consumes those frames through a `NotificationHook`, converts them into bounded
trigger envelopes, and runs them through the same deduping, audit, prompt, and action queue as
dynamic trigger checks. Built-in MCP notifications such as tool/resource/prompt list changes use
stable replacement keys, so repeated updates collapse to the latest event. Custom
`notifications/*` events must include `_meta.theway_dedup_key` or `_theway_dedup_key`; otherwise they
are dropped at the adapter and counted in hook status.

The notification privacy boundary is intentionally conservative. Raw MCP notification params are
not persisted as chat content or trigger audit. Unknown/custom notifications persist only a
bounded method-style summary unless the server provides `_meta.theway_summary`, which is capped and
redacted before display or audit. This notification runtime is used by ordinary `mcp.toml`
servers and cron hooks.

The experimental public cross-agent messaging service has been removed from the shipped client
surface. Configure ordinary MCP servers explicitly in `~/.theway/mcp.toml` when you need external
tools or notification sources.

## Cron jobs

Cron jobs are time-based automations, separate from dynamic triggers. By default they are
stored next to the active session transcript, so a new session starts cleanly and `--resume`
brings that session's scheduled jobs back. Cron jobs use local time and support standard
5-field cron expressions:

```text
/cron add "*/30 * * * *" summarize the repo state
/cron list
/cron disable cron-...
```

When a cron job is due, it enters the same serialized agent turn queue used by prompts and
trigger inject-and-run actions. `theway` does not backfill missed ticks after downtime. If a
job is still running when its next tick arrives, that tick is skipped and recorded in the
job status. Cron config stores only the schedule and action text; control-plane audit and
UI output use bounded, redacted previews.

`/cron add` and natural-language scheduled jobs are session-scoped. They do not write a
user-global `~/.theway/cron.toml`; a global cron install must be an explicit separate user
action rather than the default behavior.

### Loops: stateful jobs + inbox

Add `--stateful` to turn a cron job into a loop: the job runs in a background sub-agent,
keeps its own notes between runs (so "what changed since last time?" works), and reports
findings to a triage inbox instead of interrupting your chat:

```text
/cron add --stateful "0 9 * * *" check the repo issues and report anything new since the last run
/inbox            # triage findings
/inbox claim 1    # promote a finding into a real agent turn
```

See [docs/loops.md](docs/loops.md) for the full guide and design rationale.

## Files and storage

By default, `theway` stores local state under `~/.theway`:

| Path | What |
|------|------|
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.db` | Session history for each project — one SQLite database per session |
| `~/.theway/memory/*.md` | Cross-session memory injected into future sessions |
| `~/.theway/auth.json` | Stored API keys from `/login` |
| `~/.theway/models.json` | User-global local/custom model definitions |
| `~/.theway/history` | Prompt history |
| `~/.theway/mcp.toml` | User-global MCP server config; project config may live at `<repo>/.theway/mcp.toml` |
| `~/.theway/hooks.toml` | Optional command/webhook hooks |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.triggers.json` | Session-scoped dynamic trigger rules (same stem as the session `.db`) |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.cron.toml` | Session-scoped cron jobs (same stem as the session `.db`) |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.loop-<job-id>.md` | Loop state kept by a stateful cron job (same stem as the session `.db`) |
| `~/.theway/sessions/<cwd-hash>/<uuidv7>.endpoints.json` | Session-scoped endpoint bindings (same stem as the session `.db`) |
| `~/.theway/inbox.jsonl` | Global triage inbox written by stateful loops |
| `~/.theway/daemon-port-<cwd-hash>` | Port + pid the running `thewayd` bound for that cwd (written on bind; clients discover the daemon here) |
| `~/.theway/config.toml` | Optional user config, including trigger poll interval and TUI scrollback (`[tui] max_feed_lines`) |

Set `THEWAY_DIR` to use a different base directory.

## Workspace layout

| Crate | Package | What |
|-------|---------|------|
| `crates/theway-core` | `theway-core` | Core interface + agent runtime: bare `Agent` + loop, `AgentHarness` (skills, sessions, compaction, permission policy), session storage contracts, multiagent DAG engine machinery, and the `ToolExecutor` trait (`theway_core::executor`). |
| `crates/theway-daemon` | `theway-daemon` | The single kernel (bin `thewayd`): harness assembly, all tool bodies, the local/sandbox executor policy (fail-closed in sandbox-only builds), trigger/cron runtime, session lifecycle, DAG persistence, skills/templates, MCP/LSP wiring, and the transport servers. |
| `crates/theway-transport` | `theway-transport` | Protocol layer: wire model + gRPC / HTTP / WebSocket / MCP transports, plus the shared client-contract modules (auth, commands, config, feed, history, images, mentions, triggers). |
| `crates/theway-tui` | `theway-tui` | Terminal UI (the `theway` CLI binary) — a pure client of the daemon; links `theway-transport` / `theway-core` / `theway-storage`, so its dependency graph contains no daemon kernel code. Offline exception: `session export/import` and the standalone session queries open the local SQLite repo directly. |
| `crates/theway-contract` | `theway-contract` | Shared contract leaf crate: session-scoped trigger/cron sidecar models and the `~/.theway` base-dir / cwd-hash path layout. No engine, no protocol, no runtime. |
| `crates/theway-storage` | `theway-storage` | Session storage: one SQLite database per session (`<uuidv7>.db`), session archive export/import, DAG run persistence. Depends on `theway-core` + `theway-contract` only — never on the transport stack. |
| `crates/theway-llm-provider` | `theway-llm-provider` | Unified streaming LLM client and provider integrations. |
| `crates/theway-mcp` | `theway-mcp` | Minimal MCP client (stdio transport, JSON-RPC framing). |
| `crates/mermaid-parser` | `mermaid-rs-parser` | Vendored mermaid parser for DAG specs. |

The TUI renders assistant output through the Grok Build ports
(`theway-markdown`, `theway-pager-render`, `theway-ratatui-textarea`).

Tool effects flow through the `ToolExecutor` trait. The daemon kernel is the
single execution authority: in `local` builds (default) the file-content and
git tools dispatch through the local executor, while the direct-OS tools
(`bash`, the `exec` shell family, `ls`, `grep`, `find`) run against the host;
in `sandbox`-only builds those direct-OS tools are dropped from the tool set
(fail closed, with an explicit warning), and the executor-backed tools answer
through the sandbox seam. See
[docs/architecture.md](docs/architecture.md) for the full three-layer layout,
the tool policy matrix, and the storage contract.

**Versioning**: the runtime crates (`theway-core`, `theway-daemon`,
`theway-tui`, `theway-transport`, `theway-storage`, `theway-contract`,
`theway-mcp`, `theway-llm-provider`, `theway-probe`, `tests-bridge-macro`)
release together at the workspace version. The vendored/port crates keep
independent versions and licenses: `mermaid-rs-parser` (vendored upstream,
MIT) and the Grok Build ports `theway-markdown`, `theway-markdown-core`,
`theway-pager-render`, `theway-ratatui-textarea` (Apache-2.0).

## Development

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

## License

[MIT](LICENSE)
