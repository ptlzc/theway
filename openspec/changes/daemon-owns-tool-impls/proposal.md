# Proposal: daemon-owns-tool-impls

## Why

三个 crate 的职责与原始分层设计不符,工具实现散落三处,新增功能时不知道放哪:

- **core 承载了 5458 行工具本体**(`src/tools/**`:dag_tools、subagent、skill 族、memory、mcp_adapter、exec、exec_shell、assembly)——这是 `tools-into-core`(已归档)留下的偏差。原始设计是 **core 只定义 interface**(AgentTool / ToolExecutor / ExecutionEnv trait + agent runtime),实现由外部提供。
- **sdk 承载了 runtime 侧实现**(`local/executor.rs` LocalExecutor、`sandbox/executor.rs` stub、`local/file_lock.rs`)——sdk 的定位是 **{tui,web} → sdk → daemon 的客户端契约**,fs 访问策略是 daemon 的资产,不是客户端契约。
- **daemon 未来分 local / sandbox 两版**:local = harness 直接跑在容器里直触 fs;sandbox = 通过 sandbox 的 gRPC 接口操作文件,不走 OS 文件系统。只有工具本体 + executor 实现集中在一处(daemon),双版本切换才是注入一个 executor 的事。

本次变更把分层收敛到原始设计,并为 daemon 双版本铺路。

## What Changes

- **core 瘦身为 interface + runtime**:
  - 移除 `src/tools/**` 全部工具本体(24 文件,约 5458 行)及对应测试 → 迁入 daemon
  - 保留 `AgentTool` trait 族、`executor` trait 族(`ToolExecutor`/`CommandOutput`/`ExecutorKind`/`ExecutorError`)、`ExecutionEnv` trait、harness/run_loop/session/compaction/multiagent 引擎
  - 移除 `native-env` feature 与 `agent/env/native.rs`(NativeEnv 实现)→ 迁入 daemon(挂 `local` feature);core 不再有实现类代码
- **daemon 成为工具/executor 实现之家**:
  - 收编 core 迁出的工具本体 + 现有 11 个本地工具(bash/edit/find/git/grep/ls/outline/read/truncate/web_fetch/web_search/write)
  - 收编 sdk 迁出的 `LocalExecutor`、`SandboxExecutor`(stub)、`FileLock`、`atomic_write`
  - 收编 `NativeEnv` 实现与 `exec.rs` 进程原语(setsid/killpg 整树 kill 语义在 daemon 内单点维护;三个消费方:bash 工具、exec_shell 家族、native env)
  - 新增 `local`(默认)/ `sandbox` 两个 feature:同一套工具本体,注入不同 executor;`sandbox` 当前挂 stub(未来接 gRPC executor)
- **sdk 回归纯客户端契约**:
  - 删除 `local/executor.rs`、`sandbox/executor.rs`、`local/file_lock.rs` 及对应测试(executor 实现完全搬出)
  - `common/` = 客户端共享契约(wire/session/config/feed/commands 框架);`local/` = 客户端本地辅助(auth store、history、images、mentions、session repo 包装、本地命令);`sandbox/` = 沙箱客户端契约占位(未来 gRPC 客户端类型)
- **引用适配**:daemon 内 `theway_core::tools::*` → `crate::tools::*`;`theway_core::NativeEnv` → `crate::…`;测试随实现迁移(tests_bridge 镜像,仓库测试规范不变)

## Impact

- **文件**:core −24(del),daemon +24(add),sdk −3(del);净移动为主,行为不变
- **core 公共 API 破坏**:`theway_core::tools` 模块树移除;`NativeEnv` re-export 移除;`native-env` feature 移除(仓库内消费者只有 daemon,已核实)
- **依赖收敛**:core 可能不再需要 `reqwest`/`tree-sitter`/`theway-mcp` 等 tools-into-core 时期新增的依赖(迁移时逐项验证后移除);daemon 承接对应依赖
- **验证**:每阶段 workspace 编译绿;终态 `cargo test --workspace` + `clippy -D warnings` + `fmt --check` + `cargo build -p theway-daemon --no-default-features --features sandbox` 特性矩阵

## 非目标

- 不实现真正的 sandbox gRPC executor(仅保留/迁移 stub,接口留缝)
- 不改工具行为语义、不改 wire/gRPC/HTTP 协议
- 不移动 theway-core 的 agent runtime(harness/run_loop/session/compaction/multiagent)本身
