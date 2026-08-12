# mcp-notify-python — minimal MCP server that pushes notifications into theway

A self-contained Python example showing how to feed events into theway's trigger runtime via
the MCP server-push notification channel. Uses only the Python standard library; no MCP
SDK, no dependencies.

The server speaks JSON-RPC 2.0 over stdio (one message per line). It registers zero tools
and emits a `notifications/theway/demo/heartbeat` event every 10 seconds. Each heartbeat
carries a unique `_meta.theway_dedup_key` plus a human-readable `_meta.theway_summary`, so theway's
`McpNotificationHook` accepts each one as a distinct trigger and persists the summary into
the audit.

## Files

- `notify-server.py` — the MCP server. ~150 lines, stdlib only.
- `mcp.toml` — the theway config snippet that wires the server in.

## Run it with theway

1. Copy or merge the `[[server]]` block from `mcp.toml` into one of theway's MCP registries.
   The user-global location applies everywhere; the project-local location applies only
   when theway is launched from that repo.

   ```sh
   # user-global (works in every project):
   mkdir -p ~/.theway
   cp mcp.toml ~/.theway/mcp.toml   # or merge the [[server]] block into an existing file

   # OR project-local (only this repo):
   mkdir -p /path/to/your/project/.theway
   cp mcp.toml /path/to/your/project/.theway/mcp.toml
   ```

   The `args` path is relative to the cwd theway is launched from. Replace it with an
   absolute path to the `notify-server.py` if you want the config to work from anywhere.

2. Set a provider API key, then start theway:

   ```sh
   export OPENAI_API_KEY=sk-...   # or ANTHROPIC_API_KEY etc
   theway
   ```

   The startup banner should show `[mcp: connected to 1 server(s), 0 extra tool(s)]` and
   `[trigger sources: watching 1 configured MCP push source(s)]`. If the server failed to
   spawn, a diagnostic line is printed instead.

3. In the REPL, type `/triggers` to see the runtime status. Within ~12s the engine should
   show `accepted=1`, then `2`, `3`, … as heartbeats arrive. Each one is persisted to the
   session JSONL as a `Custom { customType: "trigger" }` entry.

## What the example demonstrates

| Concept | How it shows up |
| --- | --- |
| **JSON-RPC over stdio** | Server reads requests from stdin line-by-line, writes responses + notifications to stdout line-by-line. Logs go to stderr to keep the protocol channel clean. |
| **`initialize` handshake** | Server responds with `protocolVersion = "2025-03-26"`, empty capabilities, and a `serverInfo` block. theway's `theway_ai_mcp::McpClient::initialize` requires this before `tools/list`. |
| **Server-push notifications** | `notifications/theway/demo/heartbeat` is emitted on a background thread every 10s. JSON-RPC notifications have no `id` field — that's how theway's read pump (`crates/theway-mcp/src/client.rs`) routes them to the `take_notifications()` channel instead of the response router. |
| **Custom-method idempotency** | The MCP method is non-standard (not `tools/listChanged` etc), so the adapter requires an explicit `_meta.theway_dedup_key`. Without it the event is dropped. See `McpNotificationHook::idempotency_for` in `crates/coding-agent/src/triggers/mcp_notification_hook.rs`. |
| **Per-server key namespacing** | The runtime sees keys as `mcp:demo-notify:custom:heartbeat:<N>`, not the bare `heartbeat:<N>` the server emitted. This prevents collisions between servers and prevents user-supplied custom keys from colliding with built-in MCP method slots. |
| **Privacy contract** | The hook is hardcoded to `payload_visibility = Local`. The full params blob (including the illustrative `counter` / `ts` fields outside `_meta`) is dropped before persistence — only `payload_summary` survives into the audit. Opt in to per-event detail via `_meta.theway_summary`, which the server is declaring as safe to persist. |

## Make theway act on the events

By default, notifications are sunk and audited but no agent action runs. To get theway to
take action when a heartbeat arrives, install a dynamic trigger rule from chat:

```text
when mcp:demo-notify fires notifications/theway/demo/heartbeat, append the summary to /tmp/theway-heartbeats.log
```

theway's `NewTrigger` tool will persist the rule to a session sidecar, and each subsequent
heartbeat dispatches a sub-agent that inherits the parent harness config but starts with a
fresh conversation context.

## Make your own notification source

Swap the heartbeat body for whatever event source you want theway to react to: GitHub webhook
forwarder, MQTT bridge, file watcher, build-system finisher, etc. The contract is just:

1. Speak JSON-RPC 2.0 over stdio. Respond to `initialize` + `tools/list`.
2. Emit each event as a JSON-RPC notification (no `id`) on stdout, one per line.
3. For non-spec methods, include `_meta.theway_dedup_key` so the adapter accepts the event.
4. Include `_meta.theway_summary` only when the human-readable string is safe to persist —
   payload params themselves are dropped before audit.
