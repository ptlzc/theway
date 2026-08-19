# Pager 渲染修改规则

本文件适用于 `crates/theway-pager-render/`。同时遵循 [`../../AGENTS.md`](../../AGENTS.md) 和 [`docs/architecture.md`](docs/architecture.md)。

## 归属

- 会话、协议、事件循环、键绑定和 target 打开策略留在调用方。
- Line 操作保持样式不丢失、grapheme-aware，并以终端 display width 为单位。
- 行为依赖 viewport 或工作目录时，要求调用方显式传入 geometry 与路径上下文。
- URL 标注只接受安全且支持的 scheme；标注不得执行或打开 target。

## 兼容性

- 代码来源细节保留在 [`NOTICE`](NOTICE)。
- 缩短展示 label 时独立保留解析后的真实 target。
- 聚焦测试放在受影响模块附近；多文件套件按 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) 组织。

## 验证

运行 `cargo test -p theway-pager-render` 和 `cargo doc -p theway-pager-render --no-deps --document-private-items`。
