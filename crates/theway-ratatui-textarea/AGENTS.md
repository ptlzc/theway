# Textarea 修改规则

本文件适用于 `crates/theway-ratatui-textarea/`。同时遵循 [`../../AGENTS.md`](../../AGENTS.md) 和 [`docs/architecture.md`](docs/architecture.md)。

## 归属

- 应用命令、会话状态、daemon 协议与主题选择不得进入本 crate。
- 所有外部 cursor 和 edit range 保留 grapheme boundary 规范化。
- Atomic element range 在移动、selection、删除、wrap 和鼠标 hit test 中都不可拆分。
- 扩展 `EditPlan` 时保留 buffer identity 与 generation 校验。
- 可视列使用终端 display width，logical-to-visual mapping 保留样式。

## 交互

- Key 解释统一经过 [`classify_key_event`](src/editor_keys.rs)，避免 editor 与 widget 行为分叉。
- Undo grouping 对齐一次用户可见编辑动作；只有提交新分支时才清除 redo history。
- 保留 [`src/lib.rs`](src/lib.rs) 的平台专用 AltGr 区分。
- Atomic element 交互通过 `TextElementEvent` 报告，其业务含义由嵌入应用决定。

## 兼容性

- 代码来源细节保留在 [`NOTICE`](NOTICE)。
- 公开交互契约变化时更新 demo。
- 多文件测试套件遵循 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) 的镜像布局。

## 验证

运行 `cargo test -p theway-ratatui-textarea`、`cargo check -p theway-ratatui-textarea --example textarea_demo` 和 `cargo doc -p theway-ratatui-textarea --no-deps --document-private-items`。
