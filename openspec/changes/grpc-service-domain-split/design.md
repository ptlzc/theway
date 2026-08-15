# Design: grpc-service-domain-split

## Context

当前 `proto/` 已按四个领域组织消息：`commands.proto`、`session.proto`、`graph_engine.proto`、`events.proto`；`theway_grpc.proto` 只保留一个 `service ThewayGrpc` 并 import 这些文件。protobuf 的 service 定义不能跨文件续写，所以把 service 与消息放到同一领域文件里，必须以多个 service 取代单 service，构成协议 breaking change。

约束：消息字段编号与名称不变；Rust 高层 `GrpcClient` 和 TS 高层 `ThewayGrpcClient` 的公开 API 不变；健康检查服务独立不受影响；迁移期间每个提交都要保持构建通过。

## Goals / Non-Goals

**Goals:**

- 每个领域 proto 自洽：同文件包含该领域的 message/enum/service。
- 四个 gRPC service 覆盖原 `ThewayGrpc` 全部 20 个 RPC，无遗漏、无重复。
- Rust transport 与 TS SDK 高层客户端 API 兼容（只换底层 stub 拓扑）。
- 迁移序列可逐步提交：新服务与旧服务短暂并存，cutover 前全量测试绿。

**Non-Goals:**

- 不修改任何消息字段、枚举值或 RPC 语义。
- 不提供长期兼容别名 `ThewayGrpc`；cutover 后旧 service 彻底删除。
- 不改 HTTP/WS/JSON 通道。
- 不重命名 Rust 模块路径 `crate::proto::theway_grpc` 与 TS 高层类名 `ThewayGrpcClient`。

## Decisions

### Decision: 四个领域 service 映射

| 文件 | service | RPC |
| --- | --- | --- |
| `commands.proto` | `theway.grpc.v1.CommandService` | `SendMessage` / `SetModel` / `Cancel` / `Approve` |
| `session.proto` | `theway.grpc.v1.SessionService` | `GetState` / `ListSessions` / `CreateSession` / `SwitchSession` / `RenameSession` / `DeleteSession` |
| `graph_engine.proto` | `theway.grpc.v1.GraphEngineService` | `GetNodeOutput` / `GraphCancel` / `GraphRetry` / `GraphSkip` / `GraphNodeInterrupt` / `GraphNodeSteer` / `GraphCheckpoint` / `GraphRestore` / `GraphList` |
| `events.proto` | `theway.grpc.v1.EventService` | `StreamEvents` |

理由：按现有四个领域 proto 的边界切，不新增“命令与 session 谁拥有 Cancel”的交叉归属；`Cancel` 属于命令面，`GetState` 属于会话面，`GraphList` 属于 graph engine 面。

### Decision: 共享原语留在 `commands.proto`

`Empty` 与 `CommandResult` 被多个 service 引用，但移动它们会引入一个语义不清的 `common.proto`。保留在 `commands.proto`，其它领域通过 import 使用；文件头注释标明它们是跨领域共享原语。

Import 图（无环）：`graph_engine.proto` imports `commands.proto`；`session.proto` imports `graph_engine.proto` + `commands.proto`；`events.proto` imports `session.proto` + `commands.proto`。cutover 后没有 `theway_grpc.proto` 入口，构建系统显式编译四个文件 + `health.proto`。

### Decision: 迁移桥（dual service，中间态）

Phase 1 只给四个领域 proto 加 service，保留 `theway_grpc.proto` 的旧 `ThewayGrpc`。Phase 2-5 并行实现新服务的注册与客户端切换，旧服务仍注册。cutover 前所有调用方都已走新服务，最后一个节点删除旧 service、入口文件与旧生成文件。这样每个节点提交时构建和测试保持绿。

桥接期实现新 tonic trait 时用全路径（`impl theway_grpc::command_service_server::CommandService for GrpcState`），不把新 trait `use` 进模块作用域，避免新旧 trait 同名方法在 `state.send_message(...)` 测试调用处产生歧义；cutover 时删除旧 trait 后，再把四个新 trait 引入作用域供测试调用。

### Decision: Rust 高层客户端共享一个 `Channel`

`GrpcClient::connect` 建立一次 `tonic::transport::Channel`，用 `Channel::clone()` 构造 `SessionServiceClient` / `CommandServiceClient` / `GraphEngineServiceClient` / `EventServiceClient` 四个生成客户端。四个客户端共享一个 HTTP/2 连接，避免 connect 四次造成四倍连接与握手开销；`GrpcClient` 的公开方法与返回类型保持不变。

### Decision: TS 高层客户端持有四个 stub

`ThewayGrpcClient` 构造时创建四个生成 service client（同一 authority + credentials），方法按领域委托：状态与会话资源 → `SessionServiceClient`；命令 → `CommandServiceClient`；graph 控制与输出 → `GraphEngineServiceClient`；`streamEvents` → `EventServiceClient`。`close()` 关闭四个 stub。

### Decision: cutover 删除入口文件

cutover 节点删除 `proto/theway_grpc.proto` 与 `sdk/proto/theway_grpc.proto`，同时：
- `crates/theway-transport/build.rs` 与 `crates/theway-probe/build.rs` 改为编译全部领域 proto（读目录收集或显式四个文件 + `health.proto`）。
- `sdk/scripts/generate.sh` 输入改为四个领域 proto + `health.proto`；`rm -f src/generated/*.ts` 清理旧 `theway_grpc.ts`。
- `grpc.rs` 删除旧 trait impl/import 与旧 `ThewayGrpcServer` 注册，改为注册四个 server。
- `sdk/src/index.ts` 删除对 `./generated/theway_grpc.js` 的 star export。

## Risks / Trade-offs

- **旧客户端全断**：wire 路径从 `ThewayGrpc` 变为四个 service，workmate 等外部客户端必须同版本升级。缓解：消息层完全兼容；TS/Rust 高层客户端 API 不变；迁移桥保证仓库内客户端先切完。
- **同名方法歧义（桥接期）**：旧 `ThewayGrpc` 与新 `CommandService` 都有 `send_message` 等 trait 方法。缓解：桥接期只 `use` 旧 trait，新 trait 用全路径实现；cutover 后再切换 `use`。
- **构建入口遗漏**：删除入口文件后，若 build.rs / generate.sh 仍引用它，cargo/npm 生成立即失败。缓解：cutover 节点同 commit 修改 build 输入，验收执行 `cargo check --workspace` 与 `npm run gen && npm run build`。
- **四 client 生命周期**：TS 关闭只关一个 stub 会泄漏连接。缓解：`close()` 显式关闭四个；测试覆盖 close 后 `streamEvents` 失败路径。
- **文档/日志残留旧服务名**：启动日志、probe 文案与 docs 产物需要同步。缓解：cutover 后 grep `theway.grpc.v1.ThewayGrpc` 复核。

## Migration Plan

1. **N1 proto 桥**：四个领域 proto 增加 service，保留旧 `ThewayGrpc`；重新生成 Rust/TS 代码。
2. **N2 并行**：server 注册新服务（保留旧）；transport 高层客户端切四 stub；probe 切生成客户端；TS SDK 切四 stub。
3. **N3 transport 测试**：直接生成客户端测试改走 `SessionServiceClient` / `CommandServiceClient`，并断言四个 service 都可连接。
4. **N4 cutover**：删除旧 service 与 `theway_grpc.proto`，改构建入口与日志/文档引用。
5. **N5 验证**：workspace 全量 test/clippy/fmt + `npm run gen && npm run build` + gRPC 冒烟（GetState / SendMessage / StreamEvents / GraphList）。
6. **N6 收尾**：README/docs 与 issue #60 关闭。

回滚：cutover 前每个节点独立提交可 revert；cutover 提交包含“删除旧 service + 切入口 + 删旧生成文件”，回滚该提交即恢复双服务中间态。

## Open Questions

- 领域 service 名是否要加前缀（例如 `ThewaySessionService`）？本方案先采用无前缀的 `CommandService` / `SessionService` / `GraphEngineService` / `EventService`。
- workmate 外部客户端的升级窗口与 SDK 版本号是否需要在同一 release 内完成；本方案只负责仓库内切完。
