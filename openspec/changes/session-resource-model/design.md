# Design: session-resource-model

## Context

重构后分层:core (引擎, 含 DagEngine/SubagentJobRegistry/Session) → app (theway: App 事件循环 + kernel + wire) → server (theway-server: gRPC/HTTP/WS)。现状:

- `GrpcState.session_id: String` 单值,`TransportEndpoints.session_id` 单值 — 进程绑定一个会话,wire 无会话资源。
- `App.session_id: String` 启动固定;`ReplKernel.harness: Arc<AgentHarness>` 不可替换;`AgentHarness.session: Session` 是 pub 字段 (core, agent_harness.rs:811)。
- `web_snapshot()` 的 dags = `dag_engine.list_runs()` 全量 (未按 session 过滤);`SubagentJob` 无 session_id;`DagEvent` 无 session_id。
- 会话管理能力已就绪:app/session/mod.rs 有 `create/resume/list_entries/delete_by_id`,core `JsonlSessionRepo` 支持全生命周期;`DagRun.session_id` 已存在且 dag_* 工具强制 session 归属;checkpoint/restore 已按 session 组织。

约束:不改变现有 RPC 语义 (GetState/SendMessage/graph RPC 兼容);automations (triggers/cron) 保持进程级;沿用 crate 边界。

## Goals / Non-Goals

**Goals:**

- 协议层 session 成为顶层资源 (生命周期 RPC + HTTP 路由)。
- graph 明确挂载在 session 下:状态快照按当前 session 过滤,事件带 session 维度。
- 运行时支持进程内切换 session (resume 语义),切换不中断进程级 DAG 引擎。
- 删除保护 (活跃 graph 拒绝删除)。

**Non-Goals:**

- 不做**连接级**并发会话绑定 (多 gRPC 连接各自绑定不同 session 的运行时隔离) — 协议 shape 通过显式 session_id 参数兼容,运行时切换为进程级 current,连接级留后续。
- 不做 per-session automations (triggers/cron 保持进程级作用于当前 session)。
- 不做会话历史/消息级 API (ListSessions 只返回摘要)。
- 不迁移 TS 侧 (workmate) — 协议先落地 Rust 侧,TS 同步另行安排。

## Decisions

### Decision: RPC 显式 session_id + 进程级 current

session 操作分两类:
- **同步查询/变更** (不涉及 App 事件循环): `ListSessions` / `CreateSession` / `RenameSession` / `DeleteSession` — 直接在 service 层调用 repo (经 `SessionOps` trait,app 实现)。
- **切换** `SwitchSession`: 必须改 App 运行时状态 (kernel 的 harness) → 走 `WebCommand::SwitchSession` 进事件循环序列化执行 (与 prompt/abort 同一串行槽,不竞争)。

`CreateSession` 的"设为当前"同样走事件循环 (创建可在 service 层做,切当前走命令)。

理由:切换涉及 App 内部状态 (kernel/feed/busy/session_id),只有事件循环能安全变更;同步命令复用现有 `WebCommand` 通道,不需要新并发模型。

替代方案:连接级绑定 (tonic interceptor + per-conn state) — 协议兼容但运行时复杂,本次无多 tab 并发会话的真实需求,列为后续。

### Decision: kernel harness 可替换 (RwLock 包装)

`ReplKernel.harness: Arc<AgentHarness>` → 保持字段,新增替换能力:`ReplKernel::replace_harness(Arc<AgentHarness>)`。App 增加 `session_factory: Arc<dyn Fn(&str) -> Result<Arc<AgentHarness>>>` (AppConfig 提供,main.rs 构造 — 它已有 800 行 harness 构建逻辑,提炼成闭包)。

切换流程 (事件循环内):
1. factory(session_id) → 新 AgentHarness (resume 语义)
2. `kernel.replace_harness(new)`;更新 `App.session_id`
3. 重置 per-session 状态:busy/queued_turns/feed (保留 feed 历史或清空?— 清空并注入一条 "switched to session X" 系统行,feed 是 UI 瞬态)
4. 刷新 goal 状态;发布快照

理由:`AgentHarness.session` 虽 pub,但 Arc 不可变 + harness 状态 (skills 缓存/监听器) 与 session 强耦合,原地换 session 风险高;factory 重建是 CLI `--resume-id` 的进程内版,复用成熟路径。

风险:factory 需要 main.rs 提炼 — 若构建逻辑引用大量局部变量,闭包捕获面大。缓解:factory 捕获必要的 Arc (repo/mcp/tools 注册) — 由 apphandle 节点评估,必要时把 factory 类型放宽为 `Arc<dyn Fn(&str) -> Result<Arc<AgentHarness>> + Send + Sync>`。

### Decision: 状态快照按 session 过滤 (修全量 bug)

`web_snapshot()` 的 dags 改为 `list_runs().filter(|r| r.session_id == Some(current))`;subagents 改为按 `job.session_id` 过滤 (SubagentJob 补字段后)。任务工具 (task tool) 直接启动的 job 归属调用 session (stamp 为当前),DAG 节点 job 从 run 继承。

### Decision: SubagentJob / DagEvent 补 session_id

- `SubagentJob`: `JobInit` 加 `session_id: Option<String>`;DAG 节点启动时从 run 继承;task tool 启动时 stamp 当前 session。
- `DagEvent`: 两个变体 (NodeStatus/RunStatus) 加 `session_id: String`;engine 发事件时从 run 取。

### Decision: SessionOps trait 放 app,server 依赖之

`theway` 暴露 `pub trait SessionOps` (list/create/rename/delete + 检查活跃 graph) 与实现 (基于 JsonlSessionRepo + DagEngine);`GrpcState`/`HttpState` 持有 `Arc<dyn SessionOps>`。server 不直接碰 repo (保持"server 只对公开接口编程"的边界)。

### Decision: DeleteSession 拒绝活跃 graph

删除前检查 `dag_engine.list_runs()` 中该 session 的 running/pending run;有则返回错误 + run id 列表 (spec: 拒绝 + 列出)。删除成功后若删的是当前 session,回退到最近会话 (list_entries 最新) 或置空。

## Risks / Trade-offs

- [SwitchSession 重建 harness 开销大 (skills/triggers 重新加载)] → 与 CLI resume 路径同构,可接受;首次切换后观察,后续可做 harness 池缓存。
- [factory 闭包捕获面大 (main.rs 提炼)] → apphandle 节点先评估 main.rs 构建逻辑,把依赖收敛为 Arc 集合;必要时 factory 签名放宽。
- [事件循环内切换与进行中 turn 竞争] → SwitchSession 命令在事件循环串行槽处理;turn 进行中先 abort (与既有 Abort 语义一致)。
- [subagents 过滤改变现有 UI 行为 (任务工具 job 不再全量显示)] → 符合 spec 意图 (subagents 挂 session 下);测试更新。
- [proto 生成代码变更波及 grpc.rs 测试] → server 测试随 RPC 增加同步扩展,现有测试保持绿。

## Migration Plan

1. **N1 proto**: `theway_grpc.proto` 加 SessionSummary + 5 RPC + GraphList;`server` build.rs 重新生成;`proto.rs` 补 wire 转换。验收: `cargo build --workspace`。
2. **N2 core**: `DagEvent.session_id` + `SubagentJob.session_id`/`JobInit` 字段 + engine/registry 落值。验收: core 测试全绿。
3. **N3 app**: kernel replace_harness + AppConfig.session_factory + WebCommand session 命令 + 事件循环分支 + `SessionOps` trait/impl + web_snapshot 过滤。验收: `cargo test -p theway --features tui --lib` (既有 3 个环境失败除外)。
4. **N4 server**: GrpcState/HttpState 实现 session RPC + `/sessions` 路由 + wire 转换接线。验收: server 测试全绿 (新增 session RPC 测试)。
5. **N5 verify**: 全量测试 + clippy/fmt + gRPC/HTTP 冒烟 (ListSessions → 创建 → 切换 → 过滤验证)。

回滚: 每节点独立 commit;proto 兼容 (新 RPC 不影响旧端点),回滚仅 revert 即可。

## Open Questions

- CreateSession 的 cwd/初始 model 参数:本版从当前进程继承 (不暴露参数),后续按需扩展。
- SwitchSession 后 feed 历史:清空 (UI 瞬态) 但保留会话内 transcript 于 JSONL — 本版采用清空 + 系统提示行。
