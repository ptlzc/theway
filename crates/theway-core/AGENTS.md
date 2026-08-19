# theway-core 修改规则

本文件适用于 `crates/theway-core/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改 agent、会话或多 agent 行为前，先阅读 [crate 概览](README.md)和[运行时架构](docs/architecture.md)。

## 边界规则

- 具体工具实现、宿主文件系统或进程代码、SQLite 类型、协议消息和遥测 exporter 不得进入 core。
- 只有当运行时机制可在 daemon 之外复用时，才通过明确的 trait 或注入闭包引入宿主相关行为。
- 保持 `theway-daemon` 为唯一直接消费运行时的工作区 crate；依赖变化后运行 `make layering-check`。
- 不启用 `harness` 和具体 provider feature 时，基础 `Agent` 构建仍须可用。

## 运行时规则

- 修改 `Agent` 或 `run_loop` 时，必须同时维护单运行准入、取消清理和终止生命周期事件。
- 带类型的会话解释留在 `agent/session`；持久化实现只通过 `PersistentSessionStorage` 接收 [`theway-contract`](../theway-contract/README.md) 记录。
- 产品事件（`LoopEvent`、`SessionEvent`、`SubagentJobEvent`、`DagEvent`）与内容安全的 `RuntimeObserver` 记录保持分离。
- DAG 状态转换校验留在 `multiagent/graph/model.rs`，调度留在 `multiagent/graph/scheduler.rs`；面向工具的命令属于 daemon。
- job 输出、transcript、队列和续轮循环必须由各自运行时类型设置上限。

## 测试与文档

- 遵循 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) 的镜像单元测试规则；多文件测试套件不得放在 `src/` 下。
- 修改异步运行时代码时，为成功、失败、超时、取消和 drop 路径补生命周期测试。
- 公开接口、归属边界、事件平面或图/会话生命周期变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-core`、[README](README.md) 中两个 `--no-default-features` 检查、`cargo doc -p theway-core --no-deps --document-private-items` 和 `make layering-check`。
