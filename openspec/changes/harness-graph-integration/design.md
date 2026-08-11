# Design: harness-graph-integration

## Context

graph(DAG 编排)是 harness 的核心能力:每个节点由 `multiagent/runner.rs::run_agent` 现场组装一个 `AgentHarness`(内存 session)执行,节点输出作为记录存入 engine(`engine/run.rs:155-157`),orchestrator 事后经 `dag_inspect`/`dag_wait` 拉取。**graph 是编排驱动**(事件驱动调度,无数据流插值),不是输出驱动 — 本设计的所有改造以此为前提。

依赖方向正确且单向:`multiagent → agent::assembly`(assembly 不知道 multiagent 的存在)。问题集中在接缝:assembly 的 API/事件/成本模型是"单会话"形状,graph 是"多节点运行"形状,中间缺一层适配。

现状证据:

- **P2 双通道**: `assembly.rs:541-611` `run_turn_with_continuation` 内联 await `OnTurnEndHook`(返回 `TurnEndDecision`,带 continuation 上限,为 goal 设计);`runner.rs:143-176` 需要轻量 per-turn 观测(text + 累计 tokens 喂 engine idle watchdog / live preview),契约对不上 → 裸订阅 `AgentEvent::MessageEnd` + `Arc<Mutex<String>>` final_text 收集器,并把 `on_turn_end: None` 传给 harness。graph 模式下 assembly 的 hook 机制完全闲置。
- **P3 执行模型**: `assembly.rs:611` hook 在 prompt 调用栈内同步 await;`goal.rs` 的 hook 内部调 `run_agent` 跑 evaluator 节点(父 turn 被阻塞);`node_launcher.rs` 是 `tokio::spawn` detached 调度。两种执行模型并存,goal 自环可用,通用 graph 无法挂内联 hook。
- **P4 装配外置**: `runner.rs:88-241` 手工编排 registry 注册 + 控制句柄 + metrics listener + final-text 收集 + cancel watcher + timeout;`assembly.rs` 模块文档自称 "opinionated assembly around the bare Agent",但 graph 实际使用的子运行形状由 runner 组装 — 两个 assembly 并存。
- **P5 身份缺失**: `AgentHarnessOptions::new(model, session)` 强制 Session(`runner.rs:120-122` 每节点 `MemorySessionStorage`);`set_model`/`set_thinking_level` 走 session 审计记录(assembly.rs:280-300);harness 不知道 `run_id`/`node_id`,`session_id` 只是 registry 元数据字符串 — HarnessEvent/会话无法与 graph run 关联。
- **P6 事件碎片**: 节点运行产生四套面:harness 的 `AgentEvent`(裸订阅)+ `AgentJobEvent`(registry/events.rs: Started/Output/Metrics/Completed)+ `HarnessEvent`(会话生命周期,节点运行中几乎不触发)+ engine 状态 + dag_tools 文本渲染。观测者需同时理解多套面,无单一事实源。
- **P7 成本孤立**: `assembly.rs:70/160/509-659` `budget_cap_usd` + `CostTracker` 每 harness 一份;runner 每节点新 tracker;registry 只聚合 token(`registry/mod.rs:81-82`),无 USD — 多节点 run 无法做 run 级预算,UI 无 run 总成本。

## Goals / Non-Goals

**Goals:**

- 消除 runner 对 `AgentEvent` 的裸订阅,per-turn 观测走 harness 一等通道(P2)。
- `prompt()` 返回运行摘要(最终文本 + input/output tokens + 状态),runner 的 final_text 收集器删除(P1 残余 + P4 部分)。
- AgentHarness 携带 graph 身份(run_id/node_id),HarnessEvent 可溯源(P5)。
- Session 支持 ephemeral 模式,一次性节点不强制会话持久化机制(P5)。
- 事件面职责边界明确:HarnessEvent(低频生命周期)与 AgentJobEvent(高频作业)归属单一化,节点事件可溯源(P6)。
- run 级 USD 成本聚合 + run 级预算上限(P7)。
- 文档化执行模型边界:内联 hook(goal)vs detached 引擎(graph)(P3)。

**Non-Goals:**

- 不改变 `OnTurnEndHook` 契约与 goal 模式行为(P3 只文档化边界,不重构 hook)。
- 不把 graph engine 搬进 assembly(依赖方向保持 multiagent → assembly 单向;graph 是"harness 之上"的编排层,assembly 是"叶子执行单元" — 本次只修接缝)。
- 不做引擎内数据流插值(节点输出仍为记录,不喂下游 task)。
- 不引入新依赖。

## Decisions

### D1: TurnObserver — 一等 per-turn 观测通道(P2)

`AgentHarnessOptions` 增加 `turn_observer: Option<TurnObserver>`:

```rust
pub type TurnObserver = Arc<dyn Fn(TurnObservation) + Send + Sync>;
pub struct TurnObservation {
    pub turn_index: u32,          // 0-based, 本次 prompt/continue 周期内
    pub text: String,             // 本 turn 最终 assistant 文本
    pub input_tokens: u64,        // 累计
    pub output_tokens: u64,       // 累计
}
```

`run_turn_with_continuation` 在每次 `agent.prompt/continue_` 返回后、`finish_persisted_run` 之后触发(与 `OnTurnEndHook` 触发点相邻但独立)。语义:**被动观测** — 不返回决策、不影响执行、不参与 continuation 计数。

**为什么不用现有 OnTurnEndHook**: 契约不同 — hook 返回 `TurnEndDecision`(控制面,goal 用它驱动 continuation)、带 cap、内联阻塞;observer 只需"看一眼"(idle watchdog 刷新 + 实时 preview),二者触发点虽近但职责正交。合并会让 hook 的 `TurnEndAction::Noop` 语义和 observer 的纯观测语义互相污染。

**备选**: 复用 `AgentEvent::MessageEnd` + 让 runner 继续裸订阅(现状)— 否:订阅是 harness 内部机制,外部订阅者依赖事件形状(脆弱),且无法拿到 token 汇总(还要额外 `sub.cost()`)。TurnObserver 是 harness 稳定契约。

### D2: prompt 返回 RunSummary(P1 残余 + P4 部分)

```rust
pub struct RunSummary {
    pub text: String,             // 最终 assistant 文本(空 = 无文本输出)
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub interrupted: bool,        // TurnInterrupted 提前结束
}
```

`prompt` / `prompt_with_images` / `continue_` / `prompt_from_template` 返回 `Result<RunSummary, AgentRunError>`。**BREAKING**: 调用方需适配(server 的 session_factory/trigger_engine/UI 与 runner)。

**为什么**: runner 的 final_text 收集器 + post-hoc `sub.cost()` 全部消失;run 摘要是"运行完成"这一事实的自然携带物。**注意**: 这不改变 graph 的编排语义 — engine 依然事件驱动,`dag_wait` 依然事后拉取;RunSummary 只是让 runner 记录节点产出时不再 hack。

**备选**: 让 runner 用 `sub.agent().state().messages` post-hoc 提取(零 API 改动)— 可行但每个调用方都要实现提取逻辑(正是 P4 抱怨的"装配外置");返回摘要是一等 API,收口一次。

### D3: graph 身份 + ephemeral session(P5)

- `AgentHarnessOptions` 增加 `run_id: Option<String>` / `node_id: Option<String>`(纯身份,不进 session)。
- `HarnessEvent` 全部变体增加可选 `run_id`/`node_id` 字段(或加一个 `GraphContext` 字段)。**BREAKING**(事件消费者需适配,server ui/listener 等)。
- Session 改为可选:保留 `AgentHarnessOptions::new(model, session)`(兼容),新增 `AgentHarnessOptions::ephemeral(model)`(无 session)与 `session: Option<Session>` 内部表示;无 session 时:
  - `set_model` / `set_thinking_level` 只改 `agent.state()`(跳过 append 审计);
  - `move_to` / `rehydrate_from_session` / `reload_skills_from_disk` 返回明确错误(或仅在存在 session 时可用);
  - compaction 仍可用(基于内存 transcript)。

**为什么**: 一次性节点不需要 session 机制(append/branch/rehydrate 全是死重);身份字段让 HarnessEvent/成本可溯源到 run/node,不再靠 registry 元数据字符串间接关联。

**备选**: 保持 Session 必选,身份只放 registry — 否:事件溯源是 P6 的前提,harness 自身的事件不带身份就无法在 assembly 边界做聚合/审计。

### D4: 事件面职责边界(P6)

文档化 + 少量代码:

- `HarnessEvent`(assembly)= **低频生命周期面**:会话开始/压缩/分支/技能重载/turn 结束决策 — 承载 P5 的身份字段后,节点级事件可溯源。
- `AgentJobEvent`(registry)= **高频作业面**:Started/Output/Metrics/Completed,grpc 传输 — 保持现状。
- 新增映射:`run_agent` 在注册 job 时,把 `HarnessEvent`(经 TurnObserver 或 subscribe_harness)桥接为 `AgentJobEvent::Completed` 的补充来源(可选,若 UI 需要)。
- 模块文档(`assembly.rs` 头部 + `multiagent/mod.rs`)写明四套面各自的归属与消费方。

**为什么**: 不合并事件类型(频率/传输需求不同),但职责边界必须有文档 + 身份可溯源,否则"graph 核心化"后观测者依然要猜。

### D5: run 级成本(P7)

**现状缺口(本设计新增前置)**: `Usage::cost` 目前**无运行时计算** — `core/src/agent/cost.rs:14-15` 注释声称 provider 按目录价格表填充,但全仓库无一处 `tokens × ModelCost / 1e6` 的乘法(anthropic `update_usage` 只填 token 数),`Usage::cost` 运行时恒为 0。后果:`/cost` 美元显示为 0、`budget_cap_usd` 检查(`assembly.rs:678`)永不触发。

**D5a(前置)**: llm-provider 在解析每条 assistant usage 时计算 `Usage::cost`(input/output/cache_read/cache_write 各自 `tokens × model.cost.<kind> / 1_000_000`,`total` 为四者之和;provider 响应自带 cost 字段时优先解析)。core 的 `cost.rs` 注释修正为如实描述。

**D5b(聚合)**: `runner.rs` 聚合本 run 各节点 USD 成本:每个节点完成时从 harness `CostTracker` 取 `CostSnapshot`,累计到 run 记录;run 状态面(graph/types.rs 运行时状态)提供 `run_cost_usd: Option<f64>`;registry 的 job 记录与 `AgentJobEvent::Completed` 增加 `cost_usd` 字段。

**D5c(预算)**: `AgentRunOptions` 增加 run 级 `budget_cap_usd`;engine 在每个节点完成时累计已发生成本,超限后:后续未启动节点不启动,运行中节点照常完成(不中途 kill),run 标记预算受限状态(复用 `DagStatus` 语义或新增状态)。assembly 的 per-harness `budget_cap_usd`(单会话场景)保留,依赖 D5a 生效。

### D6: 执行模型边界文档化(P3)

`assembly.rs` 模块文档与 `OnTurnEndHook` doc 增加一节:

> `OnTurnEndHook` 是**内联控制面**:在 prompt 调用栈内 await,返回决策驱动 continuation — 适用 goal 单自环。通用编排(graph)走 detached 事件面(DagEngine + registry),不要试图把多节点编排挂到该 hook 上。演进路径:未来若需要"非阻塞 continuation",新增 `OnRunEndEvent`(事件面)而非扩展 hook 契约。

不改代码,只改文档。**为什么**: 现状 goal 可用,强行统一执行模型是过度设计;文档化边界防止未来误用(把 graph 挂 hook 上会阻塞父 turn)。

## Risks / Trade-offs

- [BREAKING API(proposal.md 已标)] `prompt()` 返回类型 + HarnessEvent 字段变化 → 一次性适配(server 调用方约 10 处 + runner),CI 全量测试兜底。
- [TurnObserver 与 OnTurnEndHook 触发点相邻] 两个回调顺序依赖 → 文档明确"observer 先于 hook 触发"(观测先于决策),测试固定该顺序。
- [ephemeral session 路径未全覆盖] `move_to`/`rehydrate` 在无 session 时行为需定义 → 返回类型化错误,server 的 resume/分支功能只在有 session 时启用(现有调用方均带 session,不受影响)。
- [run 级预算的竞态] 并行节点同时完成 → 累计用 engine 内部 Mutex(现有状态已 Mutex 化),超限判定在调度点(on_node_completed)串行执行,天然无竞态。
- [事件面桥接过度] D4 的 HarnessEvent→AgentJobEvent 桥接若不需要可不做 → 标记为可选任务,默认只做文档 + 身份。

## Migration Plan

1. 先落 D1+D2(TurnObserver + RunSummary,核心收益),同步适配 runner 与 server 调用方。
2. 再落 D3(身份 + ephemeral session),HarnessEvent 字段变化随 D3 一次适配。
3. 再落 D5(run 级成本),engine/registry 字段扩展。
4. D4 文档 + 可选桥接;D6 文档。
5. 每步独立提交(Conventional Commits,按 core/server 分笔),`cargo test --workspace --no-fail-fast` + clippy + fmt 全绿后推送。

回滚:各步独立提交,可逐 commit revert;API 变化集中在 core,server 同步适配。

## Open Questions

- D4 的 HarnessEvent→AgentJobEvent 桥接是否本期需要?(默认否,先文档 + 身份)
- run 级预算超限后的语义:标记 run `BudgetLimited` 并取消后续节点,还是允许已完成节点保留?(默认前者,复用 dag_cancel 语义)
- `RunSummary.text` 是否需要与 `dag_inspect` 相同的 8KB tail 截断?(默认不截断,截断是工具层展示职责)
