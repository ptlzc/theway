# theway-mcp architecture

## Scope

`theway-mcp` implements the client half of the Model Context Protocol needed by theway: initialize, tool discovery and invocation, request cancellation, and server notifications over stdio or Streamable HTTP. Sampling, resource subscriptions, and MCP server behavior are outside this crate.

No runtime-engine types appear in the public API. [`theway-daemon`](../../theway-daemon/docs/architecture.md) wraps `McpTool` definitions as model-facing tools and decides how notifications enter trigger processing.

## Protocol records

[`protocol.rs`](../src/protocol.rs) defines the negotiated protocol version, initialization records, tool definitions and results, supported content variants, JSON-RPC requests/errors/notifications, and request builders. Unknown or unsupported server payloads fail through [`McpError`](../src/errors.rs) rather than being converted into agent-runtime errors here.

## Client lifecycle

[`client.rs`](../src/client.rs) owns one `Arc<dyn Transport>`, monotonically increasing request ids, an in-flight response map, initialization state, cached tools, and a server-notification channel.

The client read loop classifies each received JSON line as a response or notification. Responses complete the matching in-flight request; notifications are forwarded to the receiver obtained through `take_notifications`. Dropping an in-flight guard removes its pending map entry so timed-out or cancelled calls do not leak correlation state.

`initialize` records server information and capabilities, then sends the initialized notification. `tools_list` refreshes the catalog. `tools_call` supports an optional cancellation token; cancellation sends `notifications/cancelled` within a bounded send budget and completes the local call with the corresponding error.

`close` closes the transport and terminates pending activity. Request timeouts are client configuration and apply consistently across transport implementations.

## Transport implementations

[`transport.rs`](../src/transport.rs) defines newline-oriented `send_line`, `recv_line`, and `close` operations. Framing remains below `McpClient`, while JSON-RPC correlation remains above it.

[`stdio.rs`](../src/stdio.rs) spawns a child with piped stdin and stdout, writes one JSON document per line, reads stdout lines, and terminates the subprocess on close.

[`http.rs`](../src/http.rs) implements Streamable HTTP. Outbound JSON-RPC messages are POSTed to the configured endpoint. Response bodies may contain JSON directly or an SSE stream; the SSE parser ignores heartbeat events and forwards data fields as JSON lines. Body caps, request timeout, SSE idle timeout, reconnect backoff, last-event-id handling, and cancellation bound network resource use. The `Debug` implementation for `HttpMcpAuth` redacts bearer tokens.

## Invariants

- Request ids identify exactly one in-flight response and are removed on completion, timeout, cancellation, or drop.
- Server notifications never satisfy a pending request and responses never enter the notification stream.
- Both transports present the same newline-delimited JSON behavior to `McpClient`.
- HTTP response and SSE buffers remain bounded, and idle streams are cancellable.
- Authentication values are not emitted by debug formatting or error messages.
- Agent-tool adaptation, notification delivery policy, and MCP server mode remain daemon-owned.
