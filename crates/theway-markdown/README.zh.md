# theway-markdown

[English](README.md) | 中文

`theway-markdown` 为终端应用渲染完整或增量到达的 Markdown。它输出 ANSI 文本或带样式的 ratatui line，并携带 source map、hyperlink target、代码块 span 和稳定流式 checkpoint。

## 能力

- 使用冻结前缀与重新渲染的可变尾部进行增量渲染。
- 支持 GFM 表格、task list、代码块、行内样式、blockquote 和列表。
- 提供语法高亮与终端色彩级别适配。
- 将支持的 LaTeX 数学语法转换为 Unicode 近似表示。
- 对 Mermaid 渲染限制宽度；不支持或过大的图回退为源文本。
- 生成 OSC 8 hyperlink 元数据和源到渲染行的映射。

Parser feature 与仅双波浪线删除线策略来自 `theway-markdown-core`。Token 流使用 [`StreamingMarkdownRenderer`](src/streaming.rs)，完整文档使用 [`src/lib.rs`](src/lib.rs) 中的一次性函数。

## 开发

渲染流水线与流式契约见 [`docs/architecture.md`](docs/architecture.md)，目录修改规则见 [`AGENTS.md`](AGENTS.md)，代码来源见 [`NOTICE`](NOTICE)。

```bash
cargo test -p theway-markdown
cargo doc -p theway-markdown --no-deps --document-private-items
```
