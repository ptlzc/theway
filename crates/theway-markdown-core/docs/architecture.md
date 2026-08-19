# Markdown core 架构

## 职责

`theway-markdown-core` 负责与终端无关的 Markdown 源解释。它只有一个运行时依赖 `pulldown-cmark`，调用方无需导入展示栈即可执行策略一致的分析。

## Parser 策略

[`parser_options`](../src/lib.rs) 是启用 Markdown 扩展的事实来源。[`offset_events`](../src/lib.rs) 用这些选项创建 parser，并保留每个事件在原始输入中的字节范围。

启用删除线扩展后，`pulldown-cmark` 会把单波浪线对也识别为删除线。`DoubleTildeOnlyStrike` 把这些单波浪线 pair 的开始/结束 tag 转成字面 delimiter 文本，同时保留 `~~双波浪线~~` tag。匹配的 start 与 end 事件携带相同源范围，因此该转换无需栈。

## 分析

[`analyze`](../src/lib.rs) 消费与 renderer 相同的 offset 事件流，并返回两类信息：

- [`MarkdownStats`](../src/lib.rs) 统计 heading、代码块、表格、链接、图像、数学和列表项等已解析结构。
- [`StructuralIssue`](../src/lib.rs) 识别“源文本明显意图产生某种结构，但解析后退化”的情况，包括格式错误的 GFM 表格和未终止 fenced code block。

CommonMark 解析是 total 的，因此结构诊断比较原始源意图与解析事件，而不是报告 parser error。诊断检查必须有界，且不得改变 renderer 消费的事件流。

## 边界与不变量

- 源 offset 是传入 `offset_events` 或 `analyze` 的输入中的 UTF-8 字节范围。
- Parser 扩展与单波浪线规则只在本 crate 实现一次。
- 本 crate 不包含终端、颜色、widget、语法主题或应用状态。
- 统计描述已解析结构；结构问题描述可能的渲染保真度失败。
