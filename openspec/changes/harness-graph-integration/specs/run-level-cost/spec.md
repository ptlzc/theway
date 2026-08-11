## ADDED Requirements

### Requirement: core 成本统计中性化

core(含 llm-provider)SHALL 只持有中性统计:token 数(input/output/cache)、char 数(registry 作业指标)。`CostTracker`/`CostSnapshot` SHALL 移除 USD 聚合(`total_cost()` 与成本行删除);`AgentHarnessOptions::budget_cap_usd` 与 `check_budget_cap` SHALL 删除;llm-provider SHALL NOT 填充 `Usage::cost`(保持 default 0,`ModelCost` 价格表保留为目录数据供 server 消费)。

#### Scenario: 引擎不含美元逻辑

- **WHEN** 检查 core 的 CostTracker / assembly / llm-provider 代码
- **THEN** 无 USD 计算、无 `budget_cap_usd` 字段、无价格表换算逻辑;`Usage::cost` 恒为 default 0

#### Scenario: 模型目录价格仍可用

- **WHEN** server 需要换算美元
- **THEN** 仍可从 theway-llm-provider 模型目录读取 `ModelCost`(每百万 token 美元价格),价格数据未随计算逻辑移除

### Requirement: server USD 换算能力

server SHALL 提供 token → USD 换算(读 llm-provider 模型目录价格,input/output/cache 各自 `tokens × 单价 / 1_000_000` 合计);`/cost` 命令、UI 成本展示、debug 输出 SHALL 经此换算显示真实美元(消费 core 的 token 统计)。

#### Scenario: /cost 显示真实美元

- **WHEN** 会话累计 input=1M tokens、output=0.5M tokens,模型目录 input=$3/M、output=$15/M
- **THEN** `/cost` 显示 `$10.5000`(而非恒为 $0.0000)

#### Scenario: 目录价格为零的模型

- **WHEN** 模型目录未配置价格(`ModelCost` 全 0)
- **THEN** server 换算结果为 $0,不 panic,展示 `$0.0000`

### Requirement: 预算在 server 工具层

per-harness `budget_cap_usd` SHALL 从 core 移除(引擎不做预算判定);run 级预算上限 SHALL 由 server 工具层实现:累计已完成节点成本(节点 token 统计 × server 换算),超限 SHALL 经 `dag_cancel` 中止后续节点,已运行节点自然完成。core 的 engine/registry SHALL NOT 增加预算或 USD 字段。

#### Scenario: 预算超限中止后续节点

- **WHEN** server 预算工具监控的 run 累计成本超过上限(如 $2,已耗 $2.1)
- **THEN** 工具调用 `dag_cancel`,后续未启动节点不启动,运行中节点自然完成,run 结束状态为 cancelled(沿用 dag-orchestrator 语义)

#### Scenario: 单会话成本控制

- **WHEN** 主会话(非 graph)需要成本上限
- **THEN** 由 server 侧实现(如 `/cost` 展示 + 用户决策或 server 会话层检查),core 无 per-harness 预算机制
