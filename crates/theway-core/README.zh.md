# theway-core

[English](README.md) | 中文

`theway-core` 是由 `theway-daemon` 组装的可复用 agent 运行时。它负责单 agent 循环、`AgentHarness`、带类型的运行时会话、skill 与 prompt 组装、上下文压缩、生命周期 hook 接口与权限 hook、`RuntimeExtensionPort`、`ToolExecutor` 和 `RuntimeObserver` 接口，以及多 agent DAG/goal 编排。

Core 不负责具体工具、文件系统或进程实现、持久化后端、遥测 exporter 或协议服务。工作区分层检查只允许 `theway-daemon` 直接消费该运行时。

## 公开入口

- `Agent` 和 `AgentOptions` 运行与 provider 无关的消息及工具循环。
- `AgentHarness` 将 agent 与带类型的 `Session`、skill、压缩、成本统计和跨 turn 生命周期 hook 接口组合起来。
- `PersistentSessionStorage` 在带类型的会话条目与 `theway-contract` 的原始 `SessionReader`、`SessionStore` 记录之间转换。
- `RuntimeExtensionPort` 将引擎无关的生命周期分发拆分为 session、run、request、message、tool 与 compaction 域；嵌入式宿主校验并提交持久 action 后，core 消费规范替换和 follow-up；默认实现为空操作。
- `NormalizedModelRequestDraft` 是在 provider 序列化前接受变换、且只作用于本次请求的 provider 无关 system/message/tool/generation 快照。
- `ToolExecutor` 定义由嵌入式运行环境提供的文件系统和进程操作。
- `RuntimeObserver` 接收与传输无关的操作开始与结束记录。
- 启用 `harness` feature 时，`multiagent` 提供嵌套 agent 运行、实时 subagent job 状态、DAG 调度和 goal 评估。

## 功能开关

默认构建启用 `harness` 和 `default-providers`。`harness` 包含会话、skill、压缩、权限、生命周期 hook 接口与多 agent 编排；`default-providers` 启用 `theway-llm-provider` 中的 Anthropic 和 faux provider 实现。

```bash
# Bare Agent loop
cargo check -p theway-core --no-default-features

# Harness without concrete providers
cargo check -p theway-core --no-default-features --features harness
```

## 文档

- [运行时架构与扩展接口](docs/architecture.md)

## 验证

```bash
cargo test -p theway-core
cargo doc -p theway-core --no-deps --document-private-items
make layering-check
```
