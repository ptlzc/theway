# theway-llm-provider 修改规则

本文件适用于 `crates/theway-llm-provider/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改规范化类型或 wire 实现前，先阅读 [crate 概览](README.md)和 [provider 架构](docs/architecture.md)。

## 边界规则

- Agent 循环、跨 turn 重试策略、工具执行、会话、权限和 UI 行为不得进入本 crate。
- Provider 专用请求字段、流事件名、认证和错误留在 `src/providers/`。
- 只有至少一个调用方或多个 provider 协议需要某概念时，才扩展共享规范化类型。
- Core 运行时 crate 不得依赖 provider 实现模块；调用方使用 crate 根 API 和规范化类型。

## 流式规则

- 对成功、provider 错误、取消、非法输入和 transport 失败，均发出有序 start/delta/end 事件并只完成一次终止结果。
- 交错 delta 中保留工具调用关联；历史切换到另一 provider 前规范化标识。
- Thinking、cache、image、usage 和 stop reason 的转换由各协议显式实现。
- 响应解析必须有界；协议允许的空 frame 或部分 frame 不得让结果 future 永久等待。
- 诊断中遮蔽 API key、authorization header 和 provider 响应中的秘密。

## Feature、目录与测试

- 添加 provider 时，在同一变更中加入 feature、模块声明、依赖集合和内置注册。
- 通过 [`scripts/regen_models.sh`](scripts/regen_models.sh) 同步生成模型数据与 Rust 投影；使用 importer 时显式设置 `TS_PATH`。
- 测试必须使用脚本化 provider 或本地 HTTP/SSE fixture，不得调用真实 provider API。
- 规范化类型、dispatch、转换、目录、凭证或 provider 扩展步骤变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 [README](README.md) 中的无默认 feature 与全 feature 命令，以及 `cargo doc -p theway-llm-provider --no-deps --document-private-items`。
