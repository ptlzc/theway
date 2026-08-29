## 1. Core：独立 context 模块 + compact context 注入 LLM 上下文

- [x] 1.1 将 `crates/theway-core/src/agent/context.rs` 扩展为 `crates/theway-core/src/agent/context/` 目录，拆分为 `mod.rs`、`assembly.rs`、`collapse.rs`、`transform.rs`，保留现有 `virtualize_tool_results` 等导出。
- [x] 1.2 把 `build_session_context` 从 `session.rs` 迁入 `context/assembly.rs`（或由 `session.rs` 转发到该模块），保持公共 API 不变。
- [x] 1.3 在 `context/collapse.rs` 新增 `COMPACT_CONTEXT_CUSTOM_TYPE` 常量、`CompactContext` 结构体，以及 `Session::compact_context()` / `Session::collapse_node_id()` 读取 helper。
- [x] 1.4 在 `context/assembly.rs` 的 `build_session_context` 中识别 `compact_context` custom entry，解析 `compactText` 并生成一条 `AgentMessage::Llm(UserMessage)` 摘要消息。
- [x] 1.5 兼容旧数据格式：`compact_context` 解析同时接受 `data.compactText` 与 `data.text`，解析失败时跳过注入而不是报错。
- [x] 1.6 为 `build_session_context` 增加单测：含 `compact_context` 的 session 构建出的 messages 包含 compactText 的 User 消息。
- [x] 1.7 为 `Session::compact_context()` / `Session::collapse_node_id()` 增加单测。

## 2. Daemon：独立 context 模块 + system prompt 注入 lineage / handoff

- [x] 2.1 在 `crates/theway-daemon/src/` 下新增 `context/`（或 `context.rs`），统一负责 daemon 侧上下文组装：读取 compact context / collapse node id、渲染 lineage 块、调用 `compose_system_prompt`。
- [x] 2.2 修改 `crates/theway-daemon/src/system_prompt.rs::compose_system_prompt`，新增可选 `lineage: Option<&str>` 参数，非空时追加 `## Session lineage` 块。
- [x] 2.3 新增 daemon 侧 lineage 渲染 helper，根据 `CompactContext` + `collapse_node_id` 生成 handoff 文本，包含旧 session id、compact 摘要和 `session_graph_*` 工具指引。
- [x] 2.4 在 `crates/theway-daemon/src/orchestration/session.rs` 组装 system prompt 前调用 context 模块，将 lineage 传入 `compose_system_prompt`。
- [x] 2.5 为 `compose_system_prompt` 增加单测：传入 lineage 时输出包含旧 session id 与 `session_graph_read`；不传时无 lineage 块。

## 3. Storage / collapse 追加路径

- [x] 3.1 修复 `compact_context_entries` 或 `collapse_session_inner` 的 `into_session_id` 路径：将 compact_context entries 链接到目标 session 当前 leaf，并在 append 后更新 leaf 到最后一条 entry。
- [x] 3.2 保持新建 child 路径不变：`create_collapsed_child` 仍以 `parentId: null` 作为根 entry。
- [x] 3.3 增加 `into_session_id` 追加测试：目标 session 的 active branch 能读到 `compact_context`，且 `build_context()` 包含 compact 摘要。

## 4. 集成验证与提交

- [x] 4.1 增加集成测试：collapse 创建 child 后，`child.build_context()` 包含 compactText；daemon context 模块组装 system prompt 包含 lineage。
- [x] 4.2 运行 `cargo test -p theway-core -p theway-daemon -p theway-storage` 相关测试。
- [x] 4.3 运行 `cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check`。
- [ ] 4.4 按 crate 小步提交，Conventional Commits 引用 issue #53；推送前 `git fetch + rebase origin/main`。
