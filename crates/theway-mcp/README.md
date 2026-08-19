# theway-mcp

`theway-mcp` 是与应用 transport 无关的 MCP 客户端，[`theway-daemon`](../theway-daemon/README.md) 用它加载外部工具并接收 server notification。它实现 JSON-RPC 2.0 请求关联、MCP initialize 握手、`tools/list`、`tools/call`、取消 notification、stdio 子进程传输和 Streamable HTTP 传输。

本 crate 不依赖 [`theway-core`](../theway-core/README.md)，也不把 MCP 工具转换为 agent 工具。该适配器及所有 server 侧 MCP 行为属于 daemon。

## 公开 API

- `McpClient` 负责初始化状态、请求标识、in-flight 响应、缓存工具目录、取消和 notification receiver。
- `Transport` 是客户端使用的异步换行分隔 JSON 抽象。
- `StdioTransport` 通过 stdin/stdout 启动并监管 MCP 子进程。
- `HttpMcpTransport` 通过 HTTP 发送请求并接收直接响应或 SSE 响应，同时限制 body、空闲超时、bearer 认证和重连策略。
- `protocol` 包含已实现 MCP 操作使用的请求、响应、notification、工具、server info 和内容记录。

## 文档

- [客户端与传输架构](docs/architecture.md)
- [Daemon MCP 集成](../theway-daemon/docs/architecture.md)

## 验证

```bash
cargo test -p theway-mcp
cargo doc -p theway-mcp --no-deps --document-private-items
```
