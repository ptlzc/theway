## Context

当前 collapse 是一条链路：`collapse_session_inner` 从请求 summary 或 `latest_compaction_summary()` 生成 compact_text；`compact_context_entries` 写一个 `compact_context` custom entry；`make_collapse_node` 不写 `parent_id` / `child_ids`。因此 S → C1 → C2 时，C2 的摘要链断裂，node 链无法表达。本设计补齐嵌套坍缩语义。

## Goals / Non-Goals

**Goals:**

- 嵌套坍缩可无限进行，每层保留固定组件的有界摘要。
- node 链可在 Turso session graph 中遍历。
- lineage 是事件型，只给 node/session id。
- 保持 session entries 不可变，原始 transcript 永不合并。

**Non-Goals:**

- 不实现摘要质量评估或自动质量门禁。
- 不改变 compaction 算法本身。
- 不自动把旧 transcript 注入新 session。
- 不引入全局跨 cwd session graph。

## Decisions

### Decision 1: Rolling summary 组件固定为五个

```text
goal
completed work
key decisions
next steps
critical context
```

在 `collapse_session_inner` 中生成 `compact_text` 时使用该结构；每个组件由对应源材料推导，并有固定字符上限（例如 `CRITICAL_CONTEXT` 上限略高，其余较低）。具体上限由 daemon 常量定义，不写进 wire 契约。

### Decision 2: 摘要输入优先级

当 `request.summary` 非空时直接使用；否则按以下优先级生成：

1. 源 session 最新的 `Compaction` summary；
2. 源 session 最新的 `compact_context.compactText`；
3. 若均不存在，只对源 session 当前 transcript 生成五组件摘要。

嵌套坍缩时，第 2 项保证 C1 的上一代摘要进入 C2 的 rolling 输入。新增 `Session::latest_collapse_summary()` helper（先查 compaction，再查 compact_context）。

### Decision 3: node 父子边

`make_collapse_node` 增加 `parent_id` 参数：

- 读取源 session 的 `collapseNodeId` metadata；
- 若有，写入 node.parent_id；
- 保存 node 后更新父节点的 `child_ids`，追加当前 node id。

`SessionGraphStore` 增加 `link_child(parent_id, child_id)` 或 `save_node` 内部处理 edges；保持重启后仍可遍历。

### Decision 4: 事件型 lineage

`render_lineage` 只输出：

```text
## Session lineage

Collapse event:
  node id: <node_id>
  source session id: <source_id>
```

不包含 summary 和工具指引。模型按 `session_graph_list` / `session_graph_read` 自行查询。

### Decision 5: 提示词与实现对齐

`<harness>` 的 Collapse model 保持现有描述：固定五组件、有界、允许精度损失、lineage 只记录事件与 id。后续修改实现时同步更新该文本。

## Risks / Trade-offs

- [固定组件可能不适合所有会话] → 组件只约束结构，组件内容可以为空；摘要器按可用的源材料填充。
- [`child_ids` 更新失败会破坏链] → 保存 node 与更新父节点在同一 Turso 事务中完成；失败不返回成功。
- [摘要上限过小导致信息不足] → 原始 transcript 始终可按 node 链读取，精度损失是可接受的。
- [旧数据无父子边] → 无父边视为根节点，读取逻辑退化为逐节点列出。

## Migration Plan

1. 先增加 rolling summary helper 与测试。
2. 再增加 node parent/child 持久化与测试。
3. 最后接入 `collapse_session_inner` 与 lineage 投影。
4. 不迁移已持久化 session；历史节点仍可读取，只是没有新 parent/child 边。

## Open Questions

- 无。
