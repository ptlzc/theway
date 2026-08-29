## Context

现有 collapse 已经持久化 `compact_context` custom entry 和 `session_graph_state`，但 `build_session_context` 不消费 `Custom`，导致新 session 的 LLM 看不到 compact 摘要；system prompt 组装也没有 session lineage 信息。上下文组装逻辑目前散落在 `session.rs`、`context.rs`、`system_prompt.rs` 中，缺少一个统一的 context 模块来管理“如何组装、如何装载、如何卸载”。参见 proposal.md 的 Why。

## Goals / Non-Goals

**Goals:**

- collapse 子 session 的 LLM 初始上下文自动包含旧 session 的 compact 摘要。
- system prompt 自动注入 lineage / handoff 指引，让 LLM 知道旧 session 和 `session_graph_*` 工具。
- 兼容已存在的 collapse 子 session（只含 `compact_context` custom entry，无需迁移 transcript）。
- 保持原始 transcript 按需读取，不自动注入完整旧消息。
- 将上下文组装/装载/卸载收敛到独立 `context` 模块，避免继续散落在 session 与 system prompt 各处。

**Non-Goals:**

- 不重写 compaction 算法。
- 不改变 `dag_*` 的 session 隔离语义。
- 不自动把完整旧 transcript 或全部 session graph 节点塞进 LLM 上下文。
- 不修改 `session_graph_*` 工具本身的读取能力。

## Decisions

### Decision 1: 新增独立 context 模块，`build_session_context` 将 `compact_context` custom entry 转成 LLM User 消息

将 `crates/theway-core/src/agent/context.rs` 从单文件扩展为 `crates/theway-core/src/agent/context/` 目录，统一承载上下文组装与装卸载逻辑：

- `mod.rs`：重新导出公共 API。
- `assembly.rs`：从 session entries 组装 `AgentMessage`（即现有 `build_session_context` 的逻辑迁入）。
- `collapse.rs`：`compact_context` 解析与摘要消息注入。
- `transform.rs`：现有 `virtualize_tool_results` 等上下文变换/卸载逻辑。

在 `assembly.rs` 的 `build_session_context` 中识别 `custom_type == "compact_context"`，解析 `data.compactText`，并生成一条 `AgentMessage::Llm(UserMessage)`（文本形式类似 `[Previous session compact summary]\n{compactText}`），而不是 `AgentMessage::Custom`。

理由：

- 当前 `default_convert_to_llm` 只保留 `AgentMessage::Llm`，Custom 会被过滤；用 User 消息可以确保真正进入 LLM request。
- 已存在的 collapse 子 session 已包含 `compact_context`，无需迁移数据。
- 不改变现有 `/compact` 的 `Compaction` 路径。

备选方案：

- 新增 `CustomMessage` 并修改 `default_convert_to_llm` 翻译自定义角色。影响面更大，且需要同步处理现有 compaction_summary 的潜在过滤问题。
- 在 collapse 创建 child 时额外写一条 `Compaction` entry。对旧 child 不兼容，且语义不准确（child 本身并没有发生 compaction）。

### Decision 2: 新增 `Session::compact_context()` 与 `Session::collapse_node_id()` 读取 helper

在 core 的 `Session` facade 上新增：

- `compact_context() -> Result<Option<CompactContext>, SessionError>`：从 entries 中读取最新 `compact_context` custom entry。
- `collapse_node_id() -> Result<Option<String>, SessionError>`：从 metadata 读取 `collapseNodeId`。

这样 daemon 组装 system prompt 时不需要直接依赖存储层细节。

### Decision 3: daemon 侧新增 context 模块，`compose_system_prompt` 增加可选 lineage 块

在 `crates/theway-daemon/src/` 下新增 `context/`（或 `context.rs`），负责 daemon 侧上下文组装：读取 session 的 compact context / collapse node id、渲染 lineage 块、调用 `compose_system_prompt`。`system_prompt.rs` 保留为基础 prompt 渲染，或薄封装到 context 模块。

修改 `crates/theway-daemon/src/system_prompt.rs`：

```rust
pub fn compose_system_prompt(
    cwd: &Path,
    memory: &str,
    tool_names: &[String],
    lineage: Option<&str>,   // 新增
) -> String
```

当 `lineage` 非空时，追加一段：

```text
## Session lineage

This session continues from <old_session_id>.
Previous context summary: <compactText>
Use session_graph_list / session_graph_read / session_graph_status / session_graph_wait / session_graph_attach to inspect or take over the old session graph.
```

daemon 在 `orchestration/session.rs` 中先调用 context 模块的读取 helper（内部使用 `session.compact_context().await` 与 `session.collapse_node_id().await`），拼出 `lineage` 文本，再传入 `compose_system_prompt`。

### Decision 4: 修复 `into_session_id` 追加路径的 entry 链接

当前 `compact_context_entries` 生成的第一条 entry `parentId: null`，`append_entries` 不会更新 leaf。直接 append 到已有 session 时，新 entry 不会出现在 active branch，LLM 上下文也读不到。

修改 `compact_context_entries` 接受可选 `parent_id`；当 `into_session_id` 存在时，从目标 session 读取当前 leaf 作为第一条 entry 的 parent，并在 append 后把 leaf 更新到最后一条 entry。新建 child 时仍以 `null` 为根。

### Decision 5: 测试策略

- core 单测：构造含 `compact_context` 的 entries，断言 `build_session_context().messages` 包含 compactText 的 User 消息。
- daemon 单测：`compose_system_prompt` 传入 lineage 块时输出包含旧 session id 与工具名；不传时无 lineage 块。
- 集成测试：collapse 后新 session `build_context()` 包含摘要；`into_session_id` 追加到已有 session 后 active branch 可读到 compact_context。

## Risks / Trade-offs

- [把 compact 摘要作为 User 消息可能被模型当作新的用户指令] → 在文本前加明确标记（`[Previous session compact summary]`），并放入 system prompt lineage 块说明这是历史摘要。
- [旧 child 的 `compact_context` 数据格式不统一（测试中存在 `data.text` 与生产 `data.compactText`）] → `compact_context()` 解析时兼容 `compactText` 与 `text` 两种字段，解析失败返回 None。
- [`into_session_id` 的 leaf 更新涉及存储层语义] → 只在该路径内修正，不改变新建 child 的现有行为；如果发现风险过大，可先只保证新建 child，`into_session_id` 作为独立任务处理。
- [system prompt 变长] → lineage 块只包含摘要和引用，不包含 raw transcript；摘要为空时跳过整个块。

## Migration Plan

1. 先合并 core 的 `build_session_context` 解析，老 child 无需迁移即可获得摘要注入。
2. 再合并 daemon system prompt lineage 注入。
3. 最后修复 `into_session_id` 追加路径。
4. 全程保持旧 session transcript 不变；新写入只影响后续 collapse 创建的 child 或 append 的目标 session。

## Open Questions

- 是否需要同时修复 `default_convert_to_llm` 对 `compaction_summary` / `branch_summary` 等 Custom 消息的过滤？这影响现有 `/compact` 是否真正把摘要发给 LLM，但与本次 collapse 上下文注入可独立处理。
