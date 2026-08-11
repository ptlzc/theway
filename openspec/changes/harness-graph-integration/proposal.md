## Why

graph(DAG 编排)已是 harness 的核心能力:每个节点 = `multiagent/runner.rs::run_agent` 现场组装一个 `AgentHarness`(内存 session)执行。但 `agent/assembly.rs`(AgentHarness)与 graph 层的接缝存在六个设计问题(P2–P7):turn 结束观测双通道、执行模型张力、子运行装配外置、Session 强依赖 + 无 graph 身份、事件面碎片化、成本 per-harness 无跨节点聚合。graph 核心化后这些接缝问题开始产生实际成本(runner 裸订阅 AgentEvent、节点事件无法溯源、run 级预算缺失)。

## What Changes

- **P2 turn 观测统一**: `AgentHarness` 增加一等 per-turn 观测通道 `TurnObserver`(文本 + 累计 tokens + turn 序号),`run_turn_with_continuation` 每 turn 结束触发;`multiagent/runner.rs` 的裸 `AgentEvent::MessageEnd` 订阅 + `final_text` 收集器删除,改走 harness 内置通道。`prompt()` 系列返回运行摘要(最终文本 + tokens + 状态),消除 runner 的 post-hoc 提取 hack。
- **P3 执行模型边界**: 设计层明确"`OnTurnEndHook` = 内联控制面(goal 单自环)、graph engine = detached 事件面(DAG 调度)"的职责边界,写入模块文档;`OnTurnEndHook` 契约本轮不动(goal 自环可用,通用 graph 挂载是后续演进,设计文档记录演进路径)。
- **P4 子运行装配收口**: runner 的组合逻辑(text/tokens/状态摘要、cancel 级联)下沉为 assembly 可复用能力(经 P2 的 run 摘要 + 现有 abort 契约),runner 只保留 registry/engine 专属逻辑(注册、控制句柄、metrics)。
- **P5 graph 身份 + ephemeral session**: `AgentHarnessOptions` 增加 `run_id` / `node_id` 身份字段;`HarnessEvent` 携带身份;Session 支持 ephemeral 模式(`session: Option<Session>`,无 session 时 `set_model`/`set_thinking_level` 仅改内存状态)。
- **P6 事件面归属**: 定义 `HarnessEvent`(会话生命周期)与 `AgentJobEvent`(高频作业事件,registry)的职责边界并文档化;`HarnessEvent` 增加 graph 生命周期变体(节点开始/结束),节点事件可溯源到 run/node。
- **P7 run 级成本**: runner 汇总节点 `CostSnapshot` 到 run;`budget_cap_usd` 支持 run 级(节点完成后累计,超限中止后续节点);registry/engine 增加 USD 字段。

## Capabilities

### New Capabilities

- `harness-turn-observation`: 统一 turn 结束观测 — assembly 内置 per-turn 观察者 + `prompt()` 运行摘要,替代 multiagent 层的裸订阅与文本收集。
- `harness-graph-identity`: AgentHarness 携带 run/node 身份,HarnessEvent 可溯源;Session 支持 ephemeral 模式。
- `harness-event-ownership`: 事件面职责边界 — HarnessEvent(生命周期/低频)与 AgentJobEvent(高频作业)的归属与映射,节点级事件可溯源。
- `run-level-cost`: 跨节点 USD 成本聚合与 run 级预算上限。

### Modified Capabilities

- 无(openspec/specs/ 无已归档规范)。

## Impact

- **代码**: `crates/core/src/agent/assembly.rs`(options + run_turn_with_continuation + HarnessEvent + prompt 签名)、`crates/core/src/multiagent/runner.rs`(删收集器、改 TurnObserver)、`crates/core/src/multiagent/registry/*`(USD 字段)、`crates/core/src/multiagent/graph/engine/*`(run 级预算)、`crates/server/src/*`(装配处传身份/预算,若有)。
- **API**: `AgentHarnessOptions` 增字段(非破坏);`prompt()` 返回类型 `Result<(), AgentRunError>` → `Result<RunSummary, AgentRunError>`(**BREAKING**,影响 server 调用方与 runner)。
- **行为**: 节点/run 可溯源、run 级预算生效;turn 观测语义不变(观测者被动,不改变执行)。
- **不改变**: `OnTurnEndHook` 契约(goal 模式)、graph engine 调度语义、依赖方向(multiagent → assembly 单向)。
