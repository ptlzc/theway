# Tasks: tools-into-core

## 1. 移动 + 依赖 + 装配拆分 (N1)

- [ ] 1.1 `git mv crates/server/src/tools crates/core/src/runtime/tools` (26 文件, 含 dag_tools/ shell/ 子目录)。
- [ ] 1.2 `crates/core/Cargo.toml` 加依赖: reqwest (rustls-tls/gzip/stream)、tree-sitter 0.25 + tree-sitter-typescript/python/rust/javascript/go (版本与 server 原一致)、theway-mcp (path = "../mcp")。
- [ ] 1.3 `crates/core/src/runtime/mod.rs` 加 `pub mod tools;`;lib.rs 按需 re-export tools 类型 (AgentTool 已在;装配层需要什么补什么)。
- [ ] 1.4 装配拆分: server 新建 `crates/server/src/tools.rs` — 从原 tools/mod.rs 迁出装配函数 (default_tools/session_tool_set/task_tool/skill_tool/install_skill_tool/skill_builder_tool/set_skill_state_tool/remove_skill_tool/new_cron_job_tool/subagent_read_only_tools 等, 引用改 `theway_core::runtime::tools::`);core 的 tools/mod.rs 只留模块声明 + 纯类型。
- [ ] 1.5 server 引用适配: grep `crate::tools::` (main.rs/session 工厂/其他) → `theway_core::runtime::tools::` 或经装配层;`use crate::tools::` 全部更新。
- [ ] 1.6 server Cargo.toml 移除仅 tools 用的依赖 (reqwest/tree-sitter 若不再被 server 其他代码用 — grep 确认;memory.rs 若用 server 的 memory 扩展则评估)。
- [ ] 1.7 验收: `cargo build --workspace` + `cargo test -p theway-core --lib` 通过。

## 2. 验证 (N2)

- [ ] 2.1 `cargo test --workspace --no-fail-fast`: 新增 0 失败 (既有 Windows 环境失败除外)。
- [ ] 2.2 `cargo clippy --workspace --all-targets --features tui -- -D warnings` + `cargo fmt --all --check`。
- [ ] 2.3 结构校验: `crates/server/src/tools/` 不存在;`crates/core/src/runtime/tools/` 存在;grep `crate::tools` (server) 为空。
- [ ] 2.4 依赖校验: core Cargo.toml 有 reqwest/tree-sitter/theway-mcp;theway-mcp 无 core 依赖。
- [ ] 2.5 提交并推送。
