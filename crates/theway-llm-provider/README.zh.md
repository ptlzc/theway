# theway-llm-provider

[English](README.md) | 中文

`theway-llm-provider` 是 [`theway-core`](../theway-core/README.md)、[`theway-daemon`](../theway-daemon/README.md) 和协议侧模型目录使用的规范化流式 LLM 客户端。Provider 专用 HTTP 载荷与流 frame 从 `ApiProvider` 实现进入，统一转换为 `AssistantMessageEvent` 记录输出。

本 crate 负责模型与图像目录、provider 注册、环境变量密钥查找、消息规范化、SSE/event-stream 辅助函数和 provider 协议实现。它不负责 agent 循环、工具执行、会话持久化、跨 turn 重试或用户交互。

## 公开 API

- `stream` 与 `stream_simple` 返回 `AssistantMessageEventStream`；`complete` 与 `complete_simple` 把同一流收集为 `AssistantMessage`。
- `ApiProvider` 及 `register_api_provider`、`unregister_api_providers` 支持内置和运行时注册协议。
- `Model`、`Context`、`Message`、`Tool`、`StreamOptions`、`AssistantMessage` 和 `AssistantMessageEvent` 组成与 provider 无关的请求/响应模型。
- `get_model`、`list_models` 和自定义模型注册暴露生成目录与运行时模型目录。
- `images`、`get_image_model` 和 `list_image_models` 暴露独立的图像生成路径。

## 功能开关

具体文本 provider 由 Cargo feature 选择：`anthropic`、`openai-completions`、`openai-responses`、`openai-codex-responses`、`azure-openai-responses`、`google`、`google-vertex`、`amazon-bedrock`、`cloudflare`、`mistral` 和 `faux`。`openrouter-images` 启用图像生成，`all-providers` 启用完整工作区组合。默认启用 `anthropic` 和 `faux`。

```bash
cargo check -p theway-llm-provider --no-default-features
cargo check -p theway-llm-provider --all-features
cargo test -p theway-llm-provider --all-features
```

[`examples/`](examples/anthropic_hello.rs) 中连接 provider 的示例需要相应 API key；测试只使用本地或脚本化 fixture。

## 文档

- [Provider 流水线与扩展规则](docs/architecture.md)
- [工作区架构](../../docs/architecture.md)
