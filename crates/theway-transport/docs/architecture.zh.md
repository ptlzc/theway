# theway-transport 架构

[English](architecture.md) | 中文

## 依赖位置

`theway-transport` 使用 `theway-contract` 的共享持久化记录，并使用 `theway-llm-provider` 中被配置和 snapshot 复用的模型数据。它不依赖运行时引擎、daemon 应用或存储实现。

该依赖方向使服务端或客户端无需链接 `AgentHarness`、SQLite、终端渲染或 daemon 工具即可使用协议。

## Wire 模型与端点 API

[`wire.rs`](../src/wire.rs) 是服务端事件循环与 JSON 传输共享的 serde 表示。`WireCommand` 把变更送入串行运行时循环。`WireStatus` 是客户端权威 snapshot；`WireStatusUpdate` 可携带完整 snapshot，或仅在 base index 与接收方 snapshot 匹配时应用的 feed delta。

[`transport.rs`](../src/transport.rs) 定义面向服务端的 API：

- `TransportEndpoints` 包含命令 channel、状态 broadcaster 与最新 snapshot、agent/DAG 事件 broadcaster、会话标识和路径/配置视图，以及操作 trait object。
- `SessionOps`、`JobOps`、`GraphOps`、`ToolOps` 和 `StorageOps` 暴露无需直接访问运行时状态的请求/响应操作。
- 宿主不支持某个可选操作组时，`Unavailable*` 实现提供明确错误或空行为。

[`host.rs`](../src/host.rs) 定义 `TransportHost`。服务端把 endpoints 交给 transport，再让串行应用循环与 server task 并行运行；具体实现由 daemon 提供。

## gRPC 传输

[`commands.proto`](../proto/commands.proto)、[`events.proto`](../proto/events.proto)、[`state.proto`](../proto/state.proto) 等 protobuf 文件是服务与消息的事实来源。[`build.rs`](../build.rs) 使用 `protox` 和 `tonic-prost-build` 编译全部 proto，因此不要求系统安装 `protoc`。

[`grpc/mod.rs`](../src/grpc/mod.rs) 把 command、session、settings、graph、event、tool、storage 和 health 服务映射到 `TransportEndpoints`。必须与 turn 串行的变更操作入队 `WireCommand`；读取/控制 trait 通过各自 endpoint object 执行。事件订阅先收到当前状态，再接收增量 frame；发生 lag 时从最新权威 snapshot 恢复。

[`proto.rs`](../src/proto.rs)、[`tools.rs`](../src/tools.rs) 和 [`state.rs`](../src/state.rs) 负责会话状态、工具操作和运行时存储记录的 protobuf 转换。Proto 变化必须同步更新这些转换，并通过 `make sdk-sync` 更新生成的 TypeScript SDK。

## Web 传输

[`http.rs`](../src/http.rs) 从与 gRPC 相同的 endpoint 集合提供 health、JSON-RPC、SSE 事件和 WebSocket upgrade 路由。[`ws.rs`](../src/ws.rs) 接收 JSON 命令，并以 JSON frame 发布 status、agent 和 DAG 事件。

HTTP 与 WebSocket handler 把 carrier 输入转换为 transport 自有请求或 `WireCommand`，不实现模型、会话、存储、工具或图策略。

## 客户端与 daemon 发现

[`client.rs`](../src/client.rs) 把生成的 tonic 客户端包装为 `GrpcClient`，并暴露带类型的命令、状态流、会话/图控制、controller 工具服务和存储服务调用。

Daemon 发现从 `${THEWAY_DIR:-$HOME/.theway}` 读取按工作目录区分的 port/pid 文件，探测候选 loopback 地址，只在所有权匹配时删除过期记录，并可启动 `thewayd` 后等待就绪。发现机制面向 loopback，不额外定义认证协议。

## 共享客户端记录

[`feed/mod.rs`](../src/feed/mod.rs)、[`commands.rs`](../src/commands.rs)、[`auth.rs`](../src/auth.rs)、[`history.rs`](../src/history.rs)、[`images.rs`](../src/images.rs) 和 [`mentions.rs`](../src/mentions.rs) 等模块定义可复用的客户端/daemon 记录与纯辅助函数。叶子路径、trigger、cron 和原始持久化定义留在 `theway-contract`，只在需要稳定 transport 路径时重新导出。

## 不变量

- Wire 与 protobuf 记录不包含 core 或 daemon 私有类型。
- 所有 carrier 驱动同一套 `TransportEndpoints` 语义；carrier 专用 handler 不承载业务策略。
- 需要排序的运行时变更进入串行命令队列。
- Snapshot delta 只应用于匹配的 base；lag 或不匹配后通过完整权威 snapshot 恢复。
- Proto 源、Rust 转换、服务 handler、客户端调用和生成 SDK 同步变化。
- 本 crate 不包含客户端外观、终端输入处理、存储后端或 MCP 实现。
