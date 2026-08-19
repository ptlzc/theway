# theway-pager-render

`theway-pager-render` 为 feed 和 pager 风格视图提供可复用 ratatui 渲染原语。它不包含会话状态、daemon 协议或应用事件循环。

## 模块

- [`color`](src/color.rs) 混合与清理终端 buffer 区域。
- [`line_utils`](src/line_utils.rs) 按终端 display width 测量、切片、截断和转换带样式 ratatui line。
- [`scrollbar`](src/scrollbar.rs) 为 feed pane 适配 `tui-scrollbar`。
- [`osc8`](src/osc8.rs) 检测安全 web link 与文件引用，并覆盖 OSC 8 元数据。
- [`tool_paths`](src/tool_paths.rs) 解析并缩短工具报告的路径用于展示。

TUI 把这些原语与自身视图状态和交互策略组合。链接打开仍由应用负责；本 crate 只识别并标注 target。

## 开发

模块边界与安全规则见 [`docs/architecture.md`](docs/architecture.md)，目录修改规则见 [`AGENTS.md`](AGENTS.md)，代码来源见 [`NOTICE`](NOTICE)。

```bash
cargo test -p theway-pager-render
cargo doc -p theway-pager-render --no-deps --document-private-items
```
