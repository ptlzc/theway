## Purpose

定义 session collapse 后新 session 的 LLM 上下文注入契约：新 session 自动感知继承的旧 session、compact 摘要与可用的 session graph 读取/接管工具，同时保持原始 transcript 按需读取。

## ADDED Requirements

### Requirement: Collapse 子 session 包含 compact 摘要的 LLM 可见上下文

系统 SHALL 在 collapse 创建的新 session 的 LLM 上下文初始消息中包含旧 session 的 compact 摘要，使模型无需调用工具即可知道“之前的会话总结”。

#### Scenario: 新 session 的初始上下文包含 compact summary

- **WHEN** 一个 session S 执行 collapse 并创建子 session C
- **THEN** C 的 `build_context()` 返回的 messages 中包含一条携带 `compactText` 的摘要消息
- **AND** 该消息不依赖 C 中后续新增的对话消息

#### Scenario: 已存在的 collapse 子 session 恢复时仍能注入

- **WHEN** 客户端恢复一个由旧版本 collapse 创建、只含 `compact_context` custom entry 的子 session
- **THEN** 系统仍能将该 entry 中的 `compactText` 转换为 LLM 可见的摘要消息
- **AND** 不需要修改已持久化的 session transcript

### Requirement: 新 session 的 system prompt 包含 session lineage 与 handoff 指引

当 session 存在 collapse 继承关系时，系统 SHALL 在 system prompt 中注入 lineage / handoff 块，说明当前 session 继承自哪个旧 session、旧 graph 如何读取/监控/接管。

#### Scenario: 启动时注入 lineage 块

- **WHEN** daemon 为 collapse 子 session 组装 system prompt
- **THEN** system prompt 包含旧 session id / node id
- **AND** 包含 compact 摘要或摘要引用
- **AND** 说明可使用 `session_graph_list` / `session_graph_read` / `session_graph_status` / `session_graph_wait` / `session_graph_attach` 访问旧 graph

#### Scenario: 非 collapse 会话不注入 lineage 块

- **WHEN** daemon 为一个普通 session 组装 system prompt
- **THEN** system prompt 不包含 collapse lineage / handoff 块

### Requirement: 原始 transcript 保持按需读取

系统 SHALL 不将旧 session 的完整原始 transcript 自动注入新 session 的 LLM 上下文；完整内容 SHALL 通过 `session_graph_read` 或等价分页接口按需读取。

#### Scenario: compact 摘要注入但不携带原始 transcript

- **WHEN** collapse 创建的新 session 构建 LLM 上下文
- **THEN** 上下文中包含 compact 摘要
- **AND** 不包含旧 session 的全部原始消息
- **AND** 调用 `session_graph_read(nodeId)` 可以分页读取旧 transcript
