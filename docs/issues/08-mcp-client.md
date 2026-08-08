# MCP (Model Context Protocol) client

> Parent: master roadmap issue.
> Tier: 4 (framework depth).

## Goal

MCP is becoming the de-facto pluggable-tools protocol across the ecosystem (Claude Code,
Continue, Zed, OpenAI Agents SDK). `theway` needs to be an MCP **client** so any MCP server
(filesystem, git, GitHub, Slack, custom) plugs in without code changes.

Ship:

- MCP stdio + SSE transports.
- Server registry in `~/.theway/mcp.toml` (and project `.theway/mcp.toml`).
- Tools, prompts, and resources exposed via MCP are registered with the agent at startup; tool
  calls go through MCP RPC.
- `/mcp` slash command lists active servers, status, tool count; `/mcp restart <server>`
  reconnects.
- Permission integration: MCP-provided tools route through the same approval pipeline from
  [[03-approval-permissions]].

## Architecture

```
theway-mcp/                       new crate, sibling to theway-core
  src/
    transport/                  stdio.rs, sse.rs, ws.rs (latter behind feature)
    protocol/                   2024-11-05 + 2025-03-26 schema, serde types
    client.rs                   ClientSession: handshake, capabilities, request/notify
    registry.rs                 spawn/restart, status, lifecycle
    adapter.rs                  McpTool: impl theway_ai_harness_core::Tool by RPC delegation
```

`theway` depends on `theway-mcp` and registers an `McpToolset` alongside builtins. The
McpToolset is dynamic: as servers reconnect, the set updates; the agent receives a "tools
changed" event and the system prompt regenerates inventory.

## Stability

- Each MCP server runs in its own subprocess (stdio) or HTTP connection (SSE). Crash isolation:
  one server dying does NOT take down others or the agent.
- Reconnect with exponential backoff. After N failures, mark `degraded` and surface in `/mcp`.
- Strict JSON-RPC framing — malformed frames are logged + skipped, never panic.
- Tool calls have a per-call timeout (configurable, default 30s); on timeout the call returns
  a synthetic ToolError without leaking subprocess state.
- All MCP tool invocations log full (request, response, duration) to the structured logger
  from [[14-observability]].

## Extensibility

- Transport trait so non-stdio/SSE transports (WebSocket, in-process) drop in.
- `McpFeature` enum gates optional capabilities (resources, prompts, sampling) — server says
  what it supports, client doesn't assume.

## Performance

- Servers connect lazily on first tool need by default (configurable to eager).
- Tool calls are streaming-capable for servers that support it; otherwise full response then
  emit.
- Tool catalog cached in memory; invalidation by server-pushed `tools/listChanged`
  notification.

## Testing

| Layer | What |
|---|---|
| **unit** | JSON-RPC frame parser; capability negotiation; backoff schedule. |
| **integration** | Spawn an in-process MCP server (test fixture) over stdio; agent calls a tool that the server exposes; result reaches the agent loop as a ToolResult. Server kill → agent receives a "tool unavailable" error on next call, then reconnect populates. |
| **e2e** | Real `mcp-server-filesystem` (pinned npm version) launched as a subprocess; agent uses its `read_file` against a tempdir fixture; full round-trip with the faux LLM provider scripted to call it. |

## Acceptance criteria

- `~/.theway/mcp.toml` with a stdio server entry causes the server to spawn and its tools to
  appear in `/tools` listing.
- An MCP tool call goes through the permission system — denial returns a tool error without
  invoking the server.
- Killing the server process: `/mcp` reflects `down`; the next agent attempt to use its tool
  fails clearly; reconnection restores availability.
- Two servers exposing same tool name produce a deterministic conflict resolution (project >
  user > builtin; collisions logged).

## Out of scope

- MCP **server** mode (theway itself serving MCP) — separate roadmap item.
- Auth flows for OAuth-protected MCP servers — design here but ship via [[12-login-oauth]].
- Resource subscriptions (real-time push of resource updates) — design ready, ship behind a
  feature flag.
