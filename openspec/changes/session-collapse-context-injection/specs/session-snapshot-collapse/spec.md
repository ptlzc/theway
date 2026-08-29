## MODIFIED Requirements

### Requirement: 坍缩命令与 RPC

系统 SHALL 提供 `/collapse` 命令与 `CollapseSession` RPC，将当前 session 变成图节点、创建 compact 上下文的新 session，并保留旧 session 的原始 transcript 与 subagent graph。新 session 的 LLM 上下文 SHALL 自动包含 compact 摘要与 lineage / handoff 指引。

#### Scenario: 坍缩当前 session

- **WHEN** 用户对 session S 执行 `/collapse`
- **THEN** 创建新 session C
- **AND** S 写入 `Compaction` entry
- **AND** 系统注册 `SessionGraphNode` 指向 S 与 C
- **AND** S 名下运行中的 DAG/subagent 不被取消

#### Scenario: 坍缩后的新 session 携带 LLM 上下文

- **WHEN** 用户对 session S 执行 `/collapse` 并创建新 session C
- **THEN** C 的 LLM 初始上下文包含 S 的 compact 摘要
- **AND** C 的 system prompt 包含 lineage / handoff 块，说明 C 继承自 S 且可通过 `session_graph_*` 读取或接管旧 graph
- **AND** C 的 LLM 上下文不包含 S 的完整原始 transcript

#### Scenario: 接管运行中的 graph

- **WHEN** 用户执行 `/collapse --adopt`
- **THEN** S 名下活跃 DAG/subagent 的所有权迁移到 C
- **AND** S 的 graph 节点标记为已接管
