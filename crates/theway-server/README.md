# theway (Rust)

`theway` agent runtime as an **embeddable SDK**: the crate ships both the
`theway` CLI binary (`src/main.rs`) and the full runtime as a library
(`src/lib.rs`), so external projects can depend on `theway` and run the
agent in-process instead of spawning the binary.

Built on top of [`theway-core`](../core) (agent loop / harness) and
[`theway-llm-provider`](../llm-provider) (LLM client). Modeled on the TS
implementation in `packages/coding-agent/` of the upstream `pi` monorepo.

## SDK usage (embedding in an external project)

```toml
[dependencies]
theway = { git = "git@github.com/ptlzc:theway-ai/theway.git" }
# Interactive TUI (optional; the default is headless — no terminal stack):
# theway = { git = "...", features = ["tui"] }
```

Features:

- `tui` — **off by default**. ratatui/crossterm terminal UI + terminal input,
  needed only by the interactive CLI / full-screen App. The SDK defaults to
  headless: `AgentSession`, the `App` state machine, and all transport servers
  (gRPC / HTTP / WebSocket) compile without the terminal stack. `cargo build
  --workspace --features tui` (or `make build`) builds the full CLI.

Three embedding levels (see `examples/sdk_embed.rs` for a complete walkthrough):

1. **Raw runtime** — `theway-core::AgentHarness` (agent loop, tools,
   skills, sessions, hooks, compaction).
2. **Session wrapper** — `theway_ai_harness::agent_session::AgentSession`
   (auto-retry with exponential backoff + optional fallback model) for headless
   prompt loops.
3. **Full surface** — `theway_ai_harness::ui::App` assembled from `AppConfig`:
   TUI, `--web` HTTP/SSE server, `--grpc` server, control-plane approval,
   triggers, relay. The `theway` binary is just the CLI assembly of this.

Transport servers are methods on `App`:

```rust
app.run_web(WebOptions { host, port })   // HTTP + SSE (axum)
app.run_grpc(GrpcOptions { host, port }) // gRPC (tonic, proto/theway_ai_harness_grpc.proto)
```

Other SDK modules: `commands` (slash-command registry), `tools` (all AgentTools),
`triggers` (cron / dynamic / MCP-notification), `skills`, `hooks`, `config`,
`mcp_loader`, `session_archive`, `goal`, `otlp`.

## What's in scope

| | |
|---|---|
| Tools | `read`, `write`, `bash`, `ls`, `memory` (5 total) |
| TUI | Line-based REPL, streaming output, ANSI colors via `crossterm` |
| Sessions | Append-only jsonl under `~/.theway/sessions/<cwd-hash>/` |
| Resume | `--resume` (last session) or `--resume-id <prefix>` |
| Memory | Cross-session, file-backed at `~/.theway/memory/`; auto-loaded into system prompt |
| Models | Auto-detected from `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `OPENROUTER_API_KEY` / Groq / Mistral / Gemini |

## What's deliberately out

Extensions, skills loader, themes, print/json/rpc modes, full-screen TUI widgets (no
ratatui), tool-confirmation prompts, `edit` (with diff), grep/find. All of those exist in the
TS reference and could be added on top of this skeleton.

## Run

```bash
export ANTHROPIC_API_KEY=sk-ant-…           # or any of the supported providers
cargo run

# Resume the most recent session in this cwd
cargo run -- --resume

# Resume a specific session (full UUIDv7 or a unique prefix)
cargo run -- --resume-id 019e

# List sessions for this cwd
cargo run -- --list-sessions

# Delete a session
cargo run -- --delete-session 019e

# Override model
cargo run -- --provider anthropic --model claude-haiku-4-5

# Turn on reasoning for supported models
cargo run -- --thinking high
```

REPL commands inside the loop: `/help`, `/clear`, `/quit` (or `/q`).

## Layout

```
src/
  main.rs          CLI parsing + REPL loop
  config.rs        ~/.theway paths, cwd hashing
  model.rs         env → provider/model detection
  session/mod.rs   create/resume/list/delete via JsonlSessionRepo
  tui.rs           AgentEvent renderer (colors + streaming)
  tools/
    mod.rs         default_tools()
    read.rs        ReadTool (offset/limit, truncation)
    write.rs       WriteTool (parent dir create)
    bash.rs        BashTool (sh -c, timeout, tail-truncate)
    ls.rs          LsTool
    memory.rs      MemoryTool (save/list/read/forget + system-prompt block loader)
    truncate.rs    head/tail truncation primitives
tests/
  tools.rs         end-to-end tool tests against tempdirs
```

## Memory model

When you tell the agent "remember that I prefer X", it can call:

```
memory(action="save", name="prefers-x", description="user preference",
       content="The user prefers X over Y.", type="user")
```

This writes `~/.theway/memory/prefers-x.md` (with YAML frontmatter) and updates the
index at `~/.theway/memory/MEMORY.md`. On every new session, all `*.md` files under
that directory are concatenated into a `<memory>` block in the system prompt — so the agent
sees them without explicit recall.

## Tests

```
cargo test     # 3 unit + 8 integration tests against the tool surface
```
