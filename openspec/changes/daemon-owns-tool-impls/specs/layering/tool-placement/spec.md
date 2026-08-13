# Capability: layering / tool-placement

## Purpose

工具实现、执行环境实现与客户端契约分属明确的 crate 层:core 只定义
interface,daemon 拥有全部实现并按 local/sandbox 两个 feature 注入,
sdk 只承载 {tui,web} → daemon 的客户端契约。

## ADDED Requirements

### Requirement: Core is interface-only

theway-core 只包含 trait 定义(AgentTool 族、ToolExecutor 族、
ExecutionEnv)与 agent runtime(harness/run_loop/session/compaction/
multiagent);不包含任何工具本体、fs 访问实现或进程执行实现。

#### Scenario: Tool body lookup
- **WHEN** 开发者寻找某个工具(如 dag_plan、skill install、exec_shell)的实现
- **THEN** 它在 theway-daemon 的 `src/tools/` 下,而不是 theway-core

#### Scenario: Core embeds without tools
- **WHEN** 一个嵌入式消费者依赖 theway-core
- **THEN** core 的公共 API 只暴露 trait 与 runtime,不携带工具实现或
  本地文件系统实现

### Requirement: Daemon owns all implementations

theway-daemon 承载全部工具本体、executor 实现(LocalExecutor 与
sandbox stub)、文件锁(FileLock)、进程原语(setsid/killpg 整树 kill)
与 NativeEnv 实现。

#### Scenario: Local feature
- **WHEN** daemon 以默认 `local` feature 构建
- **THEN** 工具经 LocalExecutor 直触本地文件系统与进程表

#### Scenario: Sandbox feature
- **WHEN** daemon 以 `sandbox` feature 构建(不带 local)
- **THEN** 工具注入 sandbox executor(stub 显式报 unsupported),
  组合根不触碰 OS 文件系统;未来接 gRPC executor 时工具本体零改动

#### Scenario: Single source of process-group kill semantics
- **WHEN** bash 工具、exec_shell 家族或 native env 需要整树 kill
- **THEN** 三处共享 daemon 内同一份 setsid/killpg 原语,core 无副本

### Requirement: SDK is a pure client contract

theway-sdk 只承载客户端面:common(共享 wire/session/config/feed/
commands 框架)、local(客户端本地辅助)、sandbox(沙箱客户端契约占位);
不承载 executor 实现、文件锁或任何运行时文件系统策略。

#### Scenario: Executor lookup
- **WHEN** 开发者查找 ToolExecutor 的参考实现
- **THEN** 它在 theway-daemon,而非 theway-sdk

#### Scenario: Client contract independence
- **WHEN** tui/web 客户端经 sdk 连接 local 或 sandbox 版本的 daemon
- **THEN** 客户端代码无需感知 executor 种类,sdk 面保持一致

### Requirement: Feature matrix builds

daemon 的 `local` / `sandbox` 两个 feature 的编译矩阵必须可验证:
默认构建、`--no-default-features --features sandbox`、`--all-features`
三者全部编译通过。

#### Scenario: CI gate
- **WHEN** 运行特性矩阵构建
- **THEN** 三种组合全部成功,sandbox 组合不依赖本地文件系统实现
