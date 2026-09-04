# theway configuration guide

English | [中文](theway-config.zh.md)

Configuration reference for an agent working inside theway: where every config file lives, what each section controls, precedence, and when a change takes effect. This document is bundled into the `theway` binary and installed to `~/.theway/docs/tui.md` on startup, so every install ships it; the `tui-docs` extension points your prompt at this path.

## Base directories and layering

- `$THEWAY_DIR` (default `~/.theway`) is the user root. A project layer `<cwd>/.theway/` overlays it per working directory.
- Config files: `<base>/config.toml`, `<base>/theme.toml`, `<base>/mcp.toml`. The project layer adds `<cwd>/.theway/mcp.toml`, `<cwd>/.theway/skills/`, `<cwd>/.theway/templates/`, and `<cwd>/.theway/extensions/`.
- Runtime state also lives under `<base>`: `sessions/` (per-cwd hash buckets), `memory/`, `history`, `exports/`, `logs/`, `auth.json`, `models.json`, `skill-overrides.json`, `extensions/trust.json`, `extensions/audit.jsonl`.

## config.toml

Read by the client at startup and provisioned to the daemon as a settings payload; the daemon does not read this file itself. Precedence is CLI flags > config.toml > built-in default.

| Section | Keys | Meaning |
|---|---|---|
| `[model]` | `provider`, `model`, `thinking` | Startup default model pair and thinking level (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`). The TUI writes the last `/model` pick here. |
| `[builtin_skills]` | `enabled` | Enabled built-in skill names; unioned with `--builtin-skill` flags. |
| `[triggers]` | `poll_interval_secs` | Local dynamic-trigger poll interval (default 600). |
| `[tui]` | `max_feed_lines` | TUI feed scrollback cap. |
| `[relay]` | `base_url` | Relay base URL fallback. |
| `[orchestrator]` | thinking-summary settings | Orchestrator thinking-summary tuning. |

Malformed values fail soft: the client reports a diagnostic and keeps the default. A running daemon keeps the values it was provisioned with; changing the file requires a client restart (or the settings RPC) to take effect.

## theme.toml

Versioned theme file (v2). Missing file or unknown sections/keys degrade to the built-in default and warn on stderr.

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

## mcp.toml

MCP stdio/HTTP server definitions under `[[server]]` entries: `name`, `kind`, `command`, `args`, `endpoint`, `auth`, timeouts, `reconnect`, and the notification options `inject_summary` / `inject_and_run`. Read from `<base>/mcp.toml` and `<cwd>/.theway/mcp.toml`; a server that fails to start is skipped with a diagnostic.

## Skills, templates, and extensions

- Skills resolve from `~/.theway/skills/`, `<cwd>/.theway/skills/`, and built-ins; closer layers override by name. `/reload` rescans skills and file commands.
- Templates (`.md` with frontmatter) resolve from `~/.theway/templates/` and `<cwd>/.theway/templates/`; run with `/template <name>`.
- Extension packages resolve from `<base>/extensions-managed/`, `<base>/extensions/`, and `<cwd>/.theway/extensions/` (project > user > managed). Project packages require a trust record in `<base>/extensions/trust.json`; manage via `/extension-trust` and reload with `/extension-reload`.

## Guidance for an agent

- Prefer the slash commands for configuration changes: `/model`, `/thinking`, `/skills`, `/extension-trust`, `/extension-reload`, `/reload`. They take effect in the running daemon.
- Edit files directly only when the user asks for it; for `config.toml` a client restart is needed before the change reaches the daemon.
- When debugging "why is X configured this way", check the precedence order first, then the layered locations above.
