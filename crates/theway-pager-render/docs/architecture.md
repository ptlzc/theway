# Pager 渲染架构

## 职责

`theway-pager-render` 是 [`theway-tui`](../../theway-tui/docs/architecture.md) 下方的展示工具层。函数操作 ratatui buffer、styled line、scroll geometry、URL 和路径；调用方负责应用状态、输入处理、导航和 target 打开。

## 文本与 geometry

[`line_utils.rs`](../src/line_utils.rs) 集中提供 ratatui line 的终端 display-width 操作。切片与截断保留 span 样式并尊重 Unicode grapheme 边界，调用方不能用 UTF-8 字节长度代替终端列宽。

[`scrollbar.rs`](../src/scrollbar.rs) 将内容长度、viewport 长度和 scroll position 转换为 scrollbar 渲染。[`color.rs`](../src/color.rs) 提供 buffer 级颜色混合与清理，不选择应用主题。

## 链接与路径标注

[`osc8.rs`](../src/osc8.rs) 检测渲染行中的 URL 与类文件 target，并添加 OSC 8 链接元数据。网络 URL 限制为 `http`、`https`，不把任意 URI scheme 提升为可点击 target。调用方决定是否及如何打开标注 target。

[`tool_paths.rs`](../src/tool_paths.rs) 按显式工作目录解析工具报告的路径，并生成紧凑展示形式。调用方可提供相关基础路径时，解析不得暗中依赖进程当前目录。

## 边界与不变量

- 工具函数不持有 feed、selection、会话、transport 或 daemon 状态。
- 可见列计算以 grapheme 为边界并使用终端 display width。
- 链接检测不执行 target，也不接受无限制 URI scheme。
- 路径辅助逻辑把解析 target 与缩短展示文本分开保存。
