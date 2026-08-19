# theway-tui

[English](README.md) | 中文

`theway-tui` 构建 `theway` 命令：一个面向 `thewayd` 的 ratatui client/controller，以及离线会话维护命令。它负责终端布局、输入、feed 渲染、picker、clipboard 集成、本地命令展示、daemon 发现/启动，以及连接 daemon 使用的 controller 侧工具与存储服务。

本 crate 依赖 [`theway-transport`](../theway-transport/README.md)、[`theway-storage`](../theway-storage/README.md) 和渲染 widget，但绝不依赖 [`theway-core`](../theway-core/README.md) 或 [`theway-daemon`](../theway-daemon/README.md)。运行时 turn、trigger、暴露给模型的工具和编排状态由 daemon 负责。

## 运行模式

- 交互模式启动 loopback `ToolService` 和 `StorageService` 实现，发现或启动 `thewayd`，向 daemon 下发配置，消费 snapshot/event，并运行终端应用。
- 无需活动运行时协调时，离线会话命令直接打开本地 `SqliteSessionRepo`，完成导出、导入、列举和删除。
- Headless/非交互渲染复用相同应用状态与 transport frame，不构建 agent 运行时。

## UI 组件

- [`theway-markdown`](../theway-markdown/README.md) 渲染流式 assistant 内容、代码、数学、表格、链接与 Mermaid 图。
- [`theway-ratatui-textarea`](../theway-ratatui-textarea/README.md) 提供 composer 编辑器。
- [`theway-pager-render`](../theway-pager-render/README.md) 提供宽度、scrollbar、颜色、路径和 OSC 8 链接辅助逻辑。

## 文档

- [Client/controller 架构](docs/architecture.md)
- [传输协议](../theway-transport/docs/architecture.md)
- [工作区架构](../../docs/architecture.md)

## 验证

```bash
cargo test -p theway-tui
cargo doc -p theway-tui --no-deps --document-private-items
make layering-check
```
