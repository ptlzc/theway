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

[`agent/assembly/mod.rs`](../src/agent/assembly/mod.rs) 由模型、带类型的 `Session`、skill、prompt template、工具、hook、observer、`RuntimeExtensionPort` 和可选 provider 流覆盖构建 `AgentHarness`。Harness 持久化 prompt 周期状态，发出 `SessionEvent`，统计成本，通过注入闭包重载 skill，并执行配置的续轮上限。`AgentHarnessOptions::new` 安装 `NoopRuntimeExtensionPort`，因此未配置 extension 的嵌入方不会进入 extension 引擎路径。

[`agent/session/session.rs`](../src/agent/session/session.rs) 定义追加式 `SessionTreeEntry` 并推导活动分支。`MemorySessionStorage` 服务于隔离嵌入场景和测试。`PersistentSessionStorage` 将带类型的条目编码为 `theway-contract::StoredSessionEntry`，把所有 I/O 委托给注入的 `SessionStore`，并从带类型 transcript 与模型上下文中滤除不透明 extension 记录。

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

## Runtime extension 端口

[`agent/runtime_extensions`](../src/agent/runtime_extensions/mod.rs) 定义由 session、run、request、message、tool 与 compaction 域 trait 组成的引擎无关 `RuntimeExtensionPort`。Core 调用包含生命周期关联和 JSON 兼容载荷，但不包含已发现的 extension 标识；daemon 拥有的实现把一次调用翻译到其 session 实例。

每个域 dispatcher 校验生命周期事件属于对应 core seam，并通过 `ExtensionHookContract` 校验返回的 ABI action batch。调用点只能获得类别专属的 `ValidatedRuntimeExtensionResult` variant，因此嵌入实现不能通过 input seam 应用 message 或 tool mutation。`RuntimeExtensionScopeAllocator` 在 clone 之间共享单调生命周期序列和带 session 限定的稳定标识。

`PersistentSessionExtensionStatePort` 将经过校验的耐久 action 转换为一个带父链的 `StoredSessionEntry` 批次，并通过 `SessionStore::append_entries` 提交；重放始终读取所选持久化分支。`ExtensionModelContextProjection` 滤除私有 state 与 custom event，保留 model-context 分支顺序，并在原位替换重复的 `(extension_id, context_id)` 值，使每个稳定条目只对模型可见一次。

`AgentHarness` 将 input、run、turn、context、模型选择、branch/session、fork 和 session 边界操作映射到这些端口。Input command outcome 在 provider 分发前停止，并作为结构化 `SessionEvent::ExtensionCommandOutcome` 发出；接受的 input/context replacement 保持消息 role，并局限于其声明的 seam。`before_run` patch 在 agent 发出 `run_started` 前原子持久化其父链消息，并在该 run 结束时恢复被替换的 system prompt。Run 终止事件在等待 transcript 持久化后按 `run_ended`、可选 `run_error`、`run_settled` 顺序发出。

Extension follow-up 使用独立的 32 项稳定 id 去重队列，不使用 bare Agent 的 run 内队列。Harness 只在 `run_settled` 后消费该队列，并在一个 prompt 周期达到 16 次 extension 驱动 follow-up run 后停止。Task-local 分发 guard 拒绝递归生命周期分发以及从 hook 同步启动的运行时操作。`shutdown_runtime_extensions` 取消活动 run，并等待异步 loop listener 完成后才发出 `session_shutdown`。

## 扩展规则

- Provider 协议和模型目录放在 `theway-llm-provider`，不放入 agent 循环。
- 面向模型的工具实现和宿主集成放在 `theway-daemon`；这里只添加可复用 trait 与数据类型。
- 存储后端放在 `theway-storage` 或其他实现叶子 trait 的 crate；带类型条目转换保留在 `PersistentSessionStorage`。
- 遥测 exporter 由嵌入运行时实现 `RuntimeObserver` 提供。
- 图执行后端通过 `NodeLauncher` 扩展，持久化通过 `DagPersistSink` 扩展。

## 不变量

- Core 与具体存储、传输、遥测和宿主执行库保持独立。
- 持久化状态通过 `theway-contract` 记录跨越 crate 边界，不传递后端类型。
- Core 生命周期端口不发现 package 或执行 extension 代码，daemon 原始 action batch 不能绕过 core 校验。
- 私有 extension state 不进入带类型会话消息或模型上下文投影。
- Extension follow-up 不能在 settlement 前进入活动 run，也不能无界递归。
- 取消会产生终止运行结果，并释放运行准入和控制句柄。
- 事件载荷和操作关联保持足够确定，使 daemon 无需访问 core 私有状态即可投影 snapshot。
