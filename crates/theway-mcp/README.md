# theway-mcp

English | [中文](README.zh.md)

`theway-mcp` is a transport-independent MCP client used by `theway-daemon` to load external tools and receive server notifications. It implements JSON-RPC 2.0 request correlation, the MCP initialize handshake, `tools/list`, `tools/call`, cancellation notification, stdio subprocess transport, and Streamable HTTP transport.

The crate does not depend on `theway-core` and does not convert MCP tools into agent tools. That adapter and all server-side MCP behavior belong to the daemon.

## Public API

- `McpClient` owns initialization state, request ids, in-flight responses, the cached tool catalog, cancellation, and the notification receiver.
- `Transport` is the asynchronous newline-delimited JSON abstraction used by the client.
- `StdioTransport` starts and supervises an MCP subprocess over stdin/stdout.
- `HttpMcpTransport` sends requests over HTTP and receives direct or SSE responses with bounded bodies, idle timeouts, bearer authentication, and reconnect policy.
- `protocol` contains the request, response, notification, tool, server-info, and content records used by the implemented MCP operations.

## Documentation

- [Client and transport architecture](docs/architecture.md)

## Validation

```bash
cargo test -p theway-mcp
cargo doc -p theway-mcp --no-deps --document-private-items
```
