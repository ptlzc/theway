# theway-daemon 架构

[English](architecture.md) | 中文

## 应用角色

`theway-daemon` 是工作区唯一直接消费 `theway-core` 的 crate。它提供具体宿主行为，并把 core 状态适配到持久化和协议 crate。可复用运行时机制留在 core；wire 表示留在 `theway-transport`；终端交互留在 `theway-tui`。

[`src/lib.rs`](../src/lib.rs) 围绕 `DaemonOptions`、`DaemonServices`、`DaemonTransport`、`SessionSelection`、`DaemonPaths` 和 `run` 暴露精简应用 API。大多数编排与状态模块保持 crate 私有。

## 启动组装

[`orchestration/startup.rs`](../src/orchestration/startup.rs) 是应用组合路径：

1. 设置解析后的工作目录，并选择本地或远程 `RuntimeStorage`。
2. 创建或恢复原始会话 store，初始化日志与遥测。
3. 解析模型配置并构建 provider 流函数。
4. 创建进程生命周期服务，加载 trigger/cron 状态，并用同一个 observer 构建 DAG engine 与 subagent job registry。
5. 选择 `ToolExecutor`，加载可选 MCP/LSP/hook/template/skill/extension 来源，并组装面向模型的工具。
6. 构建初始 `SessionRuntime`，创建 `TurnHost`，交给所选 gRPC、HTTP 或 MCP 服务生命周期。

[`paths.rs`](../src/paths.rs) 在 CLI 边界解析 base、home、工作目录和额外 skill 目录。运行时模块只接收 `DaemonPaths` 或显式路径，不自行解析 `HOME`、`THEWAY_DIR` 或进程当前目录。

[`orchestration/services.rs`](../src/orchestration/services.rs) 持有 trigger/cron 注册表、notification hook 和命令输出等进程生命周期可变服务。测试与嵌入方通过构造 `DaemonServices` 替换行为，而不是修改进程全局状态。

## 会话运行时生命周期

[`orchestration/session.rs`](../src/orchestration/session.rs) 负责 `SessionRuntimeBuilder`。初始启动、恢复和会话切换都经过同一个 builder，它会：

- 通过 `SessionRepository` 打开注入的 `SessionStore`；
- 使用 `theway-core::PersistentSessionStorage` 进行适配；
- 校验持久化的工作目录绑定；
- 为该会话构建 `AgentHarness`、trigger 执行、图持久化、job transcript、hook 和 notification 注册；
- 按需从活动持久化分支恢复带类型的运行时状态。

[`turn/kernel.rs`](../src/turn/kernel.rs) 提供 `ReplKernel`，负责单个活动 prompt/continuation 的准入、排队 turn，并在切换会话时整体替换运行时。[`turn/daemon.rs`](../src/turn/daemon.rs) 负责与协议无关的 daemon 状态机、命令路由、snapshot、feed 更新和生命周期事件处理。

## 存储归属

[`runtime_storage.rs`](../src/runtime_storage.rs) 定义 daemon 应用 port：

- `RuntimeStorage` 提供会话仓库、DAG 快照、job transcript、trigger 规则、cron job 和持久化 sink。
- `SessionRepository` 使用 `Arc<dyn SessionStore>` 提供创建、恢复、打开、列举、删除、fork 和导入，而不暴露具体数据库类型。

本地适配器使用 `theway-storage`。`RemoteRuntimeStorage` 使用 `theway-transport` 的存储 RPC 操作。编排代码依赖这些 daemon trait，不暴露 SQLite 类型。

## 工具与宿主集成

[`tools/mod.rs`](../src/tools/mod.rs) 包含面向模型的工具实现与组装。文件系统、命令、git、搜索、memory、skill、MCP、web、subagent 和 DAG 工具由 daemon 负责，因为它们把 core 工具接口与宿主策略、外部服务组合起来。

[`executor/mod.rs`](../src/executor/mod.rs) 实现 `theway-core::ToolExecutor`。默认 `local` feature 提供 `LocalExecutor`；不启用 `local` 的 `sandbox` 构建提供快速失败占位实现。[`forwarding_tool_ops.rs`](../src/forwarding_tool_ops.rs) 是独立协议适配器，把 `ToolOps` 请求发送到 `WireDaemonConfig` 中的 controller 地址，并在地址变化时刷新缓存客户端。

[`hooks/mod.rs`](../src/hooks/mod.rs)、[`hook_executors.rs`](../src/hook_executors.rs)、[`trigger_engine/mod.rs`](../src/trigger_engine/mod.rs) 和 [`triggers/mod.rs`](../src/triggers/mod.rs) 负责进程/webhook 操作、动态 trigger 轮询与提升、cron 执行和 notification 投递。持久化 sidecar 记录来自 `theway-contract`，调度与投递策略留在本 crate。

[`mcp_loader.rs`](../src/mcp_loader.rs) 使用 `theway-mcp` 发现外部 MCP 工具与 notification。[`mcp_server.rs`](../src/mcp_server.rs) 将 daemon 暴露为 MCP server。[`lsp_supervisor.rs`](../src/lsp_supervisor.rs) 负责 language server 进程生命周期。

## 协议适配

[`transport_adapter.rs`](../src/transport_adapter.rs) 把 core DAG 运行、节点、job 状态和事件转换为 transport 拥有的 wire snapshot，并实现 `GraphOps`、`JobOps`。Transport crate 接收 `TransportEndpoints` 和 `TransportHost`，不访问 `AgentHarness` 或 daemon 私有状态。

跨客户端行为先以 `theway-transport` 类型或操作定义。Daemon 实现协议侧语义并发出 snapshot 或事件；外观、按键、布局和本地交互由客户端负责。

## 可观测性

[`observability.rs`](../src/observability.rs) 通过有界非阻塞队列实现 core 的 `RuntimeObserver`。Worker 输出结构化日志、OpenTelemetry trace/metric 和 Prometheus 测量，但不把 prompt、消息、工具参数、工具结果、生成文本或原始错误字符串写入可观测记录。

同一个 observer 实例注入主 harness、恢复后的 harness、`SubagentJobRegistry` 和 `DagEngine`。Exporter 或队列失败不改变运行时结果；关闭过程在有界超时内排空 worker。

## 不变量

- 启动与切换会话只有一条 `SessionRuntimeBuilder` 构建路径。
- 进程服务和存储实现通过 owned handle 与 trait 注入，不使用隐藏全局变量或具体 SQLite 类型。
- Daemon 负责运行时语义，不持有客户端展示状态。
- 协议转换由 daemon 适配器针对 transport 拥有的消息完成。
- 宿主路径只解析一次并显式传递。
- 工具、trigger、hook、MCP、LSP 和遥测失败通过各自操作报告，不破坏会话运行时生命周期。
