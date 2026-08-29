## Context

Issue #53 已经把 `compact_context` 转成一条 `AgentMessage::Llm(UserMessage)`，并在 system prompt lineage 里重复携带 compactText；同时 `default_convert_to_llm` 仍然过滤所有 `Custom` 消息。这里保留 #53 的分层方向，但修正重复注入与 custom role 物化根因。参见 proposal.md 的 Why。

## Goals / Non-Goals

**Goals:**

- 让 `compaction_summary` / `branch_summary` / `collapse_context` 都能真正进入 provider 请求。
- collapse 摘要只出现一次，且位于 messages。
- daemon 上下文组装收敛到 `ContextService::load(session) -> ContextBundle`。
- session entries 继续作为 append-only canonical log。

**Non-Goals:**

- 不引入事件驱动的运行时上下文热替换。
- 不引入 Maka 式 coverage / digest / rolling checkpoint。
- 不重构 `session_graph_*` 工具。
- 不修改 session entry 持久化 schema。

## Decisions

### Decision 1: 修复 `default_convert_to_llm`，物化 known custom roles

在 `crates/theway-core/src/types.rs` 的 `default_convert_to_llm` 中增加角色映射：

| Role | Provider 形态 | 文本标记 |
|---|---|---|
| `compaction_summary` | `Message::User` | `[Previous conversation compacted]\n<summary>` |
| `branch_summary` | `Message::User` | `[Branch summary]\n<summary>` |
| `collapse_context` | `Message::User` | `[Previous session compact summary]\n<summary>` |
| 其他 custom | 过滤 | — |

理由：

- Provider message 类型没有 system role，User 是唯一可安全放前置历史摘要的角色。
- 显式标记避免摘要被误认为用户新指令。
- 未知角色保持过滤，UI-only 消息不会泄漏到 provider。

### Decision 2: `build_session_context` 生成 `collapse_context` custom message

`crates/theway-core/src/agent/context/assembly.rs` 不再直接生成 `AgentMessage::Llm(UserMessage)`，而是生成：

```rust
AgentMessage::Custom(CustomMessage {
    role: "collapse_context",
    payload: json!({ "summary": compact_text }),
    ..
})
```

这样 core 保持“语义投影”，最终 provider 形态由 `convert_to_llm` 决定，与 `compaction_summary` / `branch_summary` 同一条管线。

### Decision 3: daemon 增加 `ContextService` 与 `ContextBundle`

新增 `crates/theway-daemon/src/context/service.rs`：

```rust
pub struct ContextBundle {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
}

impl ContextService {
    pub async fn load(&self, session: &Session) -> Result<ContextBundle, SessionError> {
        // messages: session.build_context().await?.messages
        // system_prompt: compose_system_prompt(cwd, memory, tools, lineage_identity_only)
    }
}
```

`orchestration/session.rs` 只调用 `ContextService::load`，不再自行调用 `session.compact_context()` / `render_lineage` / `compose_system_prompt` 组合。

### Decision 4: system prompt lineage 只保留 identity 与工具指引

`lineage.rs` 的 `render_lineage` 不再输出 `Previous context summary: <compactText>`，只输出：

```text
## Session lineage

Collapse node: <node_id>
This session continues from <old_session_id>.
Use session_graph_list / session_graph_read / session_graph_status / session_graph_wait / session_graph_attach to inspect or take over the old session graph.
```

compactText 只经 `collapse_context` 消息注入一次。

### Decision 5: 保持 projection 不变式

`ContextService` 和 `build_session_context` 只读 session entries，绝不写回。collapse 写入仍是 `session_ops.rs` 的职责，context 模块是纯投影。

## Risks / Trade-offs

- [User role 摘要仍可能被模型当作指令] → 统一 `[Previous ... compacted/summary]` 前缀，测试断言该前缀存在；如后续引入 provider system role 再迁移。
- [变更 `default_convert_to_llm` 会影响现有 compaction 行为] → 这是有意修复；补充 `types` 单测覆盖三种已知角色和未知角色。
- [ContextBundle 初始 messages 与 harness rehydrate 的时序] → `ContextService::load` 直接复用 `session.build_context()`，与现有 rehydrate 路径一致，不在 service 内另做缓存。
- [移除 system prompt 中的 compactText 可能减少“一眼可见”摘要] → messages 中已有完整摘要，system prompt 保留 identity 指引足够。

## Migration Plan

1. 先合并 core 的 `convert_to_llm` 与 `collapse_context` custom message 变更。
2. 再合并 daemon `ContextService` / `ContextBundle` 与 lineage 精简。
3. 更新受影响测试与断言。
4. 不迁移任何已持久化 session；旧 child 的 `compact_context` entry 继续被新投影读取。

## Open Questions

- 无。
