# Proposal: daemon-kernel-layers

## Why

分层的实际状态与原始设计不符,而且多出一个已经完成历史使命的 crate:

- **core 承载了 5458 行工具本体**(`src/tools/**`)——`tools-into-core`(已归档)的遗留。原始设计是 core 只定义 interface(AgentTool / ToolExecutor / ExecutionEnv trait + agent runtime),实现由外部提供。
- **sdk 与 transport 契约重复**:transport 已有 wire 类型、`GrpcClient`、`spawn_daemon`、`SlashCompleter`、`base_dir()`——sdk 的"客户端契约"角色大半是重复的;而 TUI 早已跨层直连 transport(TUI 同时依赖 sdk + transport + core)。
- **sdk 的运行时数据放错了层**:auth store / session repo 包装 / history / images / mentions / bug_report 是 daemon 进程读写的数据,却挂在"客户端 SDK"里。
- **daemon 是唯一内核**(issue #13 方向:TUI 是 thewayd 的客户端,无 in-process 模式),那么"TUI 本地离线能力"面大幅萎缩,sdk 作为中间层的存在理由消失。

本次变更把分层收敛为:core = interface + engine;daemon = 全部实现与运行时数据(唯一内核,local|sandbox 双 feature);transport = 唯一客户端契约(wire + client + 共享模型);sdk 溶解删除。

## What Changes

- **core 瘦身为 interface + runtime**(与上一版提案相同):
  - 移除 `src/tools/**` 全部工具本体(24 文件,约 5458 行)及对应测试 → daemon
  - 移除 `native-env` feature 与 `agent/env/native.rs` → daemon(挂 `local` feature)
  - 保留 `AgentTool` trait 族、`executor` trait 族、`ExecutionEnv` trait、harness/run_loop/session/compaction/multiagent 引擎
- **daemon 成为唯一内核**:
  - 收编 core 工具本体 + 现有 11 个本地工具 + `LocalExecutor`/`SandboxExecutor`(stub)/`FileLock`/`atomic_write` + `NativeEnv` 与 `exec.rs` 进程原语
  - 收编 sdk 的运行时数据:`local/auth`、`local/session`、`local/history`、`local/images`、`local/mentions`、`local/bug_report`、`common/session_archive`、`local/stream_auth`、仅 daemon 使用的 config 解析(config_readers、parse_builtin_skills_config)
  - 新增 `local`(默认)/ `sandbox` 两个 feature:同一套工具本体,注入不同 executor;`sandbox` 当前挂 stub(未来 gRPC executor)
  - 收编原 sdk 离线命令中的运行时部分:login / logout / sessions 转 daemon gRPC 命令
- **transport 成为唯一客户端契约**:
  - 收编 sdk `common/feed` 模型(不含 ratatui 渲染)、`common/commands` 框架(Registry / SlashCommand / CommandOutcome / CommandCtx)、`common/triggers` 类型、`common/config` 公共面
  - `base_dir()` 唯一化:合并到 transport 现有的 `client::base_dir`
  - transport 依赖面不变(theway-core + theway-llm-provider + serde/toml 等已具备或承接)
- **tui 收编客户端本地面**:
  - quit / clear / help 三个纯 UI 命令 + feed 的 ratatui 渲染 + OAuth 浏览器打开辅助(如有)
  - TUI 依赖收敛为 transport(+ core / llm-provider),不再依赖 sdk
- **删除 sdk crate**:`crates/theway-sdk` 整个移除,workspace member 移除;daemon / tui 的 `theway::` 引用全部改写
- **引用适配与测试迁移**:测试随实现走(executor/会话/归档/auth/feed/commands 测试迁到对应 crate);每阶段 workspace 编译绿

## Impact

- **破坏性**:`theway` SDK crate 对外移除(外部 embedder 断依赖)——仓库当前无外部消费者,标注为 breaking
- **文件**:core −24;daemon +约 35;transport +约 10;tui +约 4;sdk −28(全删)
- **依赖**:daemon / tui 移除对 `theway` 的依赖;core 移除 tools-into-core 时期新增的 reqwest / tree-sitter / theway-mcp(若仅迁出工具使用,逐项验证);transport 承接 toml / serde 等 config 面依赖
- **行为不变**:工具语义、wire/gRPC/HTTP 协议、会话存储格式均不改
- **验证**:每阶段 workspace 编译绿;终态 `cargo test --workspace` + `clippy -D warnings` + `fmt --check` + daemon 特性矩阵(`default` / `--no-default-features --features sandbox` / `--all-features`)

## 非目标

- 不实现真正的 sandbox gRPC executor(保留 stub 与接口缝)
- 不改 wire/gRPC/HTTP 协议、不改工具行为语义
- 不移动 core 的 agent runtime 本身(harness/run_loop/session/compaction/multiagent)
- 不在本次做 web 客户端(只保证 transport 契约对其可用)
