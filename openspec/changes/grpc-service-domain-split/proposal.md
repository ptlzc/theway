# Proposal: grpc-service-domain-split

## Why

领域 proto 拆分后，消息定义已经各归其位，但全部 RPC 仍集中在 `theway_grpc.proto` 的单一 `service ThewayGrpc` 里。阅读一个领域时必须在消息文件和入口 service 文件之间来回跳，结构定义与 service 割裂。protobuf 不允许同一个 `service` 分散在多个文件定义，因此要做到“一个领域文件 = message + service 自洽”，只能把单服务拆成多个领域服务。

## What Changes

- **`commands.proto`** 新增 `service CommandService`：`SendMessage` / `SetModel` / `Cancel` / `Approve`。`Empty` 与 `CommandResult` 作为跨领域共享原语继续由该文件持有。
- **`session.proto`** 新增 `service SessionService`：`GetState` / `ListSessions` / `CreateSession` / `SwitchSession` / `RenameSession` / `DeleteSession`。
- **`graph_engine.proto`** 新增 `service GraphEngineService`：`GetNodeOutput` / `GraphCancel` / `GraphRetry` / `GraphSkip` / `GraphNodeInterrupt` / `GraphNodeSteer` / `GraphCheckpoint` / `GraphRestore` / `GraphList`。
- **`events.proto`** 新增 `service EventService`：`StreamEvents`。
- **删除 `theway_grpc.proto`**：迁移完成后不再有聚合 service 入口，Rust/TS 构建改为编译四个领域 proto + `health.proto`。
- **Rust transport**：`GrpcState` 实现四个 tonic service trait；`serve_grpc` 注册四个 service；`GrpcClient` 高层 API 不变，内部改为共享一个 `Channel` 的四个生成客户端。
- **probe**：直接使用生成客户端的两处改接 `SessionServiceClient` 与 `CommandServiceClient`。
- **TS SDK**：`ThewayGrpcClient` 类名与公开方法不变，内部改为持有四个领域 service stub；`src/index.ts` 导出四个生成模块。
- **Breaking change**：gRPC wire 路径从 `theway.grpc.v1.ThewayGrpc` 变为 `theway.grpc.v1.CommandService` / `SessionService` / `GraphEngineService` / `EventService`；旧客户端不兼容。消息字段编号、HTTP/WS/JSON 通道语义均不变。
- **迁移桥**：实现过程中先保留旧 `ThewayGrpc` 与新服务并存，等所有客户端切到新服务后，在 cutover 节点删除旧服务。

## Capabilities

### New Capabilities

- `grpc-domain-services`: gRPC 协议按领域定义四个 service（Command / Session / GraphEngine / Event），每个领域 proto 同文件承载其消息与服务定义；Rust 与 TS 客户端分别通过共享 channel 和四个 stub 访问，高层客户端 API 保持不变。

### Modified Capabilities

- 无。

## Impact

- **proto**: `proto/{commands,session,graph_engine,events}.proto` 增加 service 与必要 import；`proto/theway_grpc.proto` 在 cutover 删除；`sdk/proto/` 保持镜像。
- **Rust transport**: `crates/theway-transport/src/grpc.rs`（trait impl + 注册 + 日志）、`src/client.rs`（四个生成客户端）、`build.rs`（cutover 后编译全部领域 proto）、`tests/grpc/mod.rs`（直接生成客户端测试）。
- **probe**: `crates/theway-probe/src/main.rs`（生成客户端路径）、`build.rs`（cutover 后编译全部领域 proto）。
- **TS SDK**: `sdk/scripts/generate.sh`（输入文件）、`sdk/src/client.ts`（四个 stub）、`sdk/src/index.ts`（导出）、`sdk/src/generated/*`（重新生成，`theway_grpc.ts` 删除）。
- **行为**: 每个 RPC 的语义、消息类型、错误码和流行为不变；仅 service 路径变化。
- **不改变**: `theway_transport::client::GrpcClient` 与 `@theway-ai/sdk` 的高层类名/方法；`crate::proto::theway_grpc` Rust 模块路径；`grpc.health.v1.Health` 服务。
