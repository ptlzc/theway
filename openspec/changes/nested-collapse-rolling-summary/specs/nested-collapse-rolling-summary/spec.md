## Purpose

定义嵌套坍缩链：每次坍缩生成固定组件的有界 rolling summary，collapse node 形成父子链，lineage 只记录坍缩事件与 id，完整历史由模型通过 session graph 工具按需读取。

## ADDED Requirements

### Requirement: 固定组件的有界 rolling summary

系统 SHALL 为每次 collapse 生成一个 summary，组件固定为 `goal`、`completed work`、`key decisions`、`next steps`、`critical context`，每个组件 SHALL 有界且允许精度损失。

#### Scenario: 首次坍缩生成五组件摘要

- **WHEN** 普通 session S 执行 collapse
- **THEN** 生成的 compact summary 包含 `goal` / `completed work` / `key decisions` / `next steps` / `critical context` 五类信息
- **AND** 每类信息长度受固定上限约束

#### Scenario: 嵌套坍缩继承上一代摘要

- **WHEN** collapse child C1（已含上一代 compact summary）再次 collapse 生成 C2
- **THEN** C2 的 rolling summary 以 C1 的 compact summary 作为输入
- **AND** C2 的 summary 仍使用相同的五个固定组件与长度上限

### Requirement: Collapse node 父子链

每次 collapse 注册的 node SHALL 记录 `parent_id`（源 session 的 `collapseNodeId`，如存在）并更新父节点的 `child_ids`，形成可遍历的 node 链。

#### Scenario: 嵌套坍缩形成链

- **WHEN** S 坍缩为 C1，随后 C1 坍缩为 C2
- **THEN** node2.parent_id 指向 node1
- **AND** node1.child_ids 包含 node2
- **AND** `session_graph_list` 可返回链上全部节点

### Requirement: 事件型 lineage

Lineage block SHALL 只包含 collapse event 的 node id 与 source session id，不包含 summary 文本与工具指引。

#### Scenario: 只给事件与 id

- **WHEN** 系统为 collapse child 渲染 lineage
- **THEN** lineage 包含 `node id` 与 `source session id`
- **AND** 不包含 compactText 或 `session_graph_*` 工具说明

### Requirement: 完整历史按需读取

嵌套坍缩 SHALL 不将旧 transcript 合并进新 session；模型 SHALL 通过 `session_graph_read` / `session_graph_list` 沿 node 链按需读取原始 transcript。

#### Scenario: 按链读取历史

- **WHEN** 模型需要读取更早的坍缩历史
- **THEN** 可通过 `session_graph_list` 发现 node 链
- **AND** 可用 `session_graph_read(nodeId)` 分别读取每一代 session 的原始 transcript
