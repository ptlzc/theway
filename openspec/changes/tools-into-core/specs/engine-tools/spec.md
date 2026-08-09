# Engine Tools

规范工具本体归属引擎层 (core) 与装配归属应用层 (server) 的边界。

## ADDED Requirements

### Requirement: 工具本体位于引擎

agent 工具实现 (bash/read/write/edit/grep/ls/find/outline/git/web_fetch/web_search/memory/skill 系列/mcp_adapter/task/subagent_runner/subagent_specs/dag_tools/node_launcher/truncate/shell) SHALL 定义在 `theway-core` 的 `runtime::tools` 模块内,随 `harness` feature 门控。工具实现 MUST NOT 依赖应用层 (theway crate) 的任何类型。

#### Scenario: 工具目录位置

WHEN 检查 crate 结构
THEN `crates/core/src/runtime/tools/` 存在且包含全部工具实现
AND `crates/server/src/tools/` 不存在

#### Scenario: 依赖方向

WHEN 审查任一工具实现的 `use` 语句
THEN 仅引用 theway-core 类型、theway-llm-provider、theway-mcp 与通用库
AND 不引用 theway (server 包) 的任何模块

### Requirement: 装配保留应用层

工具装配函数 (注入 model/stream_fn/registry/SkillHarnessCell/harness 引用的构造函数, 如 `default_tools` / `session_tool_set` / `task_tool` / `skill_tool`) SHALL 保留在应用层 (theway 的装配模块)。装配 SHALL 通过参数注入运行时依赖,工具本体保持无状态 (除注册时注入的字段)。

#### Scenario: 装配位置

WHEN 搜索 `default_tools` / `session_tool_set` 定义
THEN 位于 theway 包 (server),不在 theway-core
AND 它们通过参数接收 model/stream_fn/registry 等,不自行构建

### Requirement: 依赖清单

theway-core SHALL 声明 tools 所需依赖:reqwest (web_fetch/web_search)、tree-sitter 全家 (outline)、theway-mcp (mcp_adapter,path 依赖)。theway-mcp MUST NOT 依赖 theway-core (避免环)。

#### Scenario: 依赖无环

WHEN 检查 theway-mcp 的 Cargo.toml
THEN 不含 theway-core path 依赖
AND theway-core 可依赖 theway-mcp
