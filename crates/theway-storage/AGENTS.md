# theway-storage 修改规则

本文件适用于 `crates/theway-storage/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改存储格式前，先阅读 [crate 概览](README.md)、[持久化架构](docs/architecture.md)和 [`theway-contract`](../theway-contract/README.md) 的叶子定义。

## 边界规则

- 在运行时工作区 crate 中，`theway-storage` 只依赖 `theway-contract`；不得导入 core 运行时或 transport 类型。
- 持久化实现直接使用 `SessionReader`、`SessionStore`、`StoredSessionEntry` 和持久化 DAG 记录，不复制其定义。
- 会话执行、图状态转换策略、协议转换和 UI 格式化留在各自归属 crate。

## 耐久性规则

- 会话数据库是用户数据：发现损坏时报告错误并保留原文件。
- DAG 数据库是可重建快照：修改恢复路径时保留一次重建并重试的行为。
- 归档成员 allowlist、大小上限、checksum、条目校验、默认关闭自动化、staging、WAL checkpoint 和 rename 提交必须作为一个完整机制维护。
- 保持每个会话一个 `<uuidv7>.db`，sidecar 路径从最终数据库路径派生。

## 测试与文档

- Schema 或序列化变化要补往返与损坏测试，归档导入失败还要断言 staging 文件已清理。
- 镜像模块测试遵循 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md)。
- 存储产物、恢复策略、归档规则或依赖边界变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-storage`、`cargo doc -p theway-storage --no-deps --document-private-items` 和 `make layering-check`。
