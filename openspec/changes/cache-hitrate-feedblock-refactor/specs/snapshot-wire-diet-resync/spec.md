## MODIFIED Requirements

### Requirement: GetHistory 分页读取 feed

系统 SHALL 提供 `GetHistory` 按 `offset/limit` 分页读取 session feed，返回与 `SessionFeed` 相同的结构化 `FeedBlock`。该 RPC SHALL 用于历史恢复与 collapse node 原始 transcript 读取。返回的 `FeedBlock` SHALL 使用 first-class `tool_call` / `error` variants，而不是 `tool` / error-prefixed `plain`。

#### Scenario: 分页恢复长会话

- **WHEN** 客户端需要恢复长会话
- **THEN** 客户端可多次调用 `GetHistory` 按页读取
- **AND** 每页返回 `next_offset`
- **AND** 不要求一次传输完整 `SessionSnapshot`

#### Scenario: 历史中的 tool call 与 error 使用新 variants

- **WHEN** 客户端通过 `GetHistory` 读取包含工具调用或错误的 feed
- **THEN** 工具调用以 `tool_call` 表示
- **AND** 错误以 `error` 表示
- **AND** 不出现 `tool` 或 `error:` 前缀的 `plain`

### Requirement: graph node 流式输出复用 FeedBlock

`StreamSessionGraphNode` 与 `ListSessionGraphNodeMessages` SHALL 使用 `FeedBlock` 作为结构化输出格式，与 session feed 保持一致。该输出 SHALL 使用 first-class `tool_call` / `error` variants。

#### Scenario: 节点输出与 feed 同构

- **WHEN** 客户端读取 DAG_NODE 或 SUBAGENT_JOB 的输出
- **THEN** 返回的块结构与 session feed 的 `FeedBlock` 相同
- **AND** 客户端可复用同一套渲染逻辑

#### Scenario: 节点输出中的工具调用与错误

- **WHEN** 节点输出包含工具调用或错误
- **THEN** 工具调用以 `tool_call` 表示
- **AND** 错误以 `error` 表示
