# Tasks: daemon-kernel-layers

每项任务完成后 workspace 必须编译通过;移动任务同步迁移对应测试(tests_bridge 镜像)。逐项提交(引用 issue #18)。

## 1. daemon feature 骨架

- [x] 1.1 `theway-daemon/Cargo.toml` 增加 `local`(default)/ `sandbox` feature;`local` 暂为 no-op
- [x] 1.2 验证 `cargo build -p theway-daemon`、`cargo build -p theway-daemon --no-default-features --features sandbox` 均通过

## 2. executor 实现 sdk → daemon

- [x] 2.1 迁 `crates/theway-sdk/src/local/executor.rs`(LocalExecutor + atomic_write)→ `crates/theway-daemon/src/executor/local.rs`;daemon 引用适配
- [x] 2.2 迁 `crates/theway-sdk/src/sandbox/executor.rs`(SandboxExecutor)→ `crates/theway-daemon/src/executor/sandbox.rs`;挂 `sandbox` feature
- [x] 2.3 迁 `crates/theway-sdk/src/local/file_lock.rs` → `crates/theway-daemon/src/executor/file_lock.rs`;EditTool 引用适配
- [x] 2.4 迁 `crates/theway-sdk/tests/executor.rs` → `crates/theway-daemon/tests/executor.rs`(路径/包名适配)
- [x] 2.5 sdk 删除 executor/file_lock 模块声明;验证 sdk 生产代码零残留引用

## 3. core 工具族 → daemon

- [x] 3.1 迁 `core/src/tools/assembly.rs` + `subagent.rs` + `dag_tools/` → `daemon/src/tools/`;daemon `tools.rs` 装配层 `theway_core::tools::*` → `crate::tools::*`
- [x] 3.2 迁 skill 族(`skill.rs`、`skill_builder.rs`、`install_skill/`、`remove_skill.rs`、`set_skill_state.rs`)+ `memory.rs` + `mcp_adapter.rs` → daemon;core `tests/tools/*` 镜像迁移
- [x] 3.3 迁 `exec.rs` + `exec_shell.rs` → daemon;bash 工具与装配引用适配
- [x] 3.4 core 清理:`src/tools/mod.rs`、lib.rs 中 tools 声明与 re-export 移除;验证 core 内 `crate::tools` 零引用
- [x] 3.5 迁移工具对应的 core 测试(install_skill/remove_skill/set_skill_state/skill_builder 等 tests_bridge 目录)

## 4. native env 实现 core → daemon

- [x] 4.1 迁 `core/src/agent/env/native.rs` → `daemon/src/env/native.rs`,挂 `local` feature;daemon `templates.rs`/`skills.rs` 注入点适配(`theway_core::NativeEnv` → `crate::env::native::NativeEnv`)
- [x] 4.2 core 移除 `native-env` feature 与 `NativeEnv` re-export;验证 `ExecutionEnv` trait 保留、core 内 native 零引用

## 5. sdk common → transport(契约吸收)

- [x] 5.1 迁 feed 模型(mod.rs/types.rs/preview.rs,不含 ratatui 渲染)→ `transport/src/feed/`;daemon `theway::app::feed` → `theway_transport::feed`;tui 引用适配
- [x] 5.2 迁 `common/commands` 框架 → `transport/src/commands/`;daemon/tui 引用适配
- [x] 5.3 迁 `common/triggers` 类型 → `transport/src/triggers.rs`;引用适配
- [x] 5.4 迁 `common/config` 公共面 → transport;`base_dir()` 与 `client::base_dir` 合并唯一,daemon 9 处 `theway::config::base_dir` 改 `theway_transport::client::base_dir`
- [x] 5.5 迁 feed/render.rs(ratatui 渲染)→ tui;tui 内引用适配
- [x] 5.6 transport Cargo.toml 承接 toml/serde 等依赖;验证 transport 不引入 ratatui

## 6. sdk 运行时数据 → daemon + 命令拆分

- [x] 6.1 迁 `local/auth`、`local/stream_auth` → daemon;daemon 内 `theway::auth` 引用适配;auth 测试迁移
- [x] 6.2 迁 `local/session` 包装 + `common/session_archive` → daemon;`theway::session` / `session_archive` 引用适配;测试迁移
- [x] 6.3 迁 `local/history`、`local/images`、`local/mentions`、`local/bug_report` → daemon;引用与测试迁移
- [x] 6.4 迁 `config_readers` 与仅 daemon 使用的 config 解析 → daemon
- [x] 6.5 命令拆分:quit/clear/help → tui 本地注册;login/logout/sessions → daemon 运行时命令(Registry::with_daemon_commands 调整);TUI 补全表机制适配
- [x] 6.6 tui 移除对 `theway`(sdk)的依赖,改 `theway_transport`;daemon 移除对 `theway` 的依赖

## 7. sdk 删除与终态验证

- [x] 7.1 删除 `crates/theway-sdk`,workspace member 移除;全仓 `theway::` 引用清零(保留 `theway_transport::` / `theway_core::` / `theway_daemon::`)
- [x] 7.2 依赖收敛:core 移除仅迁出工具使用的依赖(逐项验证 reqwest/tree-sitter/theway-mcp 等);`Cargo.lock` 无新增包
- [x] 7.3 特性矩阵:`cargo build -p theway-daemon`、`--no-default-features --features sandbox`、`--all-features`
- [x] 7.4 `cargo test --workspace --no-fail-fast` 全绿;`cargo clippy --workspace --all-targets -- -D warnings`;`cargo fmt --all --check`
- [ ] 7.5 归档变更(openspec archive)与 issue #18 收口

## 执行笔记 (deviations from design decision 4)

- **session 包装 + session_archive → theway-storage**(设计表原写 daemon):CLI 离线子命令
  (`theway session export/import/list/delete`) 不在 daemon 内运行,且协议不变是硬约束
  (无 export/import wire 命令),客户端亦不得依赖 daemon — storage 是两个消费方的中性归宿。
  daemon 的 `/session export|import` 命令与 TUI 的离线 CLI 均消费同一实现。
- **auth / history / mentions / bug_report(redact)/ images(编码+fs 加载)→ theway-transport**
  (设计表原写 daemon):这些模块同时被 TUI 客户端与 daemon 使用(/login 客户端交互写共享
  auth.json、输入历史、@file 展开、显示前脱敏、剪贴板/--image 编码);客户端契约层是唯一
  两边都可依赖的层。stream_auth / config_readers 按表入 daemon。
- **TUI 内 `/session export|import` 本地面移除**,改为转发 daemon 命令(daemon 实现早已存在);
  `/session switch` 仍走本地 SwitchSession RPC。
- **quit/clear/help → TUI 本地注册;login/logout/sessions → daemon with_daemon_commands 显式注册**;
  daemon 侧 /login 维持 LoginSecret 指引行为不变。
