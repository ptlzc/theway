# theway-ratatui-textarea

`theway-ratatui-textarea` 是可复用多行编辑器和 ratatui widget。它提供 grapheme-aware 编辑、soft wrap、selection、鼠标交互、clipboard 集成、undo/redo、scroll，以及应用插入且不可拆分的 atomic text element。

## 公开 API

- [`EditBuffer`](src/editor.rs) 与 [`EditPlan`](src/editor.rs) 提供与 UI 无关的编辑规划和校验后应用。
- [`TextArea`](src/textarea.rs) 配置 widget，`TextAreaState` 持有可变文本、cursor、selection、history、scroll 和 element 状态。
- [`TextElement`](src/textarea.rs) 标记 atomic range，[`TextElementEvent`](src/textarea.rs) 向应用报告 element 交互。
- [`ClipboardProvider`](src/textarea.rs) 允许嵌入应用选择系统或内部 clipboard 行为。

[`examples/textarea_demo.rs`](examples/textarea_demo.rs) 展示键盘输入、selection、搜索、渲染和 clipboard 接线。

## 开发

编辑器、widget、wrap 和渲染层见 [`docs/architecture.md`](docs/architecture.md)，目录修改规则见 [`AGENTS.md`](AGENTS.md)，代码来源见 [`NOTICE`](NOTICE)。

```bash
cargo test -p theway-ratatui-textarea
cargo check -p theway-ratatui-textarea --example textarea_demo
cargo doc -p theway-ratatui-textarea --no-deps --document-private-items
```
