# tui-grok-prompt-chrome

Issue: #28

## Problem

输入框仍是 theway 自制的 chrome（直角青色边框 + `> ` 前缀 + 独立状态行）。
#24 只移植了 textarea 内核（xai-ratatui-textarea），没有移植 Grok 的输入框 UI。

Grok 的输入框在 `/root/workspace/grok-build/crates/codegen/xai-grok-pager/src/views/prompt_widget/mod.rs`
（draw() / render_info_line() / PromptStyle），checkout 里可读。

## What changes

把 Grok prompt widget 的 chrome 外观移植为 theway-tui 的轻量模块
`ui/prompt_chrome.rs`（仅外观层，不引入 pager 生态依赖）：

- 圆角边框 `╭─╮│╰─╯`，tokyonight 色：focused `rgb(75,92,140)` / unfocused `rgb(60,75,120)`
- `❯` 前缀（2 列）：focused 用 accent blue `rgb(122,162,247)`，unfocused 用 `rgb(59,66,97)`
- 底部 info line（在 `╰─╯` 内）：左侧 ` model · flags `、右侧 `multiline` 指示
- 布局：顶边框行 + 文本行 + info 行（theway 保留框下的 hint 行与状态分隔行）
- focused 语义：model picker / control-plane prompt 打开时视为 unfocused

## Out of scope（依赖太重，不做）

slash 高亮 / ghost text、文件引用、图片 chips、voice overlay、paste preview。

## Acceptance

- 输入框渲染为 Grok 风格圆角 chrome + `❯` + 模型名 info line
- 键盘/行为不变（测试与手动验证）
