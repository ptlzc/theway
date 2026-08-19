# Markdown renderer 架构

[English](architecture.md) | 中文

## 职责

`theway-markdown` 将 Markdown 源转换为终端输出。它负责解析编排、展示转换、语法与颜色适配、表格、数学、图、链接元数据和增量渲染状态，不负责应用 feed 状态或终端输入处理。

## 渲染流水线

一次性渲染按以下顺序执行：

1. [`latex_delimiters.rs`](../src/latex_delimiters.rs) 将支持的 LaTeX delimiter 规范化为 renderer 存储的标准形式。
2. [`parse.rs`](../src/parse.rs) 消费 `theway-markdown-core` 的 offset 事件流并构建 `ParsedMarkdown`。
3. [`render.rs`](../src/render.rs) 输出 ANSI 文本或 ratatui line，同时保留源行与源字节关联。
4. [`url_scan.rs`](../src/url_scan.rs) 检测不来自 Markdown link 语法的普通 URL，并追加 hyperlink target。

[`MarkdownRenderOutput`](../src/output.rs) 将 ratatui line、source mapping、hyperlink、代码块 span 和 link id 状态放在一起，消费者无需从渲染文本重建元数据。

## 流式模型

[`StreamingMarkdownRenderer`](../src/streaming.rs) 在 chunk 到达时规范化输入，并存储规范化后的源文本。Checkpoint 标记渲染输出已经稳定的前缀；后续 push 保留该前缀，只解析可变尾部。Link id 和开放代码块高亮状态跨越 checkpoint，使尾部渲染与完整渲染一致。

`finish` 执行最终尾部渲染和普通 URL 扫描。相同规范化源文本与渲染设置下，完成的流式输出和一次性输出在可见内容与元数据上必须一致。

## 专用转换

[`syntax.rs`](../src/syntax.rs) 选择语法定义和主题，[`colors.rs`](../src/colors.rs) 按终端色彩级别调整样式。[`latex/`](../src/latex/mod.rs) 在 pretty 模式下把支持的数学命令与环境转换为 Unicode 近似表示。

[`mermaid.rs`](../src/mermaid.rs) 解析有界 Mermaid 子集并布局为终端字符图。Renderer 执行宽度与复杂度上限；不支持或过大的输入转成带边框源文本，不执行无界布局。

## 边界与不变量

- Markdown parser 策略属于 `theway-markdown-core`；本 crate 不构造另一套选项。
- 源范围是 renderer 规范化源文本中的字节 offset，line map 标识渲染行对应的源行。
- 流式 checkpoint 只冻结后续 chunk 无法改变的输出。
- 宽度计算使用终端 display width 与 grapheme-aware 操作，不使用字节数或 Unicode scalar 数。
- 面向模型生成输入的图与高亮工作保持有界。
