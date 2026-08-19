# theway-transport 修改规则

本文件适用于 `crates/theway-transport/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改 wire 记录、protobuf 服务或 carrier 行为前，先阅读 [crate 概览](README.md)和[传输架构](docs/architecture.md)。

## 边界规则

- 本 crate 与 `theway-core`、`theway-daemon`、`theway-storage` 及所有 UI crate 保持独立。
- 跨客户端请求、结果、snapshot 和事件记录先在这里定义，再实现 daemon 或客户端行为。
- 模型、会话、工具和图策略留在服务端实现；gRPC、HTTP、SSE 和 WebSocket handler 只做转换与路由。
- MCP 客户端与 server 行为分别放在 [`theway-mcp`](../theway-mcp/README.md) 和 [`theway-daemon`](../theway-daemon/README.md)。

## 协议规则

- 需要有序执行的运行时变更通过 `WireCommand` 路由；独立读取和控制使用操作 trait。
- 修改增量 feed/status frame 时，保留完整 snapshot 恢复路径。
- Proto 文件、Rust 转换模块、服务实现、客户端包装和 TypeScript SDK 必须一起修改；proto 变化后运行 `make sdk-sync`。
- 共享操作与错误在 gRPC 和 web carrier 上保持语义一致。
- 复用 [`theway-contract`](../theway-contract/README.md) 的叶子记录，不创建 transport 自有副本。

## 测试与文档

- 每个新 protobuf 字段要补转换往返测试；carrier 要覆盖路由、校验、流式传输、lag 和断线行为。
- 镜像测试遵循 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md)。
- 端点归属、snapshot 语义、carrier、发现机制或协议生成变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-transport`、`cargo doc -p theway-transport --no-deps --document-private-items` 和 `make layering-check`；proto 变化还要运行 `make sdk-sync` 及生成差异检查。
