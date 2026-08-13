# Tasks: daemon-owns-tool-impls

每项任务完成后 workspace 必须编译通过;涉及移动的任务同步迁移对应测试(tests_bridge 镜像)。逐项提交(引用 issue)。

## 1. daemon feature 骨架

- [ ] 1.1 `theway-daemon/Cargo.toml` 增加 `local`(default)/ `sandbox` feature;`local` 暂为 no-op
- [ ] 1.2 验证 `cargo build -p theway-daemon`、`cargo build -p theway-daemon --no-default-features --features sandbox` 均通过

## 2. executor 实现 sdk → daemon

- [ ] 2.1 迁 `crates/theway-sdk/src/local/executor.rs`(LocalExecutor + atomic_write)→ `crates/theway-daemon/src/executor/local.rs`;daemon 内引用适配(`crate::executor::local::LocalExecutor`)
- [ ] 2.2 迁 `crates/theway-sdk/src/sandbox/executor.rs`(SandboxExecutor)→ `crates/theway-daemon/src/executor/sandbox.rs`;挂 `sandbox` feature
- [ ] 2.3 迁 `crates/theway-sdk/src/local/file_lock.rs`(FileLock)→ `crates/theway-daemon/src/executor/file_lock.rs`;EditTool 引用适配
- [ ] 2.4 迁 `crates/theway-sdk/tests/executor.rs` → `crates/theway-daemon/tests/executor.rs`(包名/路径适配)
- [ ] 2.5 sdk 删除 executor/file_lock 模块与 `local/mod.rs` 声明;验证 sdk 生产代码零残留引用

## 3. core 工具族 → daemon

- [ ] 3.1 迁 `core/src/tools/assembly.rs` + `subagent.rs` + `dag_tools/` → `daemon/src/tools/`;适配 daemon `tools.rs` 装配层(`theway_core::tools::*` → `crate::tools::*`)
- [ ] 3.2 迁 skill 族(`skill.rs`、`skill_builder.rs`、`install_skill/`、`remove_skill.rs`、`set_skill_state.rs`)+ `memory.rs` + `mcp_adapter.rs` → daemon;core `tests/tools/*` 镜像迁移
- [ ] 3.3 迁 `exec.rs` + `exec_shell.rs` → daemon;daemon bash 工具与装配引用适配
- [ ] 3.4 core 清理:`src/tools/mod.rs`、lib.rs 中 tools 声明与 re-export 移除;验证 core 内 `crate::tools` 零引用
- [ ] 3.5 迁移工具对应的 core 测试(install_skill/remove_skill/set_skill_state/skill_builder 等 tests_bridge 目录)

## 4. native env 实现 core → daemon

- [ ] 4.1 迁 `core/src/agent/env/native.rs` → `daemon/src/env/native.rs`,挂 `local` feature;daemon `templates.rs`/`skills.rs` 注入点适配(`theway_core::NativeEnv` → `crate::env::native::NativeEnv`)
- [ ] 4.2 core 移除 `native-env` feature 与 `NativeEnv` re-export;验证 core 内 `agent::env::native` 零引用、`ExecutionEnv` trait 保留

## 5. sdk 客户端契约收口

- [ ] 5.1 sdk `sandbox/` 保留为契约占位(模块文档说明未来 gRPC 客户端类型);`local/` 文档更新(executor 已迁出)
- [ ] 5.2 更新 `openspec/specs` 与 `docs/RUST_TEST_FILES.md` 涉及的模块路径描述(如有)

## 6. 依赖收敛与终态验证

- [ ] 6.1 逐项验证 core 依赖(reqwest/tree-sitter/theway-mcp 等):只被已迁出工具使用的从 core 移除,daemon 承接
- [ ] 6.2 特性矩阵:`cargo build -p theway-daemon`、`--no-default-features --features sandbox`、`--all-features`
- [ ] 6.3 `cargo test --workspace --no-fail-fast` 全绿;`cargo clippy --workspace --all-targets -- -D warnings`;`cargo fmt --all --check`
- [ ] 6.4 归档变更(openspec archive)与 issue 收口
