# theway-core 架构

[English](architecture.md) | 中文

## 职责与依赖

`theway-core` 使用 `theway-contract` 的原始持久化记录，并使用 `theway-llm-provider` 的规范化模型消息与流。启用 `harness` feature 时，可选 Mermaid parser 用于解析 DAG plan。

本 crate 暴露运行时机制和宿主接口。`theway-daemon` 提供工具、存储实现、进程和文件系统行为、遥测导出、配置来源与协议适配。

## 单 agent 执行

[`agent.rs`](../src/agent.rs) 负责 `Agent`、可变 `AgentState`、运行准入、steering 与 follow-up 队列、取消和生命周期订阅。一个 `Agent` 同时只能运行一个 prompt 或续轮；并发准入返回 `AgentRunError::AlreadyStreaming`。

[`agent/run_loop/mod.rs`](../src/agent/run_loop/mod.rs) 驱动每个 turn：

1. 将运行时消息转换为 provider 消息并启动规范化 LLM 流。
2. 把流更新写入 agent 状态并发出 `LoopEvent`。
3. 对工具调用进行分类、授权和启动，互相独立的调用可并发运行。
4. 追加工具结果，按照 `QueueMode` 排空 steering 或 follow-up 消息，并决定是否继续下一 turn。
5. 取消或中断时收束部分输出，使状态与已发事件一致。

工具实现 `AgentTool`。宿主级文件系统与进程操作使用对象安全的 [`ToolExecutor`](../src/executor.rs) trait；core 不提供 executor 实现。

## Harness 与会话

[`agent/assembly/mod.rs`](../src/agent/assembly/mod.rs) 由模型、带类型的 `Session`、skill、prompt template、工具、hook、observer 和可选 provider 流覆盖构建 `AgentHarness`。Harness 持久化 prompt 周期状态，发出 `SessionEvent`，统计成本，通过注入闭包重载 skill，并执行配置的续轮上限。

[`agent/session/session.rs`](../src/agent/session/session.rs) 定义追加式 `SessionTreeEntry` 并推导活动分支。`MemorySessionStorage` 服务于隔离嵌入场景和测试。`PersistentSessionStorage` 将带类型的条目编码为 `theway-contract::StoredSessionEntry`，并把所有 I/O 委托给注入的 `SessionStore`。

[`agent/compaction/mod.rs`](../src/agent/compaction/mod.rs) 估算上下文占用、选择切分点、生成或调用摘要器，并记录压缩元数据，不感知会话使用哪种持久化后端。

## 多 agent 运行时

[`multiagent/runner.rs`](../src/multiagent/runner.rs) 为一次嵌套 agent 运行启动全新 harness，过滤工具集合，执行空闲超时取消，并返回规范化输出与用量。

[`multiagent/jobs.rs`](../src/multiagent/jobs.rs) 负责 `SubagentJobRegistry`，即嵌套 job 的有界实时视图。它跟踪生命周期、指标、消息、interrupt/steer 控制句柄，并可通过 `JobTranscriptStore` 持久化 transcript。

[`multiagent/graph.rs`](../src/multiagent/graph.rs) 负责 DAG 与 goal 运行调度：

- `model.rs` 校验定义、构建运行、推导下游闭包并协调节点就绪状态。
- `mermaid.rs` 将 Mermaid flowchart 文本适配为 DAG 定义，并渲染运行状态。
- `engine.rs` 负责运行状态、retry/skip/cancel 转换、事件、持久化通知和 launcher 注入。
- `scheduler.rs` 在并发和依赖状态约束下选择就绪节点。
- `node_launcher.rs` 将图节点适配为嵌套 agent 运行。
- `persist.rs` 通过注入的 `DagPersistSink` 在活动运行与持久化记录之间转换。

[`multiagent/goal.rs`](../src/multiagent/goal.rs) 在会话中存储 goal 状态，并实现 turn 结束评估器：完成 goal、暂停，或请求下一 turn。DAG 与 goal 运行共用 `DagEngine`，由运行类型区分生命周期规则。

## 可观测记录与产品事件

[`observability.rs`](../src/observability.rs) 定义 `RuntimeObserver`、关联操作标识、稳定结果与错误分类，以及 `OperationScope`。未完成的 scope 在 drop 时发出 abandoned 结束记录。Observer 调用与运行时结果隔离，默认实现为空操作。

可观测记录不是产品事件流。`LoopEvent`、`SessionEvent`、`SubagentJobEvent` 和 `DagEvent` 向持久化、工具与客户端传递运行时状态；`RuntimeObservation` 向嵌入方拥有的 exporter 传递不含内容的运行测量。

## 扩展规则

- Provider 协议和模型目录放在 `theway-llm-provider`，不放入 agent 循环。
- 面向模型的工具实现和宿主集成放在 `theway-daemon`；这里只添加可复用 trait 与数据类型。
- 存储后端放在 `theway-storage` 或其他实现叶子 trait 的 crate；带类型条目转换保留在 `PersistentSessionStorage`。
- 遥测 exporter 由嵌入运行时实现 `RuntimeObserver` 提供。
- 图执行后端通过 `NodeLauncher` 扩展，持久化通过 `DagPersistSink` 扩展。

## 不变量

- Core 与具体存储、传输、遥测和宿主执行库保持独立。
- 持久化状态通过 `theway-contract` 记录跨越 crate 边界，不传递后端类型。
- 取消会产生终止运行结果，并释放运行准入和控制句柄。
- 事件载荷和操作关联保持足够确定，使 daemon 无需访问 core 私有状态即可投影 snapshot。
