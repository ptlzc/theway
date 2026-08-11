## ADDED Requirements

### Requirement: run 级 USD 成本聚合

`multiagent` 层 SHALL 聚合单个 DAG run 下所有节点的 USD 成本:每个节点完成时从 harness `CostTracker` 取 `CostSnapshot`,累计到 run 记录;run 状态面(graph/types.rs 运行时状态)SHALL 提供 `run_cost_usd: Option<f64>`;registry 的 job 记录与 `AgentJobEvent::Completed` SHALL 增加 `cost_usd` 字段。

#### Scenario: run 完成后查询总成本

- **WHEN** 一个含多个节点的 DAG run 全部完成
- **THEN** run 记录与每个节点 job 记录均带 USD 成本,UI/`dag_inspect` 可展示 run 总成本与单节点成本

#### Scenario: 节点失败的成本保留

- **WHEN** 某节点失败但已产生 tokens
- **THEN** 该节点已发生成本仍计入 run 累计(成本是已发生事实,不因失败回滚)

### Requirement: run 级预算上限

`AgentRunOptions` SHALL 提供 run 级 `budget_cap_usd`;engine 在每个节点完成时累计已发生成本,超限后:后续未启动节点不启动,运行中节点照常完成(不中途 kill),run 标记预算受限状态(复用 `DagStatus` 语义或新增状态)。assembly 的 per-harness `budget_cap_usd`(单会话场景)SHALL 保留不变。

#### Scenario: 预算超限中止后续节点

- **WHEN** run 级 `budget_cap_usd` 为 $2,已完成的节点累计成本达 $2.1
- **THEN** 后续 ready/pending 节点不启动,运行中节点正常完成,run 结束状态标记预算受限

#### Scenario: 单会话预算不受影响

- **WHEN** CLI 主会话使用 per-harness `budget_cap_usd`
- **THEN** 行为与现状一致(在 prompt/continue 前检查,超限拒绝启动新 turn)
