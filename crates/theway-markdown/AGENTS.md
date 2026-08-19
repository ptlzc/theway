# Markdown renderer 修改规则

本文件适用于 `crates/theway-markdown/`。同时遵循 [`../../AGENTS.md`](../../AGENTS.md) 和 [`docs/architecture.md`](docs/architecture.md)。

## 归属

- 应用 feed 状态、输入事件和 transport 逻辑不得进入本 crate。
- 共享 parser 选项或删除线解释在 [`theway-markdown-core`](../theway-markdown-core/AGENTS.md) 修改，并验证两个 crate。
- 添加渲染转换时保留 source map、hyperlink range 和代码块 span。
- 终端布局使用 display width 与 grapheme 边界。

## 流式契约

- 相同规范化源文本与设置下，完成后的流式渲染必须与一次性渲染一致。
- 只有后续 chunk 无法改变某段的解析或渲染含义时，才冻结 checkpoint。
- Link id 与开放代码块高亮状态穿过尾部重渲染，不从渲染文本反推稳定元数据。
- Mermaid 解析与布局保持有界，并提供可读源文本 fallback。

## 兼容性

- 代码来源细节保留在 [`NOTICE`](NOTICE)。
- 更新共享来源代码时保留本地 parser 策略、终端色彩适配和宽度上限。
- 聚焦测试按 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) 放入镜像测试布局。

## 验证

运行 `cargo test -p theway-markdown-core -p theway-markdown` 和 `cargo doc -p theway-markdown --no-deps --document-private-items`。
