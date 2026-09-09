# theway configuration guide

English | [中文](theway-config.zh.md)

Configuration reference for an agent working inside theway: where every config file lives, what each section controls, precedence, and when a change takes effect. This document is bundled into the `theway` binary and installed to `~/.theway/docs/tui.md` on startup, so every install ships it; the `tui-docs` extension points your prompt at this path.

## Base directories and layering

- `$THEWAY_DIR` (default `~/.theway`) is the user root. A project layer `<cwd>/.theway/` overlays it per working directory.
- The primary config file is `<base>/config.toml`. Legacy files `<base>/theme.toml` and `<base>/mcp.toml` remain as fallbacks when `config.toml` has no `[theme]` / `[[server]]` content. The project layer adds `<cwd>/.theway/mcp.toml`, `<cwd>/.theway/skills/`, `<cwd>/.theway/templates/`, and `<cwd>/.theway/extensions/`.
- Runtime state also lives under `<base>`: `sessions/` (per-cwd hash buckets), `memory/`, `history`, `exports/`, `logs/`, `auth.json`, `skill-overrides.json`, `extensions/trust.json`, `extensions/audit.jsonl`.

## config.toml

Read by the client at startup and provisioned to the daemon as a settings payload; the daemon does not read this file itself. On a fresh install or first client start the file is created with `[executor] kind = "local"` plus commented-out `[model]` / `[[model.custom]]` samples; existing files are never overwritten. `[model] api_key` is a local credential fallback — provider environment variables still win, then `auth.json`. Precedence is CLI flags > config.toml > built-in default.

| Section | Keys | Meaning |
|---|---|---|
| `[executor]` | `kind` | Execution environment for executor-backed tools: `local` (default) or `sandbox`. Startup-only; changes require a client restart. |
| `[model]` | `provider`, `model`, `thinking`, `base_url`, `api_key`, `auto_fetch_models` | Startup default model pair and thinking level (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` when the provider supports it). `base_url` points at a local OpenAI-compatible server; `auto_fetch_models = true` imports its catalog from `GET <base_url>/models` and fills an unset `model` from the first entry; `api_key` is the local credential fallback. The TUI writes the last `/model` pick here. |
| `[[model.custom]]` | `id` (required), `name`, `api`, `provider`, `base_url`, `reasoning`, `thinking_level_map`, `input`, `cost`, `context_window`, `max_tokens`, `headers`, `compat` | Custom model descriptor. Omitted `provider` / `base_url` inherit `[model]`; other defaults are `openai-completions`, 128000 context / 8192 output, text-only input. Replaces the former `models.json` files. |
| `[builtin_skills]` | `enabled` | Enabled built-in skill names; unioned with `--builtin-skill` flags. |
| `[triggers]` | `poll_interval_secs` | Local dynamic-trigger poll interval (default 600). |
| `[tui]` | `max_feed_lines` | TUI feed scrollback cap. |
| `[relay]` | `base_url` | Relay base URL fallback. |
| `[orchestrator]` | `thinking_summary`, `thinking_summary_min_chars` | When `thinking_summary = true`, finished thinking bursts are replaced by a summarizer subagent output. `thinking_summary_min_chars` defaults to 2000. |
| `[ui.feed]` | `thinking_mode` | How thinking renders: `full`, `peek`, or `hidden` (Ctrl+O cycles). |
| `[ui.panel]` | `mode`, `position`, `show_hooks`, `show_runtime` | Side panel visibility (`auto`, `shown`, `hidden`) and position (`top`, `bottom`, `left`, `right`); `show_hooks` / `show_runtime` opt into diagnostic sections. |
| `[ui.graph]` | `position` | Graph band position: `composer-top` or `side-panel`. |
| `[[server]]` | same fields as mcp.toml entries | MCP servers defined directly in `config.toml`. A non-empty list wins over the legacy mcp.toml files. |

Malformed values fail soft: the client reports a diagnostic and keeps the default. A running daemon keeps the values it was provisioned with; changing the file requires a client restart (or the settings RPC) to take effect.

## theme.toml and [theme]

Theme settings can live in `<base>/theme.toml` or under a `[theme]` table in `config.toml`; the `config.toml` table wins. Versioned theme format (v2). Missing file, missing table, or unknown sections/keys degrade to the built-in default and warn on stderr.

| Section | Controls |
|---|---|
| `[palette]` | Named colors referenced elsewhere. |
| `[colors]` | Color roles (user/assistant/tool/thinking text and backgrounds, status, picker, sidebar, dag band). |
| `[screen]` | Viewport inset: `margin` (all four sides) plus per-side `margin_top` / `margin_right` / `margin_bottom` / `margin_left`; default left margin is 2. |
| `[blocks.<kind>]` | Block layout for `user` / `assistant` / `tool` / `thinking`: `margin_top`, `margin_bottom`, `border_bottom`, background. |
| `[composer]` | Composer chrome colors: `border_focused`, `border_unfocused`, `prefix`, `text`, `bg`, `info_text`, `placeholder`, `hint`, `cursor`. |
| `[feed]` | Feed rhythm: `gap` between blocks, `separate_all`, separator style. |
| `[statusbar]` / `[picker]` / `[sidebar]` / `[dag_band]` | Component style tables (foreground/background color slots, some accept `transparent`). |

Theme edits hot-reload: after a daemon-side reload bumps the runtime revision, connected clients re-read the file without a restart.

## mcp.toml and [[server]] in config.toml

MCP stdio/HTTP server definitions under `[[server]]` entries: `name`, `kind`, `command`, `args`, `endpoint`, `auth`, timeouts, `reconnect`, and the notification options `inject_summary` / `inject_and_run`. A non-empty `[[server]]` list in `config.toml` wins; otherwise the client reads `<base>/mcp.toml` and `<cwd>/.theway/mcp.toml` and merges them. A server that fails to start is skipped with a diagnostic.

## Skills, templates, and extensions

- Skills resolve from `~/.theway/skills/`, `<cwd>/.theway/skills/`, and built-ins; closer layers override by name. `/reload` rescans skills and file commands.
- Templates (`.md` with frontmatter) resolve from `~/.theway/templates/` and `<cwd>/.theway/templates/`; run with `/template <name>`.
- Extension packages resolve from `<base>/extensions-managed/`, `<base>/extensions/`, and `<cwd>/.theway/extensions/` (project > user > managed). The official packages (`tui-docs`) are embedded in the daemon binary and auto-provisioned into the managed layer at startup. Project packages require a trust record in `<base>/extensions/trust.json`; manage via `/extension-trust` and reload with `/extension-reload`.

## Guidance for an agent

- Prefer the slash commands for configuration changes: `/model`, `/thinking`, `/skills`, `/extension-trust`, `/extension-reload`, `/reload`. They take effect in the running daemon.
- Edit files directly only when the user asks for it; for `config.toml` a client restart is needed before the change reaches the daemon.
- When debugging "why is X configured this way", check the precedence order first, then the layered locations above.
