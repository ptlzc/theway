## Why

Issue #53 解决了“collapse 后 compact 摘要能进入 LLM 上下文”，但实现评审发现三个问题：compactText 被重复注入（初始消息 + system prompt lineage）；`build_session_context` 硬编码 collapse 专用逻辑；`default_convert_to_llm` 过滤所有 `Custom` 消息，导致现有 `compaction_summary` / `branch_summary` 可能从未真正发给模型。需要把上下文投影收敛为单一、可测试的管线。

## What Changes

- 修复 `default_convert_to_llm`，将已知 custom roles（`compaction_summary`、`branch_summary`、`collapse_context`）物化为 provider 可见消息，未知角色继续过滤。
- collapse 摘要只注入一次：进入 messages，system prompt 只保留 lineage identity 与 `session_graph_*` 工具指引。
- 新增 daemon 侧 `ContextService::load(session) -> ContextBundle`，作为组装 messages 与 system prompt 的唯一入口。
- 保持 session entries 为 append-only canonical log；所有上下文变化都是 projection，不修改历史。

## Capabilities

### New Capabilities

- `context-projection`: 定义 session entries → LLM 上下文的投影管线，包括 custom role 物化、collapse 摘要单次注入、ContextBundle 单入口、以及投影不修改 canonical log 的约束。

### Modified Capabilities

<!-- 不修改既有 main spec；本 change 定义新的投影层，后续与 #53 归档时统一合并。 -->

## Impact

- `crates/theway-core/src/types.rs`：`default_convert_to_llm` 物化已知 custom roles。
- `crates/theway-core/src/agent/context/assembly.rs`：compact summary 生成 `collapse_context` custom message。
- `crates/theway-daemon/src/context/`：新增 `ContextService` / `ContextBundle`；`lineage.rs` 移除 compactText，只保留 identity 与工具指引。
- `crates/theway-daemon/src/orchestration/session.rs`：改用 `ContextService::load`。
- 测试：`types` 转换测试、core session 测试、daemon context 测试更新。
