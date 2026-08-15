# tui-composer-single-line-wrap

Issue: #40

## Problem

composer 行数由 `input_display_lines()` 决定，它只数 `\n` 与 paste chip：
单行输入（无换行）时 composer 恒为 1 行高。textarea 在 1 行视口内垂直滚动
跟随光标——文本超出宽度后只看得见光标所在的一小段，行首被裁掉。

textarea 内核已具备所需能力，问题只在 TUI 的行数计算没用它：

- `wrapped_lines(width)`：FirstFit + `break_words` 折行——长词/CJK 在列边界
  断行，即"字符换行"语义。
- `desired_height(width)`：直接给出折行后的视觉行数。

## What changes

`ui/mod.rs` 的 `composer_rows()` 从"数 `\n`"改为"按宽度算视觉折行"：

- 输入区内容宽度 = input 区宽度 − chrome 左侧 padding(2) − 右侧 padding(1) −
  `❯ ` 前缀(2)，即 `input_area_width.saturating_sub(5)`。
- `composer_rows(input_area_width)` 用 `self.input.desired_height(content_width)`
  计算视觉折行数，clamp 到 `1..MAX_INPUT_ROWS`（6）。
- 溢出 6 行时 textarea 会显示 scrollbar（`content_width` 预留 1 列）——按
  `desired_height` 先算满宽行数，超过 6 行则用 `width − 1` 复核（与内核
  `content_width()` 的 scrollbar 语义对齐）。
- `manual_composer_rows`（拖拽覆盖）优先，语义不变；render() 调用点传入
  `frame.area().width`。
- `input_display_lines()` 保留，`input_is_single_line()` 仍按逻辑行（无 `\n`）
  判定——折行不影响历史导航（Up/Down）、slash 补全、Enter 提交。
- 不改 textarea 内核。

## Out of scope

- 不引入 word wrap（保留内核现有 FirstFit 折行语义）。
- 不改变 MAX_INPUT_ROWS 上限与拖拽调高逻辑。

## Acceptance

- 单行长文本（中文/英文）在 composer 内折行显示，行首可见；宽度变化
  （终端 resize）时行数跟随重算。
- 有 `\n` 的多行草稿、拖拽调高、历史导航、slash 补全行为不变。
- `make check` / `make test` / `make lint` / `make fmt-check` 通过。
