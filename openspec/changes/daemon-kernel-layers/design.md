# Design: daemon-kernel-layers

## Context

历史偏差三条(均已在归档变更或当前代码中核实):

- `tools-into-core`:把 26 个工具文件从 server 迁入 core("引擎自包含")。本次逆转:自包含边界收窄为 *interface + runtime*。
- `sdk-split-local-sandbox`:executor trait 放 core(保留)、参考实现放 sdk(修正:实现随 daemon)。
- TUI 现状已跨层:TUI 同时依赖 sdk + transport + core,transport 已有完整客户端面(client.rs 的 GrpcClient / spawn_daemon / base_dir、wire、feed、SlashCompleter)——sdk 契约角色与 transport 重复。

结构证据:core 内对 `src/tools/` 引用为 0,harness 以 `Vec<Arc<dyn AgentTool>>` 注入消费;sdk 生产代码不引用 LocalExecutor;sdk → transport 单向依赖(吸收 common 无环);daemon 是唯一读写 auth/session 数据的进程。

## Goals / Non-Goals

- **Goals**:core=interface;daemon=唯一内核(实现+数据,local|sandbox 双 feature);transport=唯一客户端契约;sdk 溶解删除;TUI/web 直连 transport。
- **Non-Goals**:sandbox gRPC executor 实装;协议/行为变更;web 客户端实装。

## Decisions

### 1. 工具本体归属 daemon(逆转 tools-into-core)

core 保留 `AgentTool` trait 族、`executor` trait 族、`ExecutionEnv` trait;`src/tools/**` 全部迁入 daemon。装配与工具本体同层(daemon),core 不再有装配函数。

### 2. executor 实现与进程原语完全搬出 sdk/core

`LocalExecutor`(含 `atomic_write`)、`SandboxExecutor`(stub)、`FileLock` 从 sdk → daemon;`agent/env/native.rs` 与 `exec.rs`(setsid/killpg 整树 kill)从 core → daemon(挂 `local` feature),core 移除 `native-env` feature。daemon 内单点维护整树 kill 语义,三个消费方(bash 工具、exec_shell 家族、native env)共享。

### 3. daemon local/sandbox 双 feature(同一 crate)

- `local`(default):直触 fs + 进程表(LocalExecutor + NativeEnv)
- `sandbox`:注入 sandbox executor(stub,`ExecutorKind::Sandbox` 显式报错);未来接 gRPC 实现

两个 feature 共享同一套工具本体与装配;组合根按 feature 选 executor。`--no-default-features --features sandbox` 必须可编译。

### 4. sdk 溶解映射(逐模块归属 + 理由)

| sdk 模块 | 归宿 | 理由 |
|---|---|---|
| `common/feed`(模型部分) | **transport** | 会话快照/帧是 wire 契约;transport 已有 feed.rs 转换层 |
| `common/feed/render.rs`(ratatui 渲染) | **tui** | 渲染是终端客户端专有 |
| `common/commands`(Registry/SlashCommand/…) | **transport** | 命令表是 TUI(补全)与 daemon(执行)共享的契约 |
| `common/triggers` 类型 | **transport** | 触发规则经 wire 传输 |
| `common/config` 公共面 | **transport** | 客户端与 daemon 共用;`base_dir()` 与 transport `client::base_dir` 合并唯一 |
| `common/session_archive` | **daemon** | 导出/导入是 daemon 命令的实现 |
| `local/auth`、`local/stream_auth` | **daemon** | 凭证存储/刷新是运行时数据 |
| `local/session` 包装 | **daemon** | 会话仓库是 daemon 资产 |
| `local/history`、`local/images`、`local/mentions`、`local/bug_report` | **daemon** | 运行时数据 |
| `local/commands` 的 quit/clear/help | **tui** | 纯 UI 命令 |
| `local/commands` 的 login/logout/sessions | **daemon**(gRPC 命令) | 运行时能力 |
| `local/executor`、`sandbox/executor`、`local/file_lock` | **daemon** | 决策 2 |
| `config_readers` / daemon 专用解析 | **daemon** | 仅 daemon 使用 |

### 5. 命令框架拆分

transport 持有框架(Registry/SlashCommand/CommandOutcome/CommandCtx)+ 共享命令表;TUI 注册本地 UI 命令(quit/clear/help),daemon 注册运行时命令(goal/model/triggers/skills/login/logout/sessions/…)。补全表 = TUI 本地命令 ∪ daemon 命令清单(维持现有机制)。

### 6. base_dir 唯一化

以 transport `client::base_dir` 为唯一实现;daemon 与 tui 均引用 transport。避免 config 面出现双 base_dir。

### 7. 测试随实现迁移

executor/session/session_archive/auth/feed/commands 测试迁到对应 crate;core 只留 trait/harness 测试。测试规范(tests_bridge 镜像、800 行治理)不变。

### 8. 依赖收敛

- core:移除仅迁出工具使用的 reqwest / tree-sitter / theway-mcp 等(逐项验证)
- daemon:承接工具与数据面的依赖(tar / rpassword / chrono / toml 等已在或补)
- transport:承接 commands/config 面依赖(serde / toml)
- sdk 删除后 workspace 成员与引用同步清理

### 9. 迁移顺序(每步可编译)

1. daemon feature 骨架(local/sandbox,no-op)
2. executor / FileLock sdk → daemon
3. core 工具族 → daemon(assembly/subagent/dag_tools → skill 族/memory/mcp_adapter → exec/exec_shell),core lib.rs 清理
4. agent/env/native.rs → daemon,core 移除 native-env feature
5. sdk common → transport(feed 模型/commands 框架/triggers/config,base_dir 合并)
6. sdk 运行时数据 → daemon;本地命令拆分(tui 收 quit/clear/help,daemon 收 login/logout/sessions)
7. sdk crate 删除 + workspace/依赖清理 + 特性矩阵 + 全量验证

## Risks / Mitigations

- **大范围路径适配**(`theway_core::tools::` / `theway::` 数十处):按模块分批,每批 workspace 编译绿(见 tasks.md 阶段划分)。
- **sdk 对外 breaking**:仓库无外部消费者(已核实);commit 标注 breaking。
- **feed 模型与渲染拆分不彻底**:模型留在 transport 时禁止引入 ratatui 依赖(transport 保持 UI 无关);渲染全在 tui。
- **特性矩阵回归**:新增 `--no-default-features --features sandbox` 编译检查任务。
- **测试迁移遗漏**:每批同步移动 tests_bridge 镜像;终态全量测试兜底。
