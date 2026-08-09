# Tasks: session-resource-model

## 1. proto 层 (N1)

- [ ] 1.1 `proto/theway_grpc.proto` 新增 `SessionSummary` (session_id/name/cwd/model/created_at/last_activity_at/graph_count/active_graph_count/busy/preview) 与请求/响应消息 (ListSessionsResponse 含 current_session_id)。
- [ ] 1.2 proto 新增 5 个 RPC: `ListSessions` / `CreateSession` / `SwitchSession` / `RenameSession` / `DeleteSession`;新增 `GraphList(GraphListRequest{session_id})`。
- [ ] 1.3 `crates/server/src/proto.rs` 补 `SessionSummary` ↔ wire 转换 (从 theway wire 模型映射)。
- [ ] 1.4 验收: `cargo build --workspace` (生成代码编译通过)。

## 2. core 层 (N2)

- [ ] 2.1 `DagEvent` (runtime/graph_engineering/types.rs) 两个变体补 `session_id: String`;engine 发事件时从 run 取 (engine.rs 中 DagEvent::NodeStatus/RunStatus 构造点)。
- [ ] 2.2 `SubagentJob`/`JobInit` (runtime/subagents/registry.rs) 补 `session_id: Option<String>`;DAG 节点 job 从 run 继承;task tool 启动路径 (app 侧) stamp 当前 session。
- [ ] 2.3 验收: `cargo test -p theway-core` 全绿。

## 3. app 层 (N3)

- [ ] 3.1 `ReplKernel` 加 `replace_harness(Arc<AgentHarness>)`;确认 harness() 使用点无并发问题 (事件循环串行)。
- [ ] 3.2 `AppConfig` 加 `session_factory: Arc<dyn Fn(&str) -> Result<Arc<AgentHarness>> + Send + Sync>`;`main.rs` (cli crate) 提炼 harness 构建为闭包传入。
- [ ] 3.3 `WebCommand` 扩展: `SwitchSession{id}` (CreateSession 的"设为当前"也走它);事件循环 (web_loop.rs run_transport_loop) 加分支: 重建 harness → replace → 更新 App.session_id → 清 feed + 系统行 → 刷新 goal → 发布快照。
- [ ] 3.4 `web_snapshot()` (ui/mod.rs): dags 按 `session_id == Some(current)` 过滤;subagents 按 `job.session_id` 过滤。
- [ ] 3.5 `SessionOps` trait (app 暴露): `list/create/rename/delete` 基于 JsonlSessionRepo + DagEngine 活跃检查;DeleteSession 拒绝活跃 graph 并返回 run id 列表;删除当前 session 后回退最近会话。
- [ ] 3.6 `TransportEndpoints` 加 `session_ops: Arc<dyn SessionOps>` 与 `session_factory` 引用 (供 server 侧同步命令与事件循环切换)。
- [ ] 3.7 验收: `cargo build --workspace` + `cargo test -p theway --features tui --lib` (既有 3 个环境失败除外)。

## 4. server 层 (N4)

- [ ] 4.1 `GrpcState` 加 `session_ops: Arc<dyn SessionOps>`;实现 5 个 session RPC + `GraphList` (从 dag_engine 按 session_id 过滤)。
- [ ] 4.2 `HttpState` 加 `session_ops`;HTTP 路由: `GET /sessions` / `POST /sessions` / `POST /sessions/{id}/switch` / `PATCH /sessions/{id}` (rename) / `DELETE /sessions/{id}`。
- [ ] 4.3 测试: gRPC session RPC 测试 (ListSessions→Create→Switch→GetState 反映新 session→Rename→Delete + 删除保护);HTTP /sessions 路由测试;web_snapshot 过滤测试。
- [ ] 4.4 验收: `cargo test -p theway-server` 全绿 (含新增)。

## 5. 验证 (N5)

- [ ] 5.1 `cargo test --workspace --features tui --no-fail-fast` (记录既有环境失败,新增失败必须为 0)。
- [ ] 5.2 `cargo clippy --workspace --all-targets --features tui -- -D warnings` + `cargo fmt --all --check`。
- [ ] 5.3 冒烟: 起 `--grpc`/`--web`,ListSessions 返回 ≥1,CreateSession 后 GetState.session_id 变化,GraphList 空/按会话过滤。
- [ ] 5.4 提交全部改动。
