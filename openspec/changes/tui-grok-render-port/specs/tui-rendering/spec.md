# Capability: tui-rendering

## Purpose

`theway-tui` 的渲染层由三个来源组成:daemon 经 gRPC 推送的结构化
`FeedBlock` 数据（快照 + 事件两路）、Grok Build 移植的渲染原语 crate
（`theway-markdown-core` / `theway-ratatui-textarea` /
`theway-pager-render`, Apache-2.0 保留 NOTICE）、以及 theway-tui 自身的
布局/交互代码。事件面与快照面共用同一渲染入口。

## ADDED Requirements

### Requirement: Stream events apply as increments

TUI 对 `StreamFrame` 的 `event` 帧做增量应用, 不再整帧丢弃; 无法增量的
事件仍走 snapshot 路径, 但不产生整表重建。

#### Scenario: Feed update via event frame
- **WHEN** daemon 推送 feed 相关的事件帧
- **THEN** TUI 按尾部 diff 追加/替换对应 `FeedBlock`, 未变化的块不重新布局

#### Scenario: Frame throttling
- **WHEN** 多个事件帧在同一渲染周期内到达
- **THEN** TUI 合并到下一个 100ms tick 统一绘制, 每个事件不单独触发一帧

### Requirement: Markdown analysis matches the ported core

feed 块的 markdown 语义（GFM、删除线、math、任务列表）由
`theway-markdown-core` 判定, 判定口径与其来源 Grok Build 一致:单波浪线
`~text~` 不删除线, 仅 `~~…~~` 删除。

#### Scenario: Single-tilde pair
- **WHEN** feed 文本包含 `~**10%**`
- **THEN** 渲染为字面波浪线文本, 无删除线

#### Scenario: Code block rendering
- **WHEN** feed 文本包含 ``` 代码块
- **THEN** 块边界与高亮样式按 markdown 分析结果渲染, 不按行拼接猜测

### Requirement: Input editing uses the ported textarea

输入组件来自 `theway-ratatui-textarea`（Grok Build fork 的 tui-textarea）;
上游 `tui-textarea` 依赖不再出现在 `theway-tui`。

#### Scenario: Editor dependency
- **WHEN** 检查 `crates/theway-tui/Cargo.toml`
- **THEN** 无 `tui-textarea` 依赖, 输入组件引用 `theway-ratatui-textarea`

#### Scenario: Existing key bindings preserved
- **WHEN** 用户使用历史翻页、补全、多行输入（≤6 行）、粘贴
- **THEN** 行为与切换前一致（移植阶段不引入新编辑行为）

### Requirement: Render primitives carry license notices

每个移植 crate 保留捐赠方版权头, 并携带 `NOTICE`（捐赠方、源 revision、
上游出处）; 移植后修改过的文件带 Apache §4(b) 变更声明。

#### Scenario: Notice file present
- **WHEN** 检查任一 `theway-*` 移植 crate 根目录
- **THEN** 存在 `NOTICE` 文件, 其中包含捐赠方 SpaceXAI 与源 revision `5d08d7e`

#### Scenario: No donor prefix leakage
- **WHEN** 在 `crates/` 下 grep `xai-` 前缀
- **THEN** 除 NOTICE/许可证声明中提及捐赠方外无残留（crate 名、包名、路径）
