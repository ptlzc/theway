# theway-markdown-core

[English](README.md) | 中文

`theway-markdown-core` 是无终端依赖的 Markdown 策略与分析 crate。它暴露终端 renderer 使用的 parser 配置、保留 offset 的事件流、元素统计和结构诊断，不依赖 ratatui、语法高亮或终端能力。

## 公开 API

- [`parser_options`](src/lib.rs) 定义启用的 `pulldown-cmark` 扩展：GFM、表格、task list、数学和删除线。
- [`offset_events`](src/lib.rs) 返回带源字节范围的 parser 事件，并执行“只有 `~~双波浪线~~` 表示删除线”的项目规则。
- [`analyze`](src/lib.rs) 生成 [`MarkdownAnalysis`](src/lib.rs)，组合 [`MarkdownStats`](src/lib.rs) 与渲染保真度 [`StructuralIssue`](src/lib.rs)。

调用方只需检查 Markdown、不希望引入 `theway-markdown` 时使用本 crate。Renderer 也应使用 `offset_events`，而不是自行构造 parser，确保分析与渲染对同一源文本作出一致解释。

## 开发

机制与不变量见 [`docs/architecture.md`](docs/architecture.md)，目录修改规则见 [`AGENTS.md`](AGENTS.md)，代码来源见 [`NOTICE`](NOTICE)。

```bash
cargo test -p theway-markdown-core
cargo doc -p theway-markdown-core --no-deps --document-private-items
```
