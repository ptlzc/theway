# Built-in capability tools: web fetch, web search, git, compile-error feedback loop

> Parent: master roadmap issue.
> Tier: 4 (framework depth).

## Goal

`theway` ships with `read/write/edit/bash/ls/grep/find/memory`. Mature coding agents add:

- `web_fetch` — fetch a URL, return rendered text (or Markdown for HTML).
- `web_search` — search the web (configurable backend: Brave, Tavily, Bing, DuckDuckGo).
- `git` — wrapped, structured `status / diff / log / branch / blame` (rather than `bash git
  status` which is messy).
- **LSP feedback loop** — after an edit, ask the language server for diagnostics; surface
  compile errors back to the agent.

The four are bundled here because they all extend the tool catalog with similar shape and
shared considerations (sandboxing, permission, observability).

## Architecture

```
theway/src/tools/
  web_fetch.rs       reqwest client + readability-style HTML → Markdown
  web_search.rs      pluggable backend trait + builtins
  git.rs             git2 crate (libgit2 binding) for structured ops
  lsp.rs             tower-lsp-client; spawns server, requests diagnostics for a path
```

`web_fetch` defaults to `text/html → md` via `readability` style extraction; respects
`robots.txt`; configurable user agent.

`web_search` backend choice is `~/.theway/config.toml`. No built-in API keys — user supplies.

`git` is structured (returns parsed status / diff hunks) so the LLM doesn't have to parse
porcelain.

LSP integration is the trickiest:

- Spawn appropriate server per language (rust-analyzer, typescript-language-server,
  gopls, pyright). Lazy launch; reuse across tool calls.
- `lsp::after_edit(path)` triggers a `didChange` + waits for diagnostics. Returns the set with
  line/col/severity/message.
- `Agent::after_tool_call` hook (already exists) attaches the diagnostics to the `edit`/`write`
  tool result so the LLM sees "you broke the build" without an extra round trip.

## Stability

- LSP servers are subprocesses with all the failure modes you'd expect — crash, hang, slow
  startup. Each language server runs with a watchdog; misbehaving servers are quarantined for
  the session.
- `web_fetch` accepts an explicit allow-list / deny-list in `~/.theway/config.toml`; no
  sandbox-based network isolation (sandboxing was de-scoped at the master level).
- `git` operations are read-only by default; `git apply`/`commit`/`push` require explicit
  approval via [[03-approval-permissions]].
- LSP diagnostics attached to tool results are size-capped (e.g. 50 entries) — overflow turns
  into a summary.

## Extensibility

- Web-search backend trait; new providers drop in.
- LSP language map is config-driven (`~/.theway/lsp.toml`): file extension → server command.
- Git tool exposes a subcommand enum; new subcommands add via match arm.

## Performance

- `web_fetch` has a 15s timeout and 5 MiB body cap.
- LSP servers reuse a long-lived process — no per-edit spawn.
- Diagnostics requests have a 2s timeout; if the server is slow, the tool result reports "lsp
  timed out" rather than blocking the agent loop.

## Testing

| Layer | What |
|---|---|
| **unit** | Readability extraction on a fixture HTML doc; git status parser handles edge cases (empty repo, detached HEAD); LSP capability negotiation. |
| **integration** | `git::status` reports a dirty file inside a tempdir-init repo. `web_fetch` against a local `wiremock` server returns expected Markdown. |
| **e2e** | LSP: agent writes a file with a syntax error; the `write` tool result includes `diagnostics: [...]`; on the next turn, the agent receives the error and can react. `git`: agent calls `git diff` and gets structured hunks. |

## Acceptance criteria

- `web_fetch` returns Markdown body from a URL within timeout.
- `web_search` returns ranked results from the configured backend.
- `git status/diff/log` are structured and parseable in tests without regex.
- An edit that breaks compilation surfaces diagnostics back to the agent automatically.

## Out of scope

- Multi-language LSP polyglot in the same project — v1 picks one server per file extension.
- `git push` / GitHub PR creation — separate sub-issue (could live under
  [[09-extensions-skills]] as a `gh` extension).
- Search result re-ranking with an LLM.
