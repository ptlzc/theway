## Purpose

定义 session entries 到 LLM 上下文的投影契约：known custom roles 正确物化、collapse 摘要单次注入、daemon 通过单一 ContextService 组装 ContextBundle，且投影永不修改 append-only session log。

## ADDED Requirements

### Requirement: Known custom roles 物化为 provider 消息

系统 SHALL 在 `default_convert_to_llm` 中将 `compaction_summary`、`branch_summary`、`collapse_context` 三种 custom role 物化为 provider 可见消息；未知 custom role SHALL 继续被过滤。

#### Scenario: compaction summary 进入 provider 请求

- **WHEN** agent messages 包含 role 为 `compaction_summary` 的 custom message
- **THEN** `convert_to_llm` 输出中包含一条带 summary 文本的 provider 消息
- **AND** 消息带明确的历史摘要标记，不与用户新指令混淆

#### Scenario: 未知 custom role 被过滤

- **WHEN** agent messages 包含未知 custom role
- **THEN** `convert_to_llm` 输出中不包含该消息

### Requirement: Collapse 摘要单次注入

Collapse 子 session 的 LLM 上下文 SHALL 只包含一次 compact summary，且 SHALL 位于 messages；system prompt SHALL 不重复携带 compactText。

#### Scenario: compact summary 只出现在 messages

- **WHEN** daemon 为 collapse 子 session 组装上下文
- **THEN** messages 包含一条 `collapse_context` 摘要贡献
- **AND** system prompt 不包含 compactText 全文

#### Scenario: system prompt 保留 lineage 指引

- **WHEN** daemon 为 collapse 子 session 组装 system prompt
- **THEN** system prompt 包含旧 session id / collapse node id
- **AND** 包含 `session_graph_list` / `session_graph_read` / `session_graph_status` / `session_graph_wait` / `session_graph_attach` 工具指引

### Requirement: ContextService 单一入口

daemon 侧 SHALL 通过 `ContextService::load(session)` 返回 `ContextBundle`（包含 system prompt 与 messages），组装上下文的其他代码 SHALL 不再直接调用 `compose_system_prompt` / `render_lineage` 拼装。

#### Scenario: orchestration 只调用 ContextService

- **WHEN** daemon 启动或恢复 session
- **THEN** 上下文通过 `ContextService::load` 获得
- **AND** 返回的 `ContextBundle` 同时包含 system prompt 与 messages

### Requirement: 投影不修改 canonical log

上下文投影 SHALL 从 session entries 派生，不得改写、删除或覆盖已持久化的 session entries。

#### Scenario: collapse 投影不破坏旧 session

- **WHEN** 系统为 collapse 子 session 或目标 session 构建上下文
- **THEN** 旧 session 与目标 session 的原有 entries 保持不变
- **AND** 原始 transcript 仍可通过 `session_graph_read` 读取
