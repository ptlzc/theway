# Design: tools-into-core

## 修订记录 (2026-08-17)

最终分层在原提案基础上做了修正 (commit `f290c0b`):

- **runtime/tools (core)** = harness 支撑: dag_tools/skill*/memory/subagent_runner/subagent_specs/task/node_launcher。
- **core/tools (core)** = 引擎通用能力: bash/fs/git/grep/find/ls/outline/mcp/truncate/shell + 装配 `default_tools`/`subagent_read_only_tools`。
- **server (应用层)** = agent 能力: **web_fetch/web_search 移回 `server/src/tools/`** — web 是 agent 的联网调研能力,不是 harness 运行支撑,不应在引擎里;subagent 工具集随之移除 web 能力 (纯本地)。

分层判据: 该工具是否支撑 harness 运行 (subagents 编排/DAG/skill/memory) → runtime;是否引擎通用能力 → core/tools;是否应用级 agent 能力 (如 web, 需外部 API key 配置) → server。

## Context

依赖面调研结论 (grep 全部 26 个工具): 工具本体只引用 theway-core (AgentTool trait/类型)、theway-llm-provider (Tool/UserContentBlock)、theway-mcp (mcp_adapter) 与通用库。工具装配 (default_tools/session_tool_set/task_tool/skill_tool 等) 是注入式构造函数 (model/stream_fn/registry/SkillHarnessCell 由调用方传入)。dag_tools 引用同目录 subagent_specs (随目录整体迁移解决)。theway-mcp 不依赖 core (无环)。

约束: AgentTool trait 位置不变 (core);装配注入式设计不变;harness feature 门控语义不变 (tools 随 runtime 模块门控);wasm 消费者不受影响。

## Goals / Non-Goals

**Goals:**

- 工具本体 (26 文件) 移入 `core/runtime/tools`,引擎自包含。
- 装配层留 server (注入式),不改装配语义。
- 依赖无环 (core → mcp 单向)。
- 全量测试/clippy/fmt 绿。

**Non-Goals:**

- 不改工具行为/参数/注册名。
- 不做工具抽象重构 (如拆分 read/write 的共享逻辑)。
- 不移动 AgentTool trait 本身 (已在 core)。

## Decisions

### Decision: 整体迁移, 装配拆分

`crates/server/src/tools/**` 整体 `git mv` 到 `crates/core/src/runtime/tools/`;`mod.rs` 中属于装配的函数 (default_tools/session_tool_set/task_tool/skill_tool/install_skill_tool/skill_builder_tool/set_skill_state_tool/remove_skill_tool/new_cron_job_tool/subagent_read_only_tools + 工具注册辅助) 移到 server 新装配层 `src/tools.rs`;core 的 tools/mod.rs 只保留模块声明 (pub mod xxx) 与纯类型定义 (如 SkillHarnessCell 定义位置按依赖决定 — 若 skill.rs 定义则随迁,装配函数引用 theway_core::runtime::tools::xxx)。

理由: 装配注入的是运行时对象 (model/stream_fn/harness cell),属应用层组装职责;工具本体无状态化后归属引擎。

### Decision: core 依赖新增

reqwest (rustls-tls/gzip/stream)、tree-sitter 0.25 + typescript/python/rust/javascript/go、theway-mcp (path)。版本与 server 现有保持一致。server 移除仅 tools 用的依赖 (reqwest/tree-sitter,若 main.rs 等仍用 reqwest 则保留)。

### Decision: 引用适配

core 内: 相对引用保持 (tools 内 use super::xxx);`crate::` 指向 core。server 内: `crate::tools::` → `theway_core::runtime::tools::` (或经 core lib.rs re-export 后 `theway_core::...`);main.rs / session factory 的装配调用更新。

## Risks / Trade-offs

- [core 依赖变重 (reqwest/tree-sitter)] → 全部挂在 harness feature 的 runtime 模块下,wasm/裸内核不携带;文档注明。
- [装配拆分漏函数] → grep `crate::tools::` 引用复核 + 编译错误定位。
- [skill.rs 的 SkillHarnessCell 与装配耦合] → cell 类型随迁 core,装配构造 cell 时注入 harness 引用 (现有模式保持)。
- [mcp_adapter 引入 core→mcp 依赖] → 已确认 mcp 不依赖 core,单向无环;若未来 mcp 需要 core 类型,改回注入式。

## Migration Plan

1. **N1 移动+依赖+注册**: git mv tools 目录 → core/runtime/tools;core Cargo.toml 加依赖;core lib.rs/runtime/mod.rs 注册 tools 模块;server 建装配层 tools.rs (迁装配函数);server 删 tools 目录;引用适配 (crate::tools:: → theway_core::runtime::tools::)。验收: `cargo build --workspace` + `cargo test -p theway-core --lib`。
2. **N2 验证**: 全量 test/clippy/fmt;结构校验 (server 无 tools 目录,core 有 runtime/tools);依赖校验 (core 有 reqwest/tree-sitter/mcp,无环)。

回滚: 每步独立 commit;git mv 保留历史。

## Open Questions

- 无 (依赖面已核实)。
