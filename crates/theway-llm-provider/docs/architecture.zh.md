# theway-llm-provider 架构

[English](architecture.md) | 中文

## 规范化数据模型

[`types.rs`](../src/types.rs) 定义与 provider 无关的模型：

- `Model` 标识 provider、wire API、端点、能力、上下文窗口、成本和支持的 thinking level。
- `Context` 包含有序 user/assistant/tool-result 消息与工具定义。
- `StreamOptions` 包含请求级上限、thinking、cache、transport、header、中止和密钥解析行为。
- `AssistantMessageEvent` 是文本、thinking、工具调用 delta、完成与错误的有序流协议。
- `AssistantMessage` 是收集后的终止结果，包含内容、用量、stop reason、provider/模型标识和可选错误。

Provider 模块只在该模型与外部 wire 协议之间转换。`AgentMessage`、工具调度、会话条目或权限决策等运行时概念不进入本 crate。

## Provider 注册与 dispatch

[`api_registry.rs`](../src/api_registry.rs) 定义 `ApiProvider` 和按 API id 索引的进程注册表。查找返回自有 `RegisteredHandle`，因此请求解析到 provider 后，即使注册来源并发移除也能完成。

[`providers/register_builtins.rs`](../src/providers/register_builtins.rs) 通过 `OnceLock` 只注册一次已启用内置实现。Cargo feature 控制编译哪些实现与 provider 专用依赖。运行时扩展可按 source id 注册 provider，并移除该来源的全部 provider。

[`stream.rs`](../src/stream.rs) 确保内置实现已注册，解析 `model.api` 并委托给 provider。缺失或不匹配 provider 与 provider 失败使用相同终止错误流形式。`complete` 消费流结果，不实现第二条请求路径。

## 请求与流流水线

发送请求前，[`providers/transform_messages.rs`](../src/providers/transform_messages.rs) 按目标模型能力调整历史。它可以降级不支持的图像、转换不兼容 thinking block、规范化工具调用标识、移除不可用错误 turn，并删除孤儿工具调用，使 provider API 收到合法工具序列。

[`providers/mod.rs`](../src/providers/mod.rs) 下每个 provider 模块负责请求 body、认证 header、端点选择、流解码、usage/stop reason 映射和协议专用工具/thinking 转换。共享 Responses、Google、prompt cache、SSE、AWS event stream、retry、overflow、validation 和 Unicode 辅助逻辑保留在 `providers/` 对应共享模块或 [`utils/mod.rs`](../src/utils/mod.rs)。

[`provider_interceptor.rs`](../src/provider_interceptor.rs) 为 `open_ai_chat_completions`、`open_ai_responses` 与 `anthropic_messages` 定义可选 `ProviderRequestInterceptor` 边界。Adapter 先构建完整 provider-format JSON 与不含 secret 的 header，依次应用 header transform 和 raw-payload transform，按照活动 format 校验 replacement，随后才开始网络 I/O。认证 header 与配置的 secret header 不进入 hook payload；受保护 header 或跨 format replacement 会保留完整的先前值。任何 response body 被消费前，最终 HTTP status 与脱敏 header 已完成 observe。Authentication、serialization、client 或 transport failure 只发布一次脱敏 request-failure observation，不伪造 response metadata。

[`utils/event_stream.rs`](../src/utils/event_stream.rs) 将 `AssistantMessageEventSender` 与 `AssistantMessageEventStream` 配对。Sender 发布有序事件并完成一个最终 `AssistantMessage`；stream 支持增量消费和等待终止结果。

取消或 provider 失败必须以规范化 stop/error 记录终止流。Provider 网络 task 退出后不得留下仍在等待终止结果的调用方。

## 目录与凭证

[`models.rs`](../src/models.rs) 把运行时自定义模型覆盖到 [`models_generated.rs`](../src/models_generated.rs) 和 [`models.generated.json`](../src/models.generated.json) 的生成目录上。[`image_models.rs`](../src/image_models.rs) 提供相应图像目录。

[`env_api_keys.rs`](../src/env_api_keys.rs) 将 provider 标识映射到环境变量。调用方也可通过 stream options 提供 `get_api_key`；provider 流水线消费解析后的凭证但不持久化。

[`session_resources.rs`](../src/session_resources.rs) 暴露 provider 自有会话资源清理。长连接 cache 属于 provider 基础设施，不成为 agent 会话状态。

## 添加 provider

1. 在 [`Cargo.toml`](../Cargo.toml) 中添加 feature 及该协议所需的最小依赖。
2. 在 provider 模块实现 `ApiProvider`；只有 wire 协议确实共享时才复用请求/流辅助逻辑。
3. 在 `providers/register_builtins.rs` 的同一 feature 下注册实现。
4. 将所有内容、工具、thinking、usage、stop、取消和错误路径映射到规范化类型。
5. 添加无需密钥的请求/流 fixture 和 feature 矩阵编译覆盖。

## 不变量

- Provider 专用 JSON、header、事件名和错误 body 留在 provider 模块。
- 每个已启动流只完成一次规范化终止结果。
- 工具调用 delta 保留 provider 关联，同时生成稳定规范化标识与参数文本。
- 消息转换保留合法历史顺序，不静默发送模型不支持的内容。
- 凭证由调用方或环境提供，不写入目录、消息、诊断或会话资源。
- Provider interception 不包含认证值，按照活动 wire format 校验单个 raw payload，并在读取 stream byte 前观察 response metadata。
- 未启用 feature 的 provider 及其可选依赖树不进入精简构建。
