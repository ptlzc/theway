## Why

协议层没有 session 资源:gRPC/HTTP 只暴露"当前进程绑定的那一个会话" (GetState/SendMessage),而引擎与持久化层早已按 session 组织 (DagRun.session_id、按 session 的状态文件、checkpoint/restore 的 session 挂载键)。`web_snapshot()` 甚至把引擎里**所有** DAG run 全量塞进 `SessionState.dags` (与 proto 注释 "mounts under the session" 自相矛盾)。workmate 浏览器端因此无法列出/创建/切换会话,也无法按会话查看其 graph。

## What Changes

- **proto**: 新增 `SessionSummary` (session_id/name/cwd/model/created_at/last_activity_at/graph_count/active_graph_count/busy/preview) 与 5 个 session RPC — `ListSessions` / `CreateSession` / `SwitchSession` / `RenameSession` / `DeleteSession`;新增 `GraphList(session_id)` 按会话列出 graph run。
- **状态面修正**: `SessionState.dags` / `subagents` 按当前 session 过滤 (修全量 bug)。
- **core**: `DagEvent` 补 `session_id` (事件面可按会话路由);`SubagentJob` 补 `session_id` (register 时从 run 或调用方 stamp)。
- **app (theway)**: `ReplKernel` 的 harness 改为可替换 (切换 = 进程内 resume,复用 `Session::resume`);`AppConfig` 增加 session factory;`WebCommand` 扩展 session 命令 (`SwitchSession` 等),在事件循环内序列化执行;`TransportEndpoints` 暴露 session 操作面。
- **server (theway-server)**: `GrpcState`/`HttpState` 实现 session RPC + HTTP `/sessions` 路由 (GET 列表 / POST 创建 / POST {id}/switch / PATCH 重命名 / DELETE 删除)。
- **DeleteSession 语义**: 有活跃 graph 的 session 拒绝删除,返回运行中 run 列表。

## Capabilities

### New Capabilities

- `session-resource-model`: 协议层 session 作为顶层资源 — 生命周期 RPC、graph 按 session 挂载、删除保护、事件面 session 维度。

### Modified Capabilities

- 无。

## Impact

- **proto**: `proto/theway_grpc.proto` 增加消息与 RPC;build.rs (server) 重新生成;TS 侧 (workmate) 协议同步 (后续)。
- **core**: `runtime/graph_engineering/types.rs` (DagEvent)、`runtime/subagents/registry.rs` (SubagentJob/JobInit)。
- **app**: `ui/kernel.rs` (harness 可替换)、`ui/web_loop.rs` (WebCommand + 事件循环分支 + TransportEndpoints)、`ui/mod.rs` (App session 方法 + web_snapshot 过滤)、`session/mod.rs` (复用)。
- **server**: `grpc.rs` (新 RPC + GrpcState 扩展)、`http.rs` (新路由 + HttpState)、`proto.rs` (新 wire 转换)。
- **不改变**: 现有 RPC 端点语义 (`GetState`/`SendMessage`/graph RPC 兼容);包名与 crate 边界 (沿用重构后结构);automations (triggers/cron) 保持进程级,per-session 自动化明确不做。
