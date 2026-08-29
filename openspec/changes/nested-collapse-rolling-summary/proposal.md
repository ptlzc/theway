## Why

Issue #53/#54 定义了 collapse 与 prompt 语义，但当前实现只支持一跳坍缩：嵌套坍缩时 `latest_compaction_summary` 读不到上一代 `compact_context`，collapse node 没有父子边，lineage 不形成链。需要把“无限坍缩 + 固定组件的有损 rolling summary + 事件型 lineage”落成正式契约。

## What Changes

- 每次坍缩生成一个有界 rolling summary，固定组件为 `goal`、`completed work`、`key decisions`、`next steps`、`critical context`。
- 嵌套坍缩时，以源 session 的上一代 `compact_context.compactText` 作为 rolling 输入之一。
- Collapse node 写入 `parent_id` / `child_ids`，形成 node 链。
- Lineage block 只输出 collapse event + node id / source session id；完整内容由 LLM 通过 `session_graph_*` 自行查询。
- Harness prompt 的 Collapse model 与实现保持一致。

## Capabilities

### New Capabilities

- `nested-collapse-rolling-summary`: 定义嵌套坍缩链、有界固定组件 rolling summary、事件型 lineage、按需读取完整 transcript 的行为契约。

### Modified Capabilities

<!-- 不修改既有 main spec；本 change 定义嵌套坍缩语义，归档时与 #53/#54 一起合并。 -->

## Impact

- `crates/theway-core/src/multiagent/session_graph.rs`：collapse material 支持上一代 summary 链。
- `crates/theway-core/src/agent/session/session.rs`：`latest_compaction_summary` 或新 helper 回退到 `compact_context`。
- `crates/theway-daemon/src/session_ops.rs`：rolling summary 生成、node 父子边写入、lineage 投影。
- `crates/theway-storage/src/session_graph.rs`：node parent/child 更新接口（如缺失）。
- `crates/theway-daemon/src/context/lineage.rs` / `system_prompt.rs`：事件型 lineage 与 prompt 语义对齐。
- 测试：core session、daemon session_ops、context prompt、storage session graph。
