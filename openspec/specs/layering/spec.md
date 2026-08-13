# Capability: layering / kernel-layers

## Purpose

分层收敛为三级:core 只定义 interface 与 engine runtime;daemon 是唯一
内核(全部工具/executor/运行时数据实现,local|sandbox 双 feature);
transport 是唯一客户端契约(wire + client + 共享模型);sdk 溶解删除,
{tui,web} 直连 transport。

## ADDED Requirements

### Requirement: Core is interface-only

theway-core 只包含 trait 定义(AgentTool 族、ToolExecutor 族、
ExecutionEnv)与 agent runtime;不包含工具本体、fs 访问实现、进程执行
实现或运行时数据(auth/session 存储等)。

#### Scenario: Tool body lookup
- **WHEN** 开发者查找某个工具(dag_plan、skill install、exec_shell)的实现
- **THEN** 它在 theway-daemon 的 `src/tools/`,不在 theway-core

#### Scenario: Runtime data lookup
- **WHEN** 开发者查找 auth store / session 仓库 / history 的实现
- **THEN** 它在 theway-daemon,不在 theway-core

#### Scenario: No OS side effects in core
- **WHEN** 开发者查找进程执行(子进程 spawn、进程组 kill)或 HTTP webhook 的实现
- **THEN** core 只有注入 seam(hooks 副作用执行器),实现位于 theway-daemon;
  core 不依赖 libc/reqwest/tokio process

### Requirement: Daemon is the single kernel

theway-daemon 承载全部工具本体、executor 实现、文件锁、进程原语、
NativeEnv 实现与运行时数据,并按 local / sandbox 两个 feature 构建。

#### Scenario: Local feature
- **WHEN** daemon 以默认 `local` feature 构建
- **THEN** 工具经 LocalExecutor 直触本地文件系统与进程表,数据落在本地磁盘

#### Scenario: Sandbox feature
- **WHEN** daemon 以 `sandbox` feature 构建(不带 local)
- **THEN** 工具注入 sandbox executor(stub 显式报 unsupported),
  组合根不触碰 OS 文件系统;未来接 gRPC executor 时工具本体零改动

#### Scenario: Single source of process-group kill semantics
- **WHEN** bash 工具、exec_shell 家族、native env 或 hook 命令执行器需要整树 kill
- **THEN** 四处共享 daemon 内同一份 setsid/killpg 原语,core 无副本
  (hooks 经注入的命令执行器消费同一原语)

### Requirement: Transport is the sole client contract

theway-transport 是 {tui,web} 客户端与 daemon 之间唯一的契约层:
wire 类型、GrpcClient/HTTP/WS 客户端、feed 模型、命令框架
(Registry/SlashCommand)与 config 公共面。客户端不依赖 daemon crate,
daemon 亦不依赖任何客户端 crate。

#### Scenario: Client dependency surface
- **WHEN** tui 或未来的 web 客户端连接 daemon
- **THEN** 客户端只依赖 theway-transport(与 core 共享类型),
  不依赖 theway-daemon

#### Scenario: Feed model home
- **WHEN** daemon 生成会话快照、tui 渲染会话
- **THEN** 双方使用 transport 的 feed 模型;UI 渲染(ratatui)留在
  tui,transport 不引入 UI 依赖

### Requirement: SDK is dissolved

`theway` SDK crate 不再存在;其模块已按映射迁入
daemon(运行时数据/实现)、transport(契约/模型)或 tui(客户端本地面)。

#### Scenario: Workspace layout
- **WHEN** 浏览 workspace 成员
- **THEN** 不存在 `theway-sdk` 成员;daemon 与 tui 均不依赖 `theway`

#### Scenario: Offline commands split
- **WHEN** 用户在 tui 输入 quit / clear / help
- **THEN** 命令由 tui 本地处理;login / logout / sessions 作为
  daemon 命令经 gRPC 执行

### Requirement: Feature matrix builds

daemon 的 `local` / `sandbox` 两个 feature 的编译矩阵必须可验证:
默认构建、`--no-default-features --features sandbox`、`--all-features`
三者全部编译通过。

#### Scenario: CI gate
- **WHEN** 运行特性矩阵构建
- **THEN** 三种组合全部成功,sandbox 组合不依赖本地文件系统实现

### Requirement: Sandbox builds never degrade silently

sandbox 组合下不可用的运行时能力必须显式告警,不得静默返回空。

#### Scenario: Sandbox skill loading warns
- **WHEN** 以 `--no-default-features --features sandbox` 构建的 daemon 加载技能/模板
- **THEN** 组合根发出明确的 warn 日志("unavailable in sandbox build"),
  不静默表现为空目录

### Requirement: Session import activation guidance is actionable

导入带自动化会话时,未激活项的指引必须可直接执行。

#### Scenario: Import with disabled automation
- **WHEN** `/session import` 导入的会话含有源端已启用的 trigger/cron
- **THEN** daemon 输出未激活项 id 与 `/triggers enable <id>` / `/cron enable <id>`
  指引;不引用不存在的 flag(如 --activate-triggers=on)

### Requirement: Transport module zoning

theway-transport 内的模块分两组语义,文档与 lib.rs 结构清晰,公共路径不变。

#### Scenario: Zone lookup
- **WHEN** 开发者判断某模块属于协议实现还是共享契约
- **THEN** lib.rs 分组声明与 crate 文档的 "Module zones" 一节给出明确归属;
  共享面模块头部标注 shared client contract
