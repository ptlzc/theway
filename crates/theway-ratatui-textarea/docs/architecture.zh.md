# Textarea 架构

[English](architecture.md) | 中文

## 职责

`theway-ratatui-textarea` 负责可复用文本编辑与 widget 行为。嵌入应用提供外围表单、命令路由、clipboard 实现、主题，以及 atomic text element 事件的业务含义。

## 编辑引擎

[`editor.rs`](../src/editor.rs) 包含 `EditBuffer`、命令分类、edit plan 和校验后应用。Cursor position 与 replacement range 规范化到 Unicode grapheme boundary。Atomic byte range 在规划前也会规范化，因此 cursor 移动与删除不能拆分应用定义的 element。

`EditPlan` 创建时捕获 buffer identity 与 generation。对过期或其他 buffer 的 plan 执行应用会失败，不会修改错误文本状态。编辑结果同时携带文本 delta 与 cursor 结果，方便上层显式更新依赖范围。

## Widget 状态与交互

[`textarea.rs`](../src/textarea.rs) 暴露 widget 与状态。[`textarea/model.rs`](../src/textarea/model.rs)、[`textarea/navigation.rs`](../src/textarea/navigation.rs)、[`textarea/mouse.rs`](../src/textarea/mouse.rs)、[`textarea/elements_wrap.rs`](../src/textarea/elements_wrap.rs) 和 [`textarea/history.rs`](../src/textarea/history.rs) 拆分对应机制，但保持一个公开归属边界。

[`textarea/history.rs`](../src/textarea/history.rs) 记录 undo/redo 状态，并支持把多个编辑组成一个用户动作。[`textarea/mouse.rs`](../src/textarea/mouse.rs) 将终端坐标映射为文本位置与 selection action。Clipboard 操作使用调用方提供的 `ClipboardProvider` 或内部 fallback。

## Wrap 与渲染

[`wrapping.rs`](../src/wrapping.rs) 将逻辑文本与 styled span 映射为可视行，同时保留 grapheme boundary、终端 display width 和源位置。[`render/mod.rs`](../src/render/mod.rs) 从状态绘制内容、selection、cursor 与 scrollbar，不修改编辑模型。

## 边界与不变量

- Cursor 与 edit boundary 是始终落在 grapheme boundary 的 UTF-8 字节 offset。
- Atomic text element 以不可拆分 range 移动、选择和删除。
- Plan 只能修改创建它的 buffer identity 与 generation。
- Wrap 保留样式，并按终端 display width 把 visual cell 映射回 logical position。
- Windows 上按需把 Ctrl+Alt 区分为 AltGr；其他平台使用其组合输入行为。
