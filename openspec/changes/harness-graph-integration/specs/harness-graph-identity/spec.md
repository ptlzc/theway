## ADDED Requirements

### Requirement: AgentHarness 携带 graph 身份

`AgentHarnessOptions` SHALL 提供 `run_id` / `node_id` 可选身份字段(纯元数据,不写入 session);`HarnessEvent` 的所有变体 SHALL 携带对应的可选 `run_id` / `node_id` 上下文,使 harness 发出的事件可溯源到 graph run/node。**BREAKING**: 事件消费者需适配新字段。

#### Scenario: 节点事件溯源

- **WHEN** DAG 节点 harness 发出 `HarnessEvent::Compaction` 或 `HarnessEvent::TurnEnded`
- **THEN** 事件携带该节点的 `run_id` / `node_id`,观测者无需依赖 registry 元数据字符串即可定位归属 run

#### Scenario: 无身份的普通会话

- **WHEN** CLI 主会话的 harness 未配置身份字段
- **THEN** 事件中 `run_id` / `node_id` 为 `None`,现有行为不变

### Requirement: ephemeral session 模式

`AgentHarnessOptions` SHALL 支持无 Session 构造(`ephemeral(model)` 构造器;`session` 字段可选)。无 session 时:`set_model` / `set_thinking_level` SHALL 仅修改内存 agent state(跳过 session 审计记录);`move_to` / `rehydrate_from_session` / `reload_skills_from_disk` SHALL 返回类型化错误(需 session 的功能不可用);compaction 与 turn 观测不受影响(基于内存 transcript)。

#### Scenario: 一次性节点不建 session

- **WHEN** runner 为 DAG 节点构造 ephemeral harness
- **THEN** 不创建 `MemorySessionStorage`,节点运行不产生任何 session append 调用

#### Scenario: 无 session 时调用 session 依赖 API

- **WHEN** ephemeral harness 上调用 `move_to`
- **THEN** 返回类型化错误(如 `SessionError::NoSession`),不 panic、不静默失败

#### Scenario: 兼容既有构造

- **WHEN** 调用方使用 `AgentHarnessOptions::new(model, session)`(带 session)
- **THEN** 行为与现状完全一致(审计记录、分支、resume 全部可用)
