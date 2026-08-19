# theway-daemon 修改规则

本文件适用于 `crates/theway-daemon/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改应用组装或协议行为前，先阅读 [crate 概览](README.md)、[daemon 架构](docs/architecture.md)和[工作区 daemon 定位](../../AGENTS.md#daemon-positioning)。

## 归属规则

- 可复用的 agent 循环、会话、可观测性和图引擎机制放在 [`theway-core`](../theway-core/README.md)；具体宿主策略与适配器放在本 crate。
- Wire 记录和传输服务放在 [`theway-transport`](../theway-transport/README.md)，持久化实现放在 [`theway-storage`](../theway-storage/README.md)，所有客户端外观与交互放在 [`theway-tui`](../theway-tui/README.md)。
- 面向模型的工具实现放在 `src/tools/`，不得把具体工具行为移入 core。
- `src/lib.rs` 的公开导出必须有明确意图；除非嵌入方需要稳定扩展点，新内部模块保持私有。

## 组装规则

- 初始、恢复和切换会话全部经过 `SessionRuntimeBuilder`，不得创建第二条 harness 组装路径。
- 进程生命周期级注册表由 `DaemonServices` 持有并注入会话/运行时 builder，不引入进程全局变量。
- 编排只依赖 `RuntimeStorage` 和 `SessionRepository`；具体本地或远程适配器封装 SQLite 与 RPC 细节。
- 宿主路径在启动时通过 `DaemonPaths` 解析，并显式传给消费者。
- 跨客户端操作先定义 transport 类型，再通过适配器和端点 trait 实现 daemon 语义。
- 不支持的 sandbox 执行和显式配置错误保持快速失败。

## 测试与文档

- 镜像单元测试遵循 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md)，仅组装后行为使用进程或网络测试。
- 启动、恢复、会话切换、取消、服务替换、本地/远程存储和传输适配路径变化时补覆盖。
- 组装归属、公开扩展点、存储 port、会话构建或协议适配变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-daemon`、`cargo doc -p theway-daemon --no-deps --document-private-items` 和 `make layering-check`；适配行为变化时再运行对应 transport 或 storage 测试。
