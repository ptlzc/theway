## 1. Core：compact context 注入 LLM 上下文

- [ ] 1.1 在 `crates/theway-core/src/agent/session/session.rs` 新增 `COMPACT_CONTEXT_CUSTOM_TYPE` 常量、`CompactContext` 结构体，以及 `Session::compact_context()` / `Session::collapse_node_id()` 读取 helper。
- [ ] 1.2 更新 `build_session_context`，在遍历 entries 时识别 `compact_context` custom entry，解析 `compactText` 并生成一条 `AgentMessage::Llm(UserMessage)` 摘要消息。
- [ ] 1.3 兼容旧数据格式：`compact_context` 解析同时接受 `data.compactText` 与 `data.text`，解析失败时跳过注入而不是报错。
- [ ] 1.4 为 `build_session_context` 增加单测：含 `compact_context` 的 session 构建出的 messages 包含 compactText 的 User 消息。
- [ ] 1.5 为 `Session::compact_context()` / `Session::collapse_node_id()` 增加单测。

## 2. Daemon：system prompt 注入 lineage / handoff

- [ ] 2.1 修改 `crates/theway-daemon/src/system_prompt.rs::compose_system_prompt`，新增可选 `lineage: Option<&str>` 参数，非空时追加 `## Session lineage` 块。
- [ ] 2.2 新增 daemon 侧 lineage 渲染 helper，根据 `CompactContext` + `collapse_node_id` 生成 handoff 文本，包含旧 session id、compact 摘要和 `session_graph_*` 工具指引。
- [ ] 2.3 在 `crates/theway-daemon/src/orchestration/session.rs` 组装 system prompt 前读取 `session.compact_context()` / `session.collapse_node_id()`，将 lineage 传入 `compose_system_prompt`。
- [ ] 2.4 为 `compose_system_prompt` 增加单测：传入 lineage 时输出包含旧 session id 与 `session_graph_read`；不传时无 lineage 块。

## 3. Storage / collapse 追加路径

- [ ] 3.1 修复 `compact_context_entries` 或 `collapse_session_inner` 的 `into_session_id` 路径：将 compact_context entries 链接到目标 session 当前 leaf，并在 append 后更新 leaf 到最后一条 entry。
- [ ] 3.2 保持新建 child 路径不变：`create_collapsed_child` 仍以 `parentId: null` 作为根 entry。
- [ ] 3.3 增加 `into_session_id` 追加测试：目标 session 的 active branch 能读到 `compact_context`，且 `build_context()` 包含 compact 摘要。

## 4. 集成验证与提交

- [ ] 4.1 增加集成测试：collapse 创建 child 后，`child.build_context()` 包含 compactText；system prompt 组装包含 lineage。
- [ ] 4.2 运行 `cargo test -p theway-core -p theway-daemon -p theway-storage` 相关测试。
- [ ] 4.3 运行 `cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check`。
- [ ] 4.4 按 crate 小步提交，Conventional Commits 引用 issue #53；推送前 `git fetch + rebase origin/main`。
