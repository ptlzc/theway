## Why

Session collapse 已经把旧 session 持久化为 graph 节点，但新 session 的 LLM 启动时看不到继承关系：`compact_context` 只是 custom entry，不会进入 `build_session_context` 生成的 messages，system prompt 里也没有 lineage / compactText。结果新 session 必须靠模型自己发现 `session_graph_*` 工具，容易丢失“我从哪里来、之前做到哪”的关键上下文。

## What Changes

- 让 collapse 产生的新 session 在 LLM 上下文中自动包含 compact 摘要，而不是只存 custom entry。
- 在系统提示词中注入 session lineage / handoff 块，明确告知当前 session 继承自哪个旧 session、旧 graph 如何读取/接管。
- 保留 `session_graph_*` 工具作为按需 page-in 层，不把完整旧 transcript 自动塞回上下文。
- 补充测试：collapse 后新 session 的 `build_context()` 包含 compact summary；新 session 的 system prompt 包含 lineage 指引。

## Capabilities

### New Capabilities

- `session-collapse-context-injection`: 定义 collapse 后新 session 的 LLM 上下文注入行为，包括 compact summary 进入初始消息、lineage/handoff 进入 system prompt、旧 transcript 按需读取。

### Modified Capabilities

- `session-snapshot-collapse`: 在“坍缩命令与 RPC”需求中补充新 session 的 LLM 上下文契约，明确 collapse 创建的子 session 必须能在 LLM 上下文中看到 compact 摘要与继承关系。

## Impact

- `crates/theway-core/src/agent/session/session.rs`：`build_session_context` 需要识别 `compact_context` custom entry 并生成 LLM 可见消息。
- `crates/theway-daemon/src/system_prompt.rs` / `crates/theway-daemon/src/orchestration/session.rs`：系统提示词组装时注入 lineage / handoff 块。
- `crates/theway-daemon/src/session_ops.rs`：collapse 创建 child 时可能需要补充/规范化上下文 entry。
- 测试：`crates/theway-core` 与 `crates/theway-daemon` 的 session/collapse 相关测试。
