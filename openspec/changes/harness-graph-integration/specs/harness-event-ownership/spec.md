## ADDED Requirements

### Requirement: 事件面职责边界文档化

`assembly.rs` 模块文档与 `multiagent/mod.rs` 文档 SHALL 明确四套事件面的归属与消费方:`HarnessEvent`(低频生命周期面:会话开始/压缩/分支/技能重载/turn 结束决策)、`AgentEvent`(harness 内部 per-turn 事件,外部订阅者应改用 TurnObserver)、`AgentJobEvent`(高频作业面:Started/Output/Metrics/Completed,registry,grpc 传输)、engine 状态 + dag_tools 文本渲染(编排面)。文档 SHALL 写明:外部观测节点运行应优先使用 TurnObserver / AgentJobEvent,而非裸订阅 `AgentEvent`。

#### Scenario: 新观测者定位事件来源

- **WHEN** 开发者需要观测节点运行(实时输出/token/完成状态)
- **THEN** 文档指明使用 `TurnObserver`(per-turn)与 `AgentJobEvent`(高频作业面),`AgentEvent` 仅为 harness 内部机制

#### Scenario: 生命周期事件溯源

- **WHEN** 需要确认某次压缩或分支发生在哪个 run/node
- **THEN** 经 `HarnessEvent` 携带的身份字段(run_id/node_id)直接定位,无需跨面拼装

### Requirement: 执行模型边界文档化

`OnTurnEndHook` 的文档 SHALL 明确其为**内联控制面**(在 prompt 调用栈内 await,返回决策驱动 continuation,适用 goal 单自环);通用编排(graph)走 detached 事件面(DagEngine + registry)。文档 SHALL 记录演进路径:未来若需要非阻塞 continuation,新增 `OnRunEndEvent` 事件面,而非扩展 hook 契约。本轮不改 hook 行为。

#### Scenario: 避免误用 hook 挂编排

- **WHEN** 开发者考虑把多节点编排挂到 `OnTurnEndHook`
- **THEN** 文档明确该 hook 是内联阻塞控制面,通用 graph 应使用 DagEngine,防止父 turn 被编排阻塞

#### Scenario: goal 行为不变

- **WHEN** goal 模式继续使用 `OnTurnEndHook`
- **THEN** 行为与现状完全一致(continuation 决策、cap、审计记录不变)
