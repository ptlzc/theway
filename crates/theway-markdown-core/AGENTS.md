# Markdown core 修改规则

本文件适用于 `crates/theway-markdown-core/`。同时遵循 [`../../AGENTS.md`](../../AGENTS.md) 和 [`docs/architecture.md`](docs/architecture.md)。

## 归属

- 本 crate 保持 headless，不添加 ratatui、syntect、终端能力、UI 状态或 transport 依赖。
- [`parser_options`](src/lib.rs) 是唯一 parser feature 定义；消费者统一经过 [`offset_events`](src/lib.rs)。
- 源范围保持为调用方原始输入中的 UTF-8 字节 offset。
- 只有有界的渲染保真度检查才能成为 [`StructuralIssue`](src/lib.rs)；正常 CommonMark fallback 不是错误。

## 兼容性

- 除非 renderer 与分析契约一起变化，否则保留仅双波浪线删除线策略。
- 修改统计字段时同步更新 [`MarkdownStats::as_pairs`](src/lib.rs)；其穷举映射负责防止下游漂移。
- 代码来源细节写入 [`NOTICE`](NOTICE)，不写入 API 或架构叙述。

## 验证

运行 `cargo test -p theway-markdown-core` 和 `cargo doc -p theway-markdown-core --no-deps --document-private-items`。Parser 事件变化还要运行 `cargo test -p theway-markdown`。
