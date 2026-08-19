# theway-daemon

[English](README.md) | 中文

`theway-daemon` 是无头应用内核及 `thewayd` 二进制。它把 [`theway-core`](../theway-core/README.md)、[`theway-storage`](../theway-storage/README.md)、[`theway-transport`](../theway-transport/README.md)、[`theway-llm-provider`](../theway-llm-provider/README.md) 和 [`theway-mcp`](../theway-mcp/README.md) 组装成一个长驻服务。

Daemon 负责会话运行时组装、面向模型的工具、本地与 sandbox executor 选择、hook、trigger、cron job、嵌套 agent 编排、MCP/LSP 集成、遥测导出和协议侧行为。它没有客户端形态或终端展示概念；[`theway-tui`](../theway-tui/README.md) 只是一个协议客户端。

## 入口

- `thewayd` 解析进程参数，并调用公开组合入口 `run(DaemonOptions)`。
- `DaemonPaths` 在启动时一次性解析 base、home、工作目录和额外 skill 目录。
- `DaemonServices` 持有进程生命周期级注册表和命令输出注入。
- `SessionRuntimeBuilder` 是初始、恢复和切换会话运行时的统一内部构建路径。
- 公开模块为 executor、hook、存储适配器、工具、template、skill、trigger 和 TypeScript 扩展提供支持的扩展点。

默认 `local` feature 选择 `LocalExecutor`。只启用 `sandbox` 时选择 `SandboxExecutor`，不支持的操作以 `ExecutorError::UnsupportedKind` 失败。协议服务也可以把 `ToolOps` 转发到 controller 提供的 gRPC 工具端点。

## 运行与验证

```bash
cargo run -p theway-daemon --bin thewayd -- --help
cargo test -p theway-daemon
cargo doc -p theway-daemon --no-deps --document-private-items
```

[Daemon 架构](docs/architecture.md)说明启动、会话、存储、工具、协议和可观测性归属；[工作区架构](../../docs/architecture.md)定义跨 crate 分层。
