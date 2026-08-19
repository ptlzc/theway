# AGENTS.md — theway-mcp

This file adds crate-specific instructions to [`../../AGENTS.md`](../../AGENTS.md). Read the [crate overview](README.md) and [client architecture](docs/architecture.md) before changing request correlation or transport behavior.

## Boundary rules

- Keep the crate independent of `theway-core`, daemon configuration, tool policy, trigger delivery, and UI code.
- Keep MCP-to-`AgentTool` conversion and MCP server behavior in [`theway-daemon`](../theway-daemon/README.md).
- Add protocol records only for operations implemented by this client or required to decode their responses and notifications.

## Lifecycle and security rules

- Remove every in-flight request on response, timeout, cancellation, transport close, and future drop.
- Preserve the separation between response correlation and server notification delivery.
- Keep stdio and HTTP behavior equivalent at the `Transport` trait.
- Bound HTTP bodies, SSE buffers, idle waits, reconnect delays, and cancellation sends.
- Redact bearer credentials from debug output, diagnostics, and errors.

## Tests and documentation

- Use local subprocess or HTTP fixtures; tests must not contact external MCP servers.
- Cover fragmented SSE, heartbeat events, direct JSON responses, reconnect/cancel paths, response mismatch, notification delivery, and child shutdown when those paths change.
- Update [docs/architecture.md](docs/architecture.md) when the handshake, request lifecycle, protocol subset, or transport behavior changes.
- Run `cargo test -p theway-mcp` and `cargo doc -p theway-mcp --no-deps --document-private-items`.
