## Why

theway 的事件面目前碎片化: `AgentEvent`(20 变体中 16 个是 run_loop 机制,发射点全在 `crates/core/src/agent/run_loop/`)、`HarnessEvent`(6 变体中 4 个是会话生命周期,"Harness"是实现细节名)、`AgentJobEvent`(保留)三套类型的命名未反映其真实职责;分发机制不一致 — `AgentListener` 的 for-await 同步分发无 panic 隔离但 `HarnessListener` 有 `catch_unwind`;慢消费者会阻塞 emit 点。这导致新观测者需要同时理解多套事件面、事件名误导(如 `HarnessEvent::TurnEnded` 与 `AgentEvent::TurnEnd` 一字之差但语义不同 — 前者是决策/审计事件,后者是数据事件)、以及运行时可靠性风险。

## What Changes

- **事件重命名 (命名契约)**: `AgentEvent` → `LoopEvent`(反映其真实职责:run_loop 机制的数据事件);`AgentEvent::AgentStart` → `RunStarted`、`AgentEnd` → `RunEnded`、`TurnEnd` → `TurnCompleted`。`HarnessEvent` → `SessionEvent`(反映会话生命周期职责);`SessionStart` → `Started`、`TurnEnded` → `TurnDecision`(消除与 `TurnCompleted` 一字之差的混淆)。`AgentJobEvent` 保留不变。**BREAKING**: 所有消费者需适配新类型名与变体名(core + server 全域 grep 替换,约 30-50 处引用)。
- **分发机制重构**: 废弃当前 `AgentListener` 的纯 for-await 同步分发模型(慢消费者阻塞 emit 点、无 panic 隔离),采用方案 D — **broadcast channel 主路径 + 轻量同步回调辅路径**。broadcast 通道承载 grpc/持久化/外部订阅等长链路消费者,轻量同步回调承载 cost tracker 等短操作。trait 观察者方案被否(运行时动态 add/remove,规模小不值得)。

## Capabilities

### New Capabilities

- `event-naming`: 事件类型命名契约 — `LoopEvent`(run_loop 数据事件)、`SessionEvent`(会话生命周期事件)、`AgentJobEvent`(高频作业事件)的职责边界与命名规范。
- `event-dispatch`: 事件分发机制 — broadcast channel 主路径 + 同步回调辅路径的分层分发架构,pani c隔离与慢消费者保护。

### Modified Capabilities

- 无(`openspec/specs/` 无已归档规范)。

## Impact

- **代码**: `crates/core/src/agent/run_loop/`(AgentEvent→LoopEvent 命名 + 分发机制)、`crates/core/src/agent/assembly.rs`(HarnessEvent→SessionEvent 命名)、`crates/core/src/agent/` 全局(事件订阅方适配)、`crates/server/src/` 全局(消费者适配 + 新 broadcast 通道消费端)、`crates/server/src/registry/events.rs`(AgentJobEvent 消费者适配)。
- **API**: `AgentEvent` 类型重命名为 `LoopEvent`、`HarnessEvent` 重命名为 `SessionEvent`(**BREAKING**);`AgentListener` 的 for-await 订阅替换为 broadcast 通道订阅(**BREAKING**,订阅方需更换接收方式);新增 `LoopEvent`/`SessionEvent` 的 broadcast sender + 同步回调注册 API。
- **行为**: emit 点不再被慢消费者阻塞;panic 隔离覆盖所有分发路径;`TurnDecision` 与 `TurnCompleted` 不再一字之差。
- **不改变**: `AgentJobEvent` 类型名与语义、DAG 编排语义、`dag_*` 工具行为、事件携带的数据字段(仅改名,不增减字段)。
