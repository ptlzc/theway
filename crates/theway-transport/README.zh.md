# theway-transport

[English](README.md) | 中文

`theway-transport` 负责跨客户端 wire 模型，以及控制 theway daemon 的 gRPC 与 web 传输。它提供生成的 protobuf 服务、HTTP JSON-RPC、SSE、WebSocket 事件、带类型的 gRPC 客户端、daemon 发现辅助函数和面向传输的操作 trait。

本 crate 独立于 `theway-core`、`theway-daemon` 和 `theway-storage`。服务端实现 `TransportHost` 并提供 `TransportEndpoints`；客户端只使用 wire/protobuf 类型和 `GrpcClient`，不访问运行时内部状态。

## 协议入口

- `wire` 定义命令、完整和增量状态 snapshot、图/job 事件、配置及操作请求/结果记录。
- `transport` 定义 `TransportEndpoints` 以及 `SessionOps`、`JobOps`、`GraphOps`、`ToolOps`、`StorageOps`。
- `grpc`、`http` 和 `ws` 通过 protobuf RPC、JSON-RPC、SSE 与 WebSocket 暴露这些操作。
- `proto`、`tools` 和 `state` 在内部 wire 记录与生成的 protobuf 消息之间转换。
- `client` 包装 tonic 客户端，并按工作目录发现或启动 daemon。
- `feed`、`commands`、`auth`、`history`、`images`、`mentions` 等共享模块定义不绑定具体 carrier 的客户端/daemon 数据。

MCP 传输不在本 crate 实现：外部 MCP 客户端位于 `theway-mcp`，daemon 的 MCP server 位于 `theway-daemon`。

## 文档

- [Wire 与传输架构](docs/architecture.md)

## 验证

```bash
cargo test -p theway-transport
cargo doc -p theway-transport --no-deps --document-private-items
make layering-check
```
