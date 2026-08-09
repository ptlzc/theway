## Why

`crates/server/src/tools/` 的 26 个工具文件经依赖面验证:**几乎全部只依赖 theway-core 的 AgentTool trait + theway-llm-provider + 通用库** (bash/read/grep/skill/web_fetch/git/outline/mcp_adapter/task/subagent_runner 全部如此;唯一例外是 dag_tools 引用同目录的 subagent_specs,随目录整体迁移自然解决)。工具装配 (注入 model/stream_fn/registry/harness_cell) 在 server/tools/mod.rs,是纯粹的注入式构造函数。引擎的核心能力 (工具) 放在应用层、与引擎分离,与"引擎自包含"的分层直觉不符;工具本体移入 core 后,任何嵌入 core 的消费者自带完整工具集。

## What Changes

- **移动**: `crates/server/src/tools/**` (26 文件, 含 dag_tools/ shell/ 子目录) → `crates/core/src/runtime/tools/`。
- **core 依赖新增**: `reqwest` (rustls-tls/gzip/stream, web_fetch/web_search)、`tree-sitter` 0.25 + typescript/python/rust/javascript/go (outline)、`theway-mcp` (path, mcp_adapter — mcp 不依赖 core,无环)。
- **feature 门控**: tools 挂在 `runtime` 模块下 → 随 `harness` feature 门控 (wasm/裸内核消费者不受影响)。
- **装配保留 server**: `default_tools` / `session_tool_set` / `task_tool` / `skill_tool` 等装配函数 (注入 model/stream_fn/registry/SkillHarnessCell) 留在 server (新 `src/tools.rs` 装配层)。
- **引用适配**: server 内 `crate::tools::` → `theway_core::runtime::tools::` (或经 re-export);core 内相对引用适配。

## Capabilities

### New Capabilities

- `engine-tools`: 规范工具本体归属引擎 (core/runtime/tools) 与装配归属应用层 (server) 的边界;依赖方向 tools → core 类型 + llm-provider + mcp (无 app 层依赖)。

### Modified Capabilities

- 无。

## Impact

- **代码**: core 新增 `runtime/tools/` 模块 + lib.rs re-export;server 删 tools 目录,新建装配层 `src/tools.rs` (default_tools/session_tool_set 等);main.rs 引用更新。
- **依赖**: core 加 reqwest/tree-sitter 全家/theway-mcp;server 移除对应 (reqwest/tree-sitter 若仅 tools 用)。
- **行为**: 不变 (工具语义/注册/装配不变)。
- **不改变**: AgentTool trait 位置 (core);装配注入式设计;wasm/harness feature 门控语义。
