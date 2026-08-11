## ADDED Requirements

### Requirement: LoopEvent 类型定义与命名

系统 SHALL 在 `crates/core/src/agent/run_loop/` 模块中定义 `LoopEvent` 枚举,替代现有 `AgentEvent` 类型。`LoopEvent` 的 20 个变体中,以下核心变体 MUST 使用新名称:

- `RunStarted` 替代 `AgentStart` — 表示一次 run_loop 执行周期的开始。
- `RunEnded` 替代 `AgentEnd` — 表示一次 run_loop 执行周期的结束。
- `TurnCompleted` 替代 `TurnEnd` — 表示单次 agent turn 完成(数据事件)。

其余变体 MUST 保持与现有 `AgentEvent` 变体的结构对应,仅移除 `Agent` 前缀(如 `TurnStart`、`ModelRequest`、`ToolExecutionStart`、`ToolExecutionEnd` 等)。`AgentEvent` 类型名 SHALL NOT 存在于任何编译通过的代码路径中。

#### Scenario: 消费者使用新类型名

- **WHEN** 外部代码引用 run_loop 模块的事件类型
- **THEN** 路径 `theway_core::agent::run_loop::LoopEvent` 可用,`AgentEvent` 不存在

#### Scenario: 核心变体名正确反映语义

- **WHEN** 开发者阅读 `LoopEvent::RunStarted` 或 `LoopEvent::TurnCompleted`
- **THEN** 变体名直接传达 run 周期或 turn 完成的语义,无需查阅额外文档

#### Scenario: 旧类型名完全移除

- **WHEN** 编译 workspace
- **THEN** grep `AgentEvent` 在 `.rs` 文件中无匹配(历史注释除外)

### Requirement: SessionEvent 类型定义与命名

系统 SHALL 在 `crates/core/src/agent/assembly.rs` 模块中定义 `SessionEvent` 枚举,替代现有 `HarnessEvent` 类型。核心变体命名 MUST 满足:

- `Started` 替代 `SessionStart` — 类型名已带 `Session`,变体去冗余。
- `TurnDecision` 替代 `TurnEnded` — 承载 turn 结束时的决策/审计语义,与 `LoopEvent::TurnCompleted`(数据事件)明确区分。

其余变体(Compaction、Branch、SkillReload 等)MUST 保持结构对应,名称不变。`HarnessEvent` 类型名 SHALL NOT 存在于任何编译通过的代码路径中。

#### Scenario: SessionEvent 反映会话生命周期

- **WHEN** 开发者阅读 `SessionEvent::Started`
- **THEN** 变体名直接传达会话启动的生命周期事件,不暴露实现细节("Harness")

#### Scenario: TurnDecision 与 TurnCompleted 区分

- **WHEN** 系统在 turn 结束时同时触发 `SessionEvent::TurnDecision`(决策/审计)与 `LoopEvent::TurnCompleted`(数据事 件)
- **THEN** 两个事件名一字不同,不会混淆;开发者无需猜测语义差异

#### Scenario: 旧类型名完全移除

- **WHEN** 编译 workspace
- **THEN** grep `HarnessEvent` 在 `.rs` 文件中无匹配(历史注释除外)

### Requirement: AgentJobEvent 保持稳定

系统 SHALL 保持 `AgentJobEvent` 类型名及其变体名(Started/Output/Metrics/Completed)不变。该类型位于 `crates/server/src/registry/events.rs`,职责为高频作业事件面,不在本轮重命名范围内。

#### Scenario: AgentJobEvent 不受影响

- **WHEN** 编译 workspace
- **THEN** `AgentJobEvent` 类型名与所有变体名与变更前一致,所有消费者无需适配

### Requirement: 事件面职责边界文档化

`assembly.rs` 模块文档与 `run_loop/mod.rs` 文档 SHALL 明确三套事件面的职责边界:

- `LoopEvent` — run_loop 内部数据事件,发射点仅在 `crates/core/src/agent/run_loop/`。
- `SessionEvent` — 会话生命周期事件(启动/压缩/分支/技能重载/turn 决策),发射点在 `assembly.rs`。
- `AgentJobEvent` — 高频作业事件(registry/events.rs),grpc 传输面。

文档 SHALL 指导外部观测者:观测 run_loop 执行过程使用 `LoopEvent`,观测会话状态使用 `SessionEvent`,观测作业生命周期使用 `AgentJobEvent`。

#### Scenario: 新观测者定位正确的事件面

- **WHEN** 开发者需要观测一次 agent run 的 turn 级进度(text/tool calls/turn sequence)
- **THEN** 文档指明订阅 `LoopEvent`(或新的 broadcast 通道),而非 `SessionEvent`

#### Scenario: 混淆预防

- **WHEN** 开发者看到 `TurnDecision` 与 `TurnCompleted`
- **THEN** 文档说明前者是 SessionEvent(决策/审计),后者是 LoopEvent(数据),无歧义
